//! Result formatting and writing (txt / json / csv).
//!
//! Defines: `OutputFormat`, `write_results` (one line/object per match), and
//! `write_counts` (one line per file, for `--count`), rendering to any `Write`
//! sink in the chosen format.
//! Used by: `main.rs` (picks the format from `--format`, the sink from `-o`).
//!
//! Output rules: at most one line per match, and binary file content is never
//! raw-dumped. The matched line is shown only when it looks textual (see
//! `is_textual`); offsets in txt are hex (`0x…`). Richer per-format context is
//! opt-in via `--inspect` and appears as a labelled tag (txt) or `context`
//! (json/csv).
//! Uses: `crate::models::MatchRecord`, `serde`/`serde_json` (JSON), `csv` (CSV),
//! `anyhow` (errors).
//!
//! Why the format enum lives here and parses via `FromStr` (not clap's
//! `ValueEnum`): it keeps the library free of any CLI-framework dependency, so
//! the core can be reused without pulling in clap. `main.rs` wires it to the
//! flag.

use std::borrow::Cow;
use std::io::Write;
use std::str::FromStr;

use anyhow::{Context, Result};
use serde::Serialize;

use crate::models::MatchRecord;

// ANSI escapes for match highlighting: bold red on, all attributes off. Bytes
// are plain ASCII, so they splice into raw line bytes safely.
const COLOUR_ON: &[u8] = b"\x1b[1;31m";
const COLOUR_OFF: &[u8] = b"\x1b[0m";

/// Output format for results.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputFormat {
    Txt,
    Json,
    Csv,
}

impl FromStr for OutputFormat {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "txt" | "text" => Ok(Self::Txt),
            "json" => Ok(Self::Json),
            "csv" => Ok(Self::Csv),
            other => Err(format!(
                "unknown format '{other}' (expected txt, json, or csv)"
            )),
        }
    }
}

/// Write all `records` to `w` in `format`.
///
/// `colourise` applies only to the txt format; JSON/CSV are machine-readable
/// and never receive ANSI escapes. `path_match` selects the `--match-path`
/// rendering, where each record is a file matched by its path: txt then prints
/// just the (highlighted) path, with no in-file offset or duplicated line.
pub fn write_results(
    records: &[MatchRecord],
    format: OutputFormat,
    colourise: bool,
    path_match: bool,
    w: &mut dyn Write,
) -> Result<()> {
    match format {
        OutputFormat::Txt => write_txt(records, colourise, path_match, w),
        OutputFormat::Json => write_json(records, w),
        OutputFormat::Csv => write_csv(records, w),
    }
}

// The matched line is shown only when it looks textual; for binary files the
// exact bytes remain recoverable from the offsets (and via the export step), so a
// display-oriented format never raw-dumps binary content.

/// Heuristic: does this line look like text rather than binary?
///
/// Binary if it contains a NUL or any C0 control byte other than the usual
/// whitespace (tab, LF, VT, FF, CR). High bytes (>= 0x80) are allowed as
/// possible UTF-8. A single stray control byte marks the line binary — which is
/// what suppresses content for SQLite, bplist, and other binary files.
fn is_textual(bytes: &[u8]) -> bool {
    bytes
        .iter()
        .all(|&b| b >= 0x20 || matches!(b, b'\t' | b'\n' | 0x0b | 0x0c | b'\r'))
}

/// The matched line, lossily decoded, but only when textual (else `None`).
fn textual_line(r: &MatchRecord) -> Option<Cow<'_, str>> {
    is_textual(&r.line).then(|| String::from_utf8_lossy(&r.line))
}

/// JSON projection: `line` appears only for textual matches; `format`/`context`
/// only when the match was inspected (`--inspect`).
#[derive(Serialize)]
struct JsonView<'a> {
    /// Source archive, present only when several archives were searched.
    #[serde(skip_serializing_if = "Option::is_none")]
    archive: Option<&'a str>,
    path: &'a str,
    file_start: u64,
    file_offset: u64,
    archive_offset: u64,
    /// True for DEFLATE entries, where `archive_offset` is the blob start.
    compressed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    context: Option<&'a serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    line: Option<Cow<'a, str>>,
}

impl<'a> From<&'a MatchRecord> for JsonView<'a> {
    fn from(r: &'a MatchRecord) -> Self {
        Self {
            archive: r.archive.as_deref(),
            path: &r.path,
            file_start: r.file_start,
            file_offset: r.file_offset,
            archive_offset: r.archive_offset,
            compressed: r.compressed,
            format: r.inspection.as_ref().map(|i| i.format.as_str()),
            context: r.inspection.as_ref().map(|i| &i.detail),
            line: textual_line(r),
        }
    }
}

/// CSV projection: flat, fixed columns (so the column set never varies).
/// `format`/`context` are empty unless inspected; `line` is empty for binary.
#[derive(Serialize)]
struct CsvView<'a> {
    /// Source archive (empty unless several archives were searched).
    archive: &'a str,
    path: &'a str,
    file_start: u64,
    file_offset: u64,
    archive_offset: u64,
    compressed: bool,
    format: &'a str,
    context: &'a str,
    line: Cow<'a, str>,
}

