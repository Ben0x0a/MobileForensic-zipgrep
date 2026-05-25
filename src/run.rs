//! Subcommand orchestration and shared I/O helpers for the binary.
//!
//! Defines: `run_search` and `run_export` (the two subcommands), plus the
//! supporting pieces — archive mmap, source gathering, the terminal progress
//! reporter, the `--verify` hashing, manifest/export writing, and the output
//! sink.
//! Used by: `main` (dispatches to `run_search`/`run_export`).
//! Uses: `crate::cli` (the parsed argument structs) and the `mf_zipgrep` library
//! (`engine`, `export`, `filter`, `inspect`, `models`, `output`, `preset`),
//! plus `memmap2`, `rayon`, `regex`, `anyhow`.

use std::fs::File;
use std::io::{BufReader, BufWriter, IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use anyhow::{Context, Result};
use memmap2::Mmap;
use regex::bytes::RegexBuilder;

use mf_zipgrep::engine::{Findings, Progress, search_with_progress};
use mf_zipgrep::export::{self, ExportOutcome};
use mf_zipgrep::filter::EntryFilter;
use mf_zipgrep::inspect::{is_known_type, type_names};
use mf_zipgrep::models::RunInfo;
use mf_zipgrep::output::{OutputFormat, write_counts, write_results};
use mf_zipgrep::preset::fast::FAST_EXCLUDE_GLOBS;

use crate::cli::{ColourWhen, ExportArgs, SearchArgs};

/// Run the `export` subcommand: re-ingest files listed in a manifest.
pub(crate) fn run_export(args: ExportArgs) -> Result<()> {
    let mmap = open_archive(&args.archive)?;
    let verify_before = args.verify.then(|| sha256_hex(&mmap));

    let manifest_file = File::open(&args.from_manifest)
        .with_context(|| format!("cannot open manifest {}", args.from_manifest.display()))?;
    let manifest = export::read_manifest(BufReader::new(manifest_file))?;

    match export::export_from_manifest(&manifest, &mmap, &args.to, Some(args.max_size))? {
        ExportOutcome::Exported {
            files,
            bytes,
            skipped,
            report,
        } => {
            let note = if skipped > 0 {
                format!("; {skipped} listed file(s) not found in archive")
            } else {
                String::new()
            };
            // Per-file integrity record beside the exported artefacts. Reuses the
            // manifest's run metadata so the report says what produced it.
            write_export_report_file(&args.to, &manifest.run, &report)?;
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
pub(crate) fn run_search(cli: SearchArgs) -> Result<()> {
    // -E is a no-op: our regex flavour is already extended. Consume the field
    // so it counts as read while documenting that acceptance is deliberate.
    let _ = cli.extended_regexp;

    // -l means "match this exact text", so escape any regex metacharacters.
    let pattern = if cli.literal_string {
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

    // --fast bundles the speed options: exclude media + all cores (default) +
    // the fast exclude list, added to any --not-path globs.
    let mut excludes = cli.not_path.clone();
    if cli.fast {
        excludes.extend(FAST_EXCLUDE_GLOBS.iter().map(|g| g.to_string()));
    }
    // Media is searched by default; --exclude-media (or --fast) skips it.
    let exclude_media = cli.exclude_media || cli.fast;
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

    let filter = EntryFilter::new(&cli.path, &excludes, &cli.file_type, exclude_media);

    // Run metadata: the query and every filter in effect, recorded so JSON output
    // and the export report are self-describing.
    let run = RunInfo {
        tool: "mf-zipgrep".into(),
        version: env!("CARGO_PKG_VERSION").into(),
        pattern: cli.pattern.clone(),
        literal: cli.literal_string,
        ignore_case: cli.ignore_case,
        match_path: cli.match_path,
        inspect: cli.inspect,
        archives: sources.iter().map(|s| s.path.display().to_string()).collect(),
        path_globs: cli.path.clone(),
        not_path_globs: cli.not_path.clone(),
        types: cli.file_type.clone(),
        exclude_media,
    };

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
            // Every record carries its source archive's full path (for JSON);
            // multi-archive runs also get the short display label (for txt/csv).
            let full = src.path.display().to_string();
            for r in &mut findings.records {
                r.archive_path = Some(full.clone());
                if multi {
                    r.archive = Some(src.label.clone());
                }
            }
            if !multi {
                // --manifest/--export only apply to a single archive (guarded above).
                export_if_requested(&cli, &mmap, &findings, &run)?;
            }
            records.append(&mut findings.records);
            if let Some(before) = verify_before {
                report_verify(&before, &sha256_hex(&mmap));
            }
        }
        emit(cli.output.as_deref(), |w| {
            write_results(&records, cli.format, colourise, cli.match_path, &run, w)
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
fn export_if_requested(
    cli: &SearchArgs,
    archive: &[u8],
    findings: &Findings,
    run: &RunInfo,
) -> Result<()> {
    if cli.manifest.is_none() && cli.export.is_none() {
        return Ok(());
    }

    let plan = export::plan(&findings.files);

    if let Some(path) = &cli.manifest {
        let file =
            File::create(path).with_context(|| format!("cannot create {}", path.display()))?;
        let mut w = BufWriter::new(file);
        export::write_manifest(&plan, run, &mut w)?;
        w.flush().context("failed flushing manifest")?;
        eprintln!(
            "manifest: {} files, {} bytes total -> {}",
            plan.items.len(),
            plan.total_size,
            path.display()
        );
    }

    if let Some(dir) = &cli.export {
        match export::export_files(&plan, archive, &findings.files, dir, Some(cli.max_size))? {
            ExportOutcome::Exported {
                files,
                bytes,
                report,
                ..
            } => {
                write_export_report_file(dir, run, &report)?;
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

/// Write `export-report.json` (run metadata + per-file SHA-256) into the export
/// destination directory, beside the exported artefacts.
fn write_export_report_file(
    dir: &Path,
    run: &RunInfo,
    report: &[export::ExportedFile],
) -> Result<()> {
    let path = dir.join("export-report.json");
    let file = File::create(&path).with_context(|| format!("cannot create {}", path.display()))?;
    let mut w = BufWriter::new(file);
    export::write_export_report(run, report, &mut w)?;
    w.flush().context("failed flushing export report")?;
    eprintln!("export report: {} files -> {}", report.len(), path.display());
    Ok(())
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
    use super::sha256_hex;

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
}
