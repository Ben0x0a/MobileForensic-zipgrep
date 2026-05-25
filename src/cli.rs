//! Command-line interface definitions for the `mf-zipgrep` binary.
//!
//! Defines: the clap types — `Cli`, the `Command` subcommands, `SearchArgs`,
//! `ExportArgs`, `ColourWhen` — and the human size parser (`parse_size`) used by
//! the `--max-size` flag.
//! Used by: `main` (parses `Cli`) and `run` (reads the parsed argument structs).
//! Uses: `clap` (derive) and `mf_zipgrep::output::OutputFormat` (the `--format`
//! value). No search/IO logic lives here — that is `run`'s job.

use std::path::PathBuf;

use clap::{Args, Parser, Subcommand, ValueEnum};

use mf_zipgrep::output::OutputFormat;

/// When to colourise matched text in the output.
#[derive(Clone, Copy, ValueEnum)]
pub(crate) enum ColourWhen {
    /// Colourise only when writing txt to a terminal.
    Auto,
    Always,
    Never,
}

/// Fast regex search inside the files of a ZIP archive (mobile-forensic zipgrep).
#[derive(Parser)]
#[command(name = "mf-zipgrep", version, about)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Command,
}

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Search for a regex inside the files of a ZIP archive.
    Search(SearchArgs),
    /// Export files listed in a manifest out of a ZIP archive (no search).
    Export(ExportArgs),
}

/// Arguments for `search`.
#[derive(Args)]
pub(crate) struct SearchArgs {
    /// Regular expression to search for (matched against raw bytes).
    pub(crate) pattern: String,

    /// Archive(s) to search (after the PATTERN), e.g. `a.zip b.zip`. With `-r`,
    /// a directory argument is searched recursively for its `*.zip` files. More
    /// than one archive tags each result with its source.
    #[arg(value_name = "ARCHIVE", required = true)]
    pub(crate) archives: Vec<PathBuf>,

    /// Search directory arguments recursively for `*.zip` files.
    #[arg(short = 'r', long = "recursive")]
    pub(crate) recursive: bool,

    /// Case-insensitive matching.
    #[arg(short = 'i', long)]
    pub(crate) ignore_case: bool,

    /// Treat the pattern as a literal string instead of a regex.
    #[arg(short = 'l', long = "literal-string", visible_alias = "fixed-strings")]
    pub(crate) literal_string: bool,

    /// Accepted for grep compatibility; the engine is already ERE-like, so this
    /// has no effect.
    #[arg(short = 'E', long = "extended-regexp")]
    pub(crate) extended_regexp: bool,

    /// Output format.
    #[arg(short = 'f', long, default_value = "txt")]
    pub(crate) format: OutputFormat,

    /// Write results to this file instead of stdout.
    #[arg(short = 'o', long = "output")]
    pub(crate) output: Option<PathBuf>,

    /// Number of search threads (default: one per CPU core).
    #[arg(short = 'j', long = "threads")]
    pub(crate) threads: Option<usize>,

    /// Only search files whose internal path matches this wildcard (`*`, `?`).
    /// Repeatable; an entry matching any pattern is searched.
    #[arg(long = "path", value_name = "GLOB")]
    pub(crate) path: Vec<String>,

    /// Skip files whose internal path matches this wildcard. Repeatable; takes
    /// precedence over --path.
    #[arg(long = "not-path", value_name = "GLOB")]
    pub(crate) not_path: Vec<String>,

    /// Only search files of this type — a format name (e.g. `sqlite`, `jpeg`) or
    /// a category (e.g. `media`, `database`, `structured`, `text`). The type is
    /// detected by content header first, then file extension. Repeatable; an
    /// entry matching any value is searched.
    #[arg(short = 't', long = "type", value_name = "TYPE")]
    pub(crate) file_type: Vec<String>,

    /// Skip image/video/audio files. They are searched by default; this excludes
    /// them for speed (they hold no searchable text and dominate acquisition
    /// size). `--fast` implies it.
    #[arg(long = "exclude-media")]
    pub(crate) exclude_media: bool,

    /// Speed preset: exclude media + use all cores + the fast exclude list (see
    /// `src/preset/fast.rs`). Bundles the common speed options behind one flag.
    #[arg(long = "fast")]
    pub(crate) fast: bool,

    /// Match the PATTERN against each file's internal path instead of its
    /// content, listing the files whose path matches (e.g. PATTERN `banking`
    /// finds every file with "banking" in its path). No file content is read.
    #[arg(long = "match-path")]
    pub(crate) match_path: bool,

    /// Inspect matching files of supported formats for richer context.
    #[arg(long = "inspect")]
    pub(crate) inspect: bool,

    /// Print only the match count per file (one line per file), not each match.
    #[arg(short = 'c', long = "count")]
    pub(crate) count: bool,

    /// Write a re-ingestable manifest of matched files (with total size) here.
    #[arg(long = "manifest", value_name = "FILE")]
    pub(crate) manifest: Option<PathBuf>,

    /// Also export matched files into this directory (one-step).
    #[arg(long = "export", value_name = "DIR")]
    pub(crate) export: Option<PathBuf>,

    /// Refuse exporting if matched files exceed this size (e.g. 200MB, 1G).
    /// Defaults to 1G as an accident guard; raise it to export more.
    #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size, default_value = "1G")]
    pub(crate) max_size: u64,

    /// Hash the archive (SHA-256) before and after the run and report whether it
    /// changed — a slower, court-defensible integrity attestation.
    #[arg(long = "verify")]
    pub(crate) verify: bool,

    /// Highlight matches (txt to a terminal only): auto, always, or never.
    #[arg(
        long = "colour",
        visible_alias = "color",
        value_enum,
        default_value = "auto",
        default_missing_value = "always",
        num_args = 0..=1,
    )]
    pub(crate) colour: ColourWhen,
}

/// Arguments for `export`.
#[derive(Args)]
pub(crate) struct ExportArgs {
    /// ZIP archive to export files from.
    pub(crate) archive: PathBuf,

    /// Manifest written by a previous `search --manifest`.
    #[arg(long = "from-manifest", value_name = "FILE")]
    pub(crate) from_manifest: PathBuf,

    /// Destination directory.
    #[arg(long = "to", value_name = "DIR")]
    pub(crate) to: PathBuf,

    /// Refuse if the manifest's total size exceeds this (e.g. 200MB, 1G).
    /// Defaults to 1G as an accident guard; raise it to export more.
    #[arg(long = "max-size", value_name = "SIZE", value_parser = parse_size, default_value = "1G")]
    pub(crate) max_size: u64,

    /// Hash the archive (SHA-256) before and after the run and report whether it
    /// changed — a slower, court-defensible integrity attestation.
    #[arg(long = "verify")]
    pub(crate) verify: bool,
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

#[cfg(test)]
mod tests {
    use super::parse_size;

    #[test]
    fn parse_size_handles_units() {
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert_eq!(parse_size("1k").unwrap(), 1024);
        assert_eq!(parse_size("2MB").unwrap(), 2 * 1024 * 1024);
        assert_eq!(parse_size("1G").unwrap(), 1024 * 1024 * 1024);
        assert!(parse_size("nope").is_err());
    }
}
