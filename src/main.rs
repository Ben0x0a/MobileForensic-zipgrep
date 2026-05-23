//! mf-zipgrep — fast, forensic-aware regex search inside ZIP acquisitions.
//!
//! Defines: the binary entry point, CLI parsing (the `search` and `pull`
//! subcommands), and orchestration that ties the parser, the search engine, the
//! inspectors and the output/export writers together.
//! Used by: invoked from the shell (`mf-zipgrep search PATTERN ARCHIVE`).
//! Uses: the `mf_zipgrep` library crate (`engine`, `export`, `filter`,
//! `output`), plus `memmap2` (zero-copy access), `clap` (CLI), `rayon` (the
//! progress reporter), `anyhow`.
//!
//! Why mmap: phone acquisitions are large, and STORED entries (the common case)
//! are uncompressed on disk, so memory-mapping lets the SIMD regex engine run
//! straight over those bytes with no copy and no decompression — the core speed
//! win over `zipgrep`. DEFLATE entries are decompressed on demand.
//!
//! Every match reports the file path plus three offsets (file start, position
//! within the file, absolute position in the archive) — locating evidence is
//! the goal, so the "where" is always present, never gated behind a flag.

use std::fs::File;
use std::io::{BufReader, BufWriter, IsTerminal, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use memmap2::Mmap;
use regex::bytes::RegexBuilder;

use mf_zipgrep::engine::{Findings, Progress, search_with_progress};
use mf_zipgrep::export::{self, PullOutcome};
use mf_zipgrep::filter::PathFilter;
use mf_zipgrep::models::MatchRecord;
use mf_zipgrep::output::{OutputFormat, write_results};

/// When to colourise matched text in the output.
#[derive(Clone, Copy, ValueEnum)]
enum ColourWhen {
    /// Colourise only when writing txt to a terminal.
    Auto,
    Always,
    Never,
}

/// Fast regex search inside the files of a ZIP archive (mobile-forensic zipgrep).
#[derive(Parser)]
#[command(name = "mf-zipgrep", version, about)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Search for a regex inside the files of a ZIP archive.
    Search(SearchArgs),
    /// Pull files listed in a manifest out of a ZIP archive (no search).
    Pull(PullArgs),
}

/// Arguments for `search`.
#[derive(Args)]
struct SearchArgs {
    /// Regular expression to search for (matched against raw bytes).
    pattern: String,

    /// ZIP archive to search.
    archive: PathBuf,

    /// Case-insensitive matching.
    #[arg(short = 'i', long)]
    ignore_case: bool,

    /// Treat the pattern as a literal string instead of a regex.
    #[arg(short = 'F', long = "fixed-strings")]
    fixed_strings: bool,

    /// Accepted for grep compatibility; the engine is already ERE-like, so this
    /// has no effect.
    #[arg(short = 'E', long = "extended-regexp")]
    extended_regexp: bool,

    /// Output format.
    #[arg(long, default_value = "txt")]
    format: OutputFormat,

    /// Write results to this file instead of stdout.
    #[arg(short = 'o', long = "output")]
    output: Option<PathBuf>,

    /// Number of search threads (default: one per CPU core).
    #[arg(short = 'j', long = "threads")]
    threads: Option<usize>,

    /// Only search files whose internal path matches this wildcard (`*`, `?`).
    /// Repeatable; an entry matching any pattern is searched.
    #[arg(long = "path", value_name = "GLOB")]
    path: Vec<String>,

    /// Inspect matching files of supported formats for richer context.
    #[arg(long = "inspect")]
    inspect: bool,

    /// Write a re-ingestable manifest of matched files (with total size) here.
    #[arg(long = "manifest", value_name = "FILE")]
    manifest: Option<PathBuf>,

    /// Also pull matched files into this directory (one-step).
    #[arg(long = "pull", value_name = "DIR")]
    pull: Option<PathBuf>,

    /// Refuse pulling if matched files exceed this size (e.g. 200MB, 1G).
    #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size)]
    max_size: Option<u64>,

    /// Highlight matches (txt to a terminal only): auto, always, or never.
    #[arg(
        long = "colour",
        visible_alias = "color",
        value_enum,
        default_value = "auto",
        default_missing_value = "always",
        num_args = 0..=1,
    )]
    colour: ColourWhen,
}

/// Arguments for `pull`.
#[derive(Args)]
struct PullArgs {
    /// ZIP archive to pull files from.
    archive: PathBuf,

