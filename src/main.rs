//! mf-zipgrep — fast, forensic-aware regex search inside ZIP acquisitions.
//!
//! Defines: the binary entry point, CLI parsing (the `search` and `export`
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
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand, ValueEnum};
use memmap2::Mmap;
use regex::bytes::RegexBuilder;

use mf_zipgrep::engine::{Findings, Progress, search_with_progress};
use mf_zipgrep::export::{self, ExportOutcome};
use mf_zipgrep::fast::FAST_EXCLUDE_GLOBS;
use mf_zipgrep::filter::EntryFilter;
use mf_zipgrep::inspect::{is_known_type, type_names};
use mf_zipgrep::output::{OutputFormat, write_counts, write_results};

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
    /// Export files listed in a manifest out of a ZIP archive (no search).
    Export(ExportArgs),
}

/// Arguments for `search`.
#[derive(Args)]
struct SearchArgs {
    /// Regular expression to search for (matched against raw bytes).
    pattern: String,

    /// Archive(s) to search (after the PATTERN), e.g. `a.zip b.zip`. With `-r`,
    /// a directory argument is searched recursively for its `*.zip` files. More
    /// than one archive tags each result with its source.
    #[arg(value_name = "ARCHIVE", required = true)]
    archives: Vec<PathBuf>,

    /// Search directory arguments recursively for `*.zip` files.
    #[arg(short = 'r', long = "recursive")]
    recursive: bool,

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

    /// Skip files whose internal path matches this wildcard. Repeatable; takes
    /// precedence over --path.
    #[arg(long = "not-path", value_name = "GLOB")]
    not_path: Vec<String>,

    /// Only search files of this type — a format name (e.g. `sqlite`, `jpeg`) or
    /// a category (e.g. `media`, `database`, `structured`, `text`). The type is
    /// detected by content header first, then file extension. Repeatable; an
    /// entry matching any value is searched.
    #[arg(long = "type", value_name = "TYPE")]
    file_type: Vec<String>,

    /// Search image/video/audio files too (they are skipped by default, as they
    /// hold no searchable text and dominate acquisition size).
    #[arg(long = "include-media")]
    include_media: bool,

    /// Speed preset: skip media + use all cores + the fast exclude list (see
    /// `src/fast.rs`). Bundles the common speed options behind one flag.
    #[arg(long = "fast")]
    fast: bool,

    /// Match the PATTERN against each file's internal path instead of its
    /// content, listing the files whose path matches (e.g. PATTERN `banking`
    /// finds every file with "banking" in its path). No file content is read.
    #[arg(long = "match-path")]
    match_path: bool,

    /// Inspect matching files of supported formats for richer context.
    #[arg(long = "inspect")]
    inspect: bool,

    /// Print only the match count per file (one line per file), not each match.
    #[arg(short = 'c', long = "count")]
    count: bool,

    /// Write a re-ingestable manifest of matched files (with total size) here.
    #[arg(long = "manifest", value_name = "FILE")]
    manifest: Option<PathBuf>,

    /// Also export matched files into this directory (one-step).
    #[arg(long = "export", value_name = "DIR")]
    export: Option<PathBuf>,

    /// Refuse exporting if matched files exceed this size (e.g. 200MB, 1G).
    #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size)]
    max_size: Option<u64>,

    /// Hash the archive (SHA-256) before and after the run and report whether it
    /// changed — a slower, court-defensible integrity attestation.
    #[arg(long = "verify")]
    verify: bool,

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

/// Arguments for `export`.
#[derive(Args)]
struct ExportArgs {
    /// ZIP archive to export files from.
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

    /// Hash the archive (SHA-256) before and after the run and report whether it
    /// changed — a slower, court-defensible integrity attestation.
    #[arg(long = "verify")]
    verify: bool,
}

fn main() -> Result<()> {
    match Cli::parse().command {
        Command::Search(args) => run_search(args),
        Command::Export(args) => run_export(args),
    }
}

/// Run the `export` subcommand: re-ingest files listed in a manifest.
fn run_export(args: ExportArgs) -> Result<()> {
    let mmap = open_archive(&args.archive)?;
    let verify_before = args.verify.then(|| sha256_hex(&mmap));

    let manifest_file = File::open(&args.from_manifest)
        .with_context(|| format!("cannot open manifest {}", args.from_manifest.display()))?;
    let manifest = export::read_manifest(BufReader::new(manifest_file))?;

    match export::export_from_manifest(&manifest, &mmap, &args.to, args.max_size)? {
        ExportOutcome::Exported {
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
                "exported {files} files ({bytes} bytes) to {}{note}",
                args.to.display()
            );
        }
        ExportOutcome::Refused { total_size, cap } => {
            eprintln!(
                "refusing to export: manifest total {total_size} bytes exceeds --max-size {cap}; nothing written"
            );
        }
    }

