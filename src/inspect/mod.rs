//! In-file deep search: format detection and dispatch (`--inspect`).
//!
//! Defines: `inspect`, which detects a matching file's format and produces
//! format-specific `Inspection` context for a match at a given byte offset.
//! Used by: `engine.rs` (when `--inspect` is set).
//! Uses: `crate::models::Inspection` and the per-format submodules.
//!
//! Why a central detect+dispatch with one module per format: each format's
//! parser stays self-contained and testable, and adding a format is a new
//! submodule plus one match arm. Unsupported/undetected files return `None`,
//! so the normal match record is emitted unchanged.
//!
//! Currently implemented: TXT, JSON, XML, SQLite, CSV, plist (XML + binary).
//! ABX and SEGB are planned and will slot in here as they land.

use crate::models::Inspection;

mod csv;
mod json;
mod plist;
mod sqlite;
mod txt;
mod xml;

/// Formats the inspector can describe.
enum Format {
    Txt,
    Json,
    Xml,
    Sqlite,
    Csv,
    Plist,
}

/// Produce inspection context for a match at `offset` in `content`.
///
/// Returns `None` when the format is unsupported or undetected — the caller
/// then emits the plain match record.
pub fn inspect(name: &str, content: &[u8], offset: usize) -> Option<Inspection> {
    match detect(name, content)? {
        Format::Txt => txt::inspect(content, offset),
        Format::Json => json::inspect(content, offset),
        Format::Xml => xml::inspect(content, offset),
        Format::Sqlite => sqlite::inspect(content, offset),
        Format::Csv => csv::inspect(content, offset),
        Format::Plist => plist::inspect(content, offset),
    }
}

/// 1-based line number of `offset` within `content` (newline count + 1).
///
/// Shared by the text-based inspectors (JSON, XML, plist-XML) so their summaries
/// can cite a line, like the TXT inspector.
pub(crate) fn line_at(content: &[u8], offset: usize) -> usize {
    let end = offset.min(content.len());
    1 + content[..end].iter().filter(|&&b| b == b'\n').count()
}

/// Detect a file's format, preferring the content's header/magic over the file
/// name extension.
///
/// WHY header-first: forensic file names are often wrong or absent (renamed,
/// carved, or stripped during acquisition), so a reliable in-content signature
/// is trusted over the extension; the extension is only the fallback when the
/// header is inconclusive (formats like JSON/CSV/TXT have no magic bytes).
fn detect(name: &str, content: &[u8]) -> Option<Format> {
    detect_by_magic(content).or_else(|| detect_by_extension(name))
}

/// Recognise formats that carry a distinctive header signature.
fn detect_by_magic(content: &[u8]) -> Option<Format> {
    if content.starts_with(b"SQLite format 3\x00") {
        return Some(Format::Sqlite);
    }
    if content.starts_with(b"bplist00") {
        return Some(Format::Plist);
    }
    // An XML declaration may follow a UTF-8 BOM.
    let body = content.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(content);
    if body.starts_with(b"<?xml") {
        // Distinguish an Apple plist (resolved by dict-key path) from generic
        // XML (resolved by element path) using a marker in the document head.
        let head = &body[..body.len().min(512)];
        if contains(head, b"<plist") || contains(head, b"DOCTYPE plist") {
            return Some(Format::Plist);
        }
        return Some(Format::Xml);
    }
    None
}

/// True if `haystack` contains `needle`.
fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}

/// Map a file name's extension (lowercased) to a format.
fn detect_by_extension(name: &str) -> Option<Format> {
    let basename = name.rsplit('/').next().unwrap_or(name);
    let ext = basename
        .rsplit_once('.')
        .map(|(_, e)| e.to_ascii_lowercase());
    match ext.as_deref() {
        Some("txt") | Some("log") | Some("text") => Some(Format::Txt),
        Some("json") => Some(Format::Json),
        Some("xml") => Some(Format::Xml),
        Some("plist") => Some(Format::Plist),
        Some("sqlite") | Some("sqlite3") | Some("db") | Some("sqlitedb") => Some(Format::Sqlite),
        Some("csv") => Some(Format::Csv),
        _ => None,
    }
}
