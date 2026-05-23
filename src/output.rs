//! Result formatting and writing (txt / json / csv).
//!
//! Defines: `OutputFormat` and `write_results`, which render a slice of
//! `MatchRecord`s to any `Write` sink in the chosen format.
//! Used by: `main.rs` (picks the format from `--format`, the sink from `-o`).
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
/// and never receive ANSI escapes.
pub fn write_results(
    records: &[MatchRecord],
    format: OutputFormat,
    colourise: bool,
    w: &mut dyn Write,
) -> Result<()> {
    match format {
        OutputFormat::Txt => write_txt(records, colourise, w),
        OutputFormat::Json => write_json(records, w),
        OutputFormat::Csv => write_csv(records, w),
    }
}

// The line is lossily decoded to text in the views below; the exact bytes
// remain recoverable from the offsets (and, later, via the pull step), so a
// display-oriented format does not need to preserve them verbatim.

/// JSON projection: inspection appears as a nested `context` object plus a
/// `format` tag, both omitted when the match was not inspected.
#[derive(Serialize)]
struct JsonView<'a> {
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
    line: Cow<'a, str>,
}

impl<'a> From<&'a MatchRecord> for JsonView<'a> {
    fn from(r: &'a MatchRecord) -> Self {
        Self {
            path: &r.path,
            file_start: r.file_start,
            file_offset: r.file_offset,
            archive_offset: r.archive_offset,
            compressed: r.compressed,
            format: r.inspection.as_ref().map(|i| i.format.as_str()),
            context: r.inspection.as_ref().map(|i| &i.detail),
            line: String::from_utf8_lossy(&r.line),
        }
    }
}

/// CSV projection: flat, fixed columns. Inspection collapses to a `format` tag
/// and a human `context` summary, both empty strings when not inspected (so the
/// column set never varies between rows).
#[derive(Serialize)]
struct CsvView<'a> {
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
            path: &r.path,
            file_start: r.file_start,
            file_offset: r.file_offset,
            archive_offset: r.archive_offset,
            compressed: r.compressed,
            format: r.inspection.as_ref().map_or("", |i| i.format.as_str()),
            context: r.inspection.as_ref().map_or("", |i| i.summary.as_str()),
            line: String::from_utf8_lossy(&r.line),
        }
    }
}

/// txt: one line per match — `path:file_offset:archive_offset:line`.
///
/// For DEFLATE entries the archive offset is prefixed with `~` to flag that it
/// is the compressed blob's start, not the exact match byte (which has no
/// single archive position). When the match was inspected, a `[format summary]`
/// tag is appended.
fn write_txt(records: &[MatchRecord], colourise: bool, w: &mut dyn Write) -> Result<()> {
    for r in records {
        let archive_offset = if r.compressed {
            format!("~{}", r.archive_offset)
        } else {
            r.archive_offset.to_string()
        };
        let mut line = render_line(r, colourise);
        if let Some(i) = &r.inspection {
            line = format!("{line}  [{} {}]", i.format, i.summary);
        }
        writeln!(
            w,
            "{}:{}:{}:{}",
            r.path, r.file_offset, archive_offset, line
        )
        .context("failed writing txt output")?;
    }
    Ok(())
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