    if let Some(before) = verify_before {
        report_verify(&before, &sha256_hex(&mmap));
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

    // Resolve the archive arguments into sources, each with a display label
    // (relative path under a -r directory, else the path as given).
    let sources = gather_sources(&cli.archives, cli.recursive)?;
    if sources.is_empty() {
        anyhow::bail!("no archives given — list archive files, or use -r with a directory");
    }
    let multi = sources.len() > 1;
    if multi && (cli.export.is_some() || cli.manifest.is_some()) {
        anyhow::bail!("--export/--manifest require a single archive");
    }

    // --fast bundles the speed options: skip media (default) + all cores
    // (default) + the fast exclude list, added to any --not-path globs.
    let mut excludes = cli.not_path.clone();
    if cli.fast {
        excludes.extend(FAST_EXCLUDE_GLOBS.iter().map(|g| g.to_string()));
    }
    for t in &cli.file_type {
        if !is_known_type(t) {
            anyhow::bail!(
                "unknown --type '{t}'; valid values: {}",
                type_names().join(", ")
            );
        }
    }

    // --match-path reads no content, so anything needing the file's bytes is
    // meaningless alongside it.
    if cli.match_path {
        if cli.inspect {
            anyhow::bail!("--match-path cannot be combined with --inspect (no content is read)");
        }
        if !cli.file_type.is_empty() {
            anyhow::bail!("--match-path cannot be combined with --type (no content is read)");
        }
    }

    let filter = EntryFilter::new(&cli.path, &excludes, &cli.file_type, !cli.include_media);

    if cli.count {
        // Per-file counts aggregated across archives (path tagged when multi).
        let mut counts: Vec<(String, usize)> = Vec::new();
        for src in &sources {
            let mmap = open_archive(&src.path)?;
            let verify_before = cli.verify.then(|| sha256_hex(&mmap));
            let findings = search_with_reporter(&mmap, &re, false, cli.match_path, &filter)?;
            for f in &findings.files {
                let path = if multi {
                    format!("{}/{}", src.label, f.entry.name)
                } else {
                    f.entry.name.clone()
                };
                counts.push((path, f.offsets.len()));
            }
            if let Some(before) = verify_before {
                report_verify(&before, &sha256_hex(&mmap));
            }
        }
        let pairs: Vec<(&str, usize)> = counts.iter().map(|(p, c)| (p.as_str(), *c)).collect();
        emit(cli.output.as_deref(), |w| {
            write_counts(&pairs, cli.format, w)
        })?;
    } else {
        // Match records aggregated across archives (tagged when multi).
        let mut records = Vec::new();
        for src in &sources {
            let mmap = open_archive(&src.path)?;
            let verify_before = cli.verify.then(|| sha256_hex(&mmap));
            let mut findings = search_with_reporter(&mmap, &re, cli.inspect, cli.match_path, &filter)?;
            if multi {
                for r in &mut findings.records {
                    r.archive = Some(src.label.clone());
                }
            } else {
                // --manifest/--export only apply to a single archive (guarded above).
                export_if_requested(&cli, &mmap, &findings)?;
            }
            records.append(&mut findings.records);
            if let Some(before) = verify_before {
                report_verify(&before, &sha256_hex(&mmap));
            }
        }
        emit(cli.output.as_deref(), |w| {
            write_results(&records, cli.format, colourise, cli.match_path, w)
        })?;
    }

    Ok(())
}

/// An archive to search, plus the label shown for it (relative to a `-r`
/// directory, or the path as given).
struct Source {
    path: PathBuf,
    label: String,
}

/// Memory-map an archive read-only.
///
/// SAFETY: the archive is treated as read-only forensic evidence; we do not
/// mutate it and assume it is not concurrently truncated during the scan.
fn open_archive(path: &Path) -> Result<Mmap> {
    let file =
        File::open(path).with_context(|| format!("cannot open archive {}", path.display()))?;
    unsafe { Mmap::map(&file) }.with_context(|| format!("cannot mmap archive {}", path.display()))
}

/// Turn the operand paths into [`Source`]s.
///
/// A file becomes one source labelled by its given path. A directory (only with
/// `recursive`) is walked for `*.zip`, each labelled by its path **relative to
/// that directory**, so output reads like `sub/case.zip/internal/file`.
fn gather_sources(paths: &[PathBuf], recursive: bool) -> Result<Vec<Source>> {
    let mut sources = Vec::new();
    for arg in paths {
        if arg.is_dir() {
            if !recursive {
                anyhow::bail!(
                    "{} is a directory; pass -r to search it recursively",
                    arg.display()
                );
            }
            let mut zips = Vec::new();
            collect_zips(arg, &mut zips)
                .with_context(|| format!("cannot read directory {}", arg.display()))?;
            zips.sort();
            for zip in zips {
                let label = zip
                    .strip_prefix(arg)
                    .unwrap_or(&zip)
                    .to_string_lossy()
                    .into_owned();
                sources.push(Source { path: zip, label });
            }
        } else {
            sources.push(Source {
                path: arg.clone(),
                label: arg.to_string_lossy().into_owned(),
            });
        }
    }
    Ok(sources)
}

/// Recursively collect `*.zip` files under `dir` into `out`.
fn collect_zips(dir: &Path, out: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            collect_zips(&path, out)?;
        } else if path
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("zip"))
        {
            out.push(path);
        }
    }
    Ok(())
}