    /// Manifest written by a previous `search --manifest`.
    #[arg(long = "from-manifest", value_name = "FILE")]
    from_manifest: PathBuf,

    /// Destination directory.
    #[arg(long = "to", value_name = "DIR")]
    to: PathBuf,

    /// Refuse if the manifest's total size exceeds this (e.g. 200MB, 1G).
    #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size)]
    max_size: Option<u64>,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Search(args) => run_search(args),
        Command::Pull(args) => run_pull(args),
    }
}

/// Run the `pull` subcommand: re-ingest files listed in a manifest.
fn run_pull(args: PullArgs) -> Result<()> {
    // SAFETY: read-only forensic evidence; not mutated, assumed stable.
    let file = File::open(&args.archive)
        .with_context(|| format!("cannot open archive {}", args.archive.display()))?;
    let mmap = unsafe { Mmap::map(&file) }
        .with_context(|| format!("cannot mmap archive {}", args.archive.display()))?;

    let manifest_file = File::open(&args.from_manifest)
        .with_context(|| format!("cannot open manifest {}", args.from_manifest.display()))?;
    let manifest = export::read_manifest(BufReader::new(manifest_file))?;

    match export::pull_from_manifest(&manifest, &mmap, &args.to, args.max_size)? {
        PullOutcome::Pulled {
            files,
            bytes,
            skipped,
        } => {
            let note = if skipped > 0 {
                format!("; {skipped} listed file(s) not found in archive")
            } else {
                String::new()
            };
            eprintln!(
                "pulled {files} files ({bytes} bytes) to {}{note}",
                args.to.display()
            );
        }
        PullOutcome::Refused { total_size, cap } => {
            eprintln!(
                "refusing to pull: manifest total {total_size} bytes exceeds --max-size {cap}; nothing written"
            );
        }
    }
    Ok(())
}

/// Run the `search` subcommand.
fn run_search(cli: SearchArgs) -> Result<()> {
    // -E is a no-op: our regex flavour is already extended. Consume the field
    // so it counts as read while documenting that acceptance is deliberate.
    let _ = cli.extended_regexp;

    // -F means "match this exact text", so escape any regex metacharacters.
    let pattern = if cli.fixed_strings {
        regex::escape(&cli.pattern)
    } else {
        cli.pattern.clone()
    };
    let re = RegexBuilder::new(&pattern)
        .case_insensitive(cli.ignore_case)
        .build()
        .context("invalid regular expression")?;

    // Colour is meaningful only for txt sent to a terminal; never inject ANSI
    // into a file or a machine-readable format.
    let to_stdout = cli.output.is_none();
    let colourise = to_stdout
        && cli.format == OutputFormat::Txt
        && match cli.colour {
            ColourWhen::Always => true,
            ColourWhen::Never => false,
            ColourWhen::Auto => std::io::stdout().is_terminal(),
        };

    if let Some(n) = cli.threads {
        rayon::ThreadPoolBuilder::new()
            .num_threads(n)
            .build_global()
            .context("failed to configure thread pool")?;
    }

    // SAFETY: the archive is treated as read-only forensic evidence; we do not
    // mutate it and assume it is not concurrently truncated during the scan.
    let file = File::open(&cli.archive)
        .with_context(|| format!("cannot open archive {}", cli.archive.display()))?;
    let mmap = unsafe { Mmap::map(&file) }
        .with_context(|| format!("cannot mmap archive {}", cli.archive.display()))?;

    let filter = PathFilter::new(&cli.path);
    let findings = search_with_reporter(&mmap, &re, cli.inspect, &filter)?;

    write_output(
        &findings.records,
        cli.format,
        colourise,
        cli.output.as_deref(),
    )?;

    export_if_requested(&cli, &mmap, &findings)
}

/// Run the search, showing a live progress line on stderr when it is a terminal.
///
/// The reporter runs on its own thread while the parallel search executes; on a
/// non-terminal stderr (piped/redirected) no reporter is spawned, so logs and
/// captured output stay clean. The shared counters carry no I/O — the engine
/// only increments them.
fn search_with_reporter(
    archive: &[u8],
    re: &regex::bytes::Regex,
    deep: bool,
    filter: &PathFilter,
) -> Result<Findings> {
    let progress = Arc::new(TtyProgress::default());
    let stop = Arc::new(AtomicBool::new(false));

    let reporter = if std::io::stderr().is_terminal() {
        let progress = Arc::clone(&progress);
        let stop = Arc::clone(&stop);
        Some(std::thread::spawn(move || {
            report_progress(&progress, &stop)
        }))
    } else {
        None
    };

    let result = search_with_progress(archive, re, deep, filter, progress.as_ref());

    stop.store(true, Ordering::Relaxed);
    if let Some(reporter) = reporter {
        let _ = reporter.join();
        eprint!("\r\x1b[K"); // clear the progress line before results are read
        let _ = std::io::stderr().flush();
    }
    result
}