impl<'a> From<&'a MatchRecord> for CsvView<'a> {
    fn from(r: &'a MatchRecord) -> Self {
        Self {
            archive: r.archive.as_deref().unwrap_or(""),
            path: &r.path,
            file_start: r.file_start,
            file_offset: r.file_offset,
            archive_offset: r.archive_offset,
            compressed: r.compressed,
            format: r.inspection.as_ref().map_or("", |i| i.format.as_str()),
            context: r.inspection.as_ref().map_or("", |i| i.summary.as_str()),
            line: textual_line(r).unwrap_or(Cow::Borrowed("")),
        }
    }
}

/// txt: one line per match — `path:0x<file_offset>` plus, for textual files, the
/// matched line, plus a labelled `[format summary]` tag when inspected.
///
/// The offset is hex (`0x…`) to match how analysts read a hex editor. Binary
/// files contribute only `path:0x<offset>` — their bytes are never dumped.
fn write_txt(
    records: &[MatchRecord],
    colourise: bool,
    path_match: bool,
    w: &mut dyn Write,
) -> Result<()> {
    for r in records {
        // --match-path: the "match" is the file's path itself, so print just the
        // path (the source archive joined like a folder), highlighting the part
        // the pattern matched. No in-file offset — there is no content position.
        if path_match {
            let out = match &r.archive {
                Some(a) => format!("{a}/{}", render_line(r, colourise)),
                None => render_line(r, colourise),
            };
            writeln!(w, "{out}").context("failed writing txt output")?;
            continue;
        }

        // The source archive (when set) joins the internal path like a folder,
        // so a match reads as `case.zip/internal/file:0x<off>`.
        let mut out = match &r.archive {
            Some(a) => format!("{a}/{}:0x{:x}", r.path, r.file_offset),
            None => format!("{}:0x{:x}", r.path, r.file_offset),
        };
        if is_textual(&r.line) {
            out.push(':');
            out.push_str(&render_line(r, colourise));
        }
        if let Some(i) = &r.inspection {
            out.push_str(&format!("  [{}  {}]", i.format, i.summary));
        }
        writeln!(w, "{out}").context("failed writing txt output")?;
    }
    Ok(())
}

/// Per-file match counts (for `--count`).
#[derive(Serialize)]
struct CountView<'a> {
    path: &'a str,
    count: usize,
}

/// Write one `path:count` per file (txt), or a structured equivalent (json/csv).
pub fn write_counts(
    counts: &[(&str, usize)],
    format: OutputFormat,
    w: &mut dyn Write,
) -> Result<()> {
    match format {
        OutputFormat::Txt => {
            for (path, count) in counts {
                writeln!(w, "{path}:{count}").context("failed writing count output")?;
            }
            Ok(())
        }
        OutputFormat::Json => {
            let views: Vec<CountView> = counts
                .iter()
                .map(|&(path, count)| CountView { path, count })
                .collect();
            serde_json::to_writer_pretty(&mut *w, &views).context("failed writing count output")?;
            writeln!(w).context("failed writing count output")?;
            Ok(())
        }
        OutputFormat::Csv => {
            let mut wtr = csv::Writer::from_writer(w);
            for &(path, count) in counts {
                wtr.serialize(CountView { path, count })
                    .context("failed writing count row")?;
            }
            wtr.flush().context("failed flushing count output")?;
            Ok(())
        }
    }
}

/// json: a single pretty-printed array of record objects (re-ingestable).
fn write_json(records: &[MatchRecord], w: &mut dyn Write) -> Result<()> {
    let views: Vec<JsonView> = records.iter().map(JsonView::from).collect();
    serde_json::to_writer_pretty(&mut *w, &views).context("failed writing JSON output")?;
    writeln!(w).context("failed writing JSON output")?;
    Ok(())
}

/// csv: header row plus one row per match.
fn write_csv(records: &[MatchRecord], w: &mut dyn Write) -> Result<()> {
    let mut wtr = csv::Writer::from_writer(w);
    for r in records {
        wtr.serialize(CsvView::from(r))
            .context("failed writing CSV row")?;
    }
    wtr.flush().context("failed flushing CSV output")?;
    Ok(())
}

/// Render a record's line for display, optionally wrapping the matched bytes in
/// colour escapes.
///
/// HOW: the escapes are spliced into the raw line bytes at the match boundaries
/// and the whole thing is lossily decoded once — decoding the three pieces
/// separately could introduce replacement characters at the seams.
fn render_line(r: &MatchRecord, colourise: bool) -> String {
    if !colourise {
        return String::from_utf8_lossy(&r.line).into_owned();
    }
    let m = &r.match_in_line;
    let mut buf = Vec::with_capacity(r.line.len() + COLOUR_ON.len() + COLOUR_OFF.len());
    buf.extend_from_slice(&r.line[..m.start]);
    buf.extend_from_slice(COLOUR_ON);
    buf.extend_from_slice(&r.line[m.start..m.end]);
    buf.extend_from_slice(COLOUR_OFF);
    buf.extend_from_slice(&r.line[m.end..]);
    String::from_utf8_lossy(&buf).into_owned()
}