/// SHA-256 of `bytes` as lowercase hex.
fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Print the before/after archive hashes and whether the evidence was unchanged.
///
/// mf-zipgrep opens the archive read-only and never writes to it, so the hashes
/// match by construction; this records that fact as an attestation. Goes to
/// stderr so it never mixes with results on stdout.
fn report_verify(before: &str, after: &str) {
    eprintln!("verify: archive sha256 before  {before}");
    eprintln!("verify: archive sha256 after   {after}");
    if before == after {
        eprintln!("verify: archive unchanged during the run (read-only integrity confirmed)");
    } else {
        eprintln!("verify: WARNING — archive changed during the run");
    }
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
    match_path: bool,
    filter: &EntryFilter,
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

    let result = search_with_progress(archive, re, deep, match_path, filter, progress.as_ref());

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

/// Write a manifest and/or export matched files, if requested.
///
/// Status goes to stderr so it never pollutes the match results on stdout.
fn export_if_requested(cli: &SearchArgs, archive: &[u8], findings: &Findings) -> Result<()> {
    if cli.manifest.is_none() && cli.export.is_none() {
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

    if let Some(dir) = &cli.export {
        match export::export_files(&plan, archive, &findings.files, dir, cli.max_size)? {
            ExportOutcome::Exported { files, bytes, .. } => {
                eprintln!("exported {files} files ({bytes} bytes) to {}", dir.display());
            }
            ExportOutcome::Refused { total_size, cap } => {
                eprintln!(
                    "refusing to export: matched total {total_size} bytes exceeds --max-size {cap}; \
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

/// Run `render` against the chosen sink: a file when `-o` was given, else
/// stdout. Shared by the match output and the `--count` output.
fn emit(
    output: Option<&std::path::Path>,
    render: impl FnOnce(&mut dyn Write) -> Result<()>,
) -> Result<()> {
    match output {
        Some(path) => {
            let file =
                File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
            let mut w = BufWriter::new(file);
            render(&mut w)?;
            w.flush().context("failed flushing output file")
        }
        None => {
            let stdout = std::io::stdout();
            let mut w = stdout.lock();
            render(&mut w)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_size, sha256_hex};

    #[test]
    fn sha256_matches_known_vectors() {
        assert_eq!(
            sha256_hex(b""),
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        );
        assert_eq!(
            sha256_hex(b"abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn parse_size_handles_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("2MB").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size("nope").is_err());
    }
}