/// Shared scan counters; the reporter thread reads them, the engine writes them.
#[derive(Default)]
struct TtyProgress {
    total: AtomicUsize,
    done: AtomicUsize,
}

impl Progress for TtyProgress {
    fn set_total(&self, total: usize) {
        self.total.store(total, Ordering::Relaxed);
    }
    fn inc(&self) {
        self.done.fetch_add(1, Ordering::Relaxed);
    }
}

/// Redraw `scanned X/Y files` on stderr until told to stop.
fn report_progress(progress: &TtyProgress, stop: &AtomicBool) {
    loop {
        let total = progress.total.load(Ordering::Relaxed);
        if total > 0 {
            let done = progress.done.load(Ordering::Relaxed);
            eprint!("\r\x1b[Kmf-zipgrep: scanned {done}/{total} files");
            let _ = std::io::stderr().flush();
        }
        if stop.load(Ordering::Relaxed) {
            break;
        }
        std::thread::sleep(Duration::from_millis(120));
    }
}

/// Write a manifest and/or pull matched files, if requested.
///
/// Status goes to stderr so it never pollutes the match results on stdout.
fn export_if_requested(cli: &SearchArgs, archive: &[u8], findings: &Findings) -> Result<()> {
    if cli.manifest.is_none() && cli.pull.is_none() {
        return Ok(());
    }

    let plan = export::plan(&findings.files);

    if let Some(path) = &cli.manifest {
        let file =
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
        let mut w = BufWriter::new(file);
        export::write_manifest(&plan, &mut w)?;
        w.flush().context("failed flushing manifest")?;
        eprintln!(
            "manifest: {} files, {} bytes total -> {}",
            plan.items.len(),
            plan.total_size,
            path.display()
        );
    }

    if let Some(dir) = &cli.pull {
        match export::pull(&plan, archive, &findings.files, dir, cli.max_size)? {
            PullOutcome::Pulled { files, bytes, .. } => {
                eprintln!("pulled {files} files ({bytes} bytes) to {}", dir.display());
            }
            PullOutcome::Refused { total_size, cap } => {
                eprintln!(
                    "refusing to pull: matched total {total_size} bytes exceeds --max-size {cap}; \
                     nothing written (use the manifest to review)"
                );
            }
        }
    }

    Ok(())
}

/// Parse a human size like `1024`, `200KB`, `50M`, `2G` into bytes (1024-based).
fn parse_size(s: &str) -> Result<u64, String> {
    let lower = s.trim().to_ascii_lowercase();
    let (digits, multiplier) = if let Some(n) = strip_unit(&lower, "gb").or(strip_unit(&lower, "g"))
    {
        (n, 1u64 << 30)
    } else if let Some(n) = strip_unit(&lower, "mb").or(strip_unit(&lower, "m")) {
        (n, 1u64 << 20)
    } else if let Some(n) = strip_unit(&lower, "kb").or(strip_unit(&lower, "k")) {
        (n, 1u64 << 10)
    } else {
        (lower.as_str(), 1)
    };
    let value: u64 = digits
        .trim()
        .parse()
        .map_err(|_| format!("invalid size '{s}' (expected e.g. 200MB, 1G, or a byte count)"))?;
    value
        .checked_mul(multiplier)
        .ok_or_else(|| format!("size '{s}' overflows"))
}

/// Strip a unit suffix, returning the numeric part if present.
fn strip_unit<'a>(s: &'a str, unit: &str) -> Option<&'a str> {
    s.strip_suffix(unit)
}

/// Send results to the chosen sink: a file when `-o` was given, else stdout.
fn write_output(
    records: &[MatchRecord],
    format: OutputFormat,
    colourise: bool,
    output: Option<&std::path::Path>,
) -> Result<()> {
    match output {
        Some(path) => {
            let file =
                File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
            let mut w = BufWriter::new(file);
            write_results(records, format, colourise, &mut w)?;
            w.flush().context("failed flushing output file")
        }
        None => {
            let stdout = std::io::stdout();
            let mut w = stdout.lock();
            write_results(records, format, colourise, &mut w)
        }
    }
}
