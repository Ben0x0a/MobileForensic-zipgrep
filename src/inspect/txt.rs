//! TXT inspector: map a byte offset to a line and column.
//!
//! Defines: `inspect`, returning the 1-based line and (byte) column of a match
//! within a text file.
//! Used by: `inspect::inspect` (dispatch).
//! Uses: `crate::models::Inspection`, `serde_json` (structured detail).
//!
//! Column is counted in bytes, not Unicode scalar values — forensic text may
//! not be valid UTF-8, and a byte column stays meaningful regardless.

use serde_json::json;

use crate::models::Inspection;

/// TXT inspector — plain text / log files (no header; detected by extension).
pub struct Txt;

impl super::Inspector for Txt {
    fn extensions(&self) -> &'static [&'static str] {
        &["txt", "log", "text"]
    }
    fn detect(&self, _content: &[u8]) -> bool {
        false // no signature
    }
    fn inspect(&self, content: &[u8], offset: usize) -> Option<Inspection> {
        resolve(content, offset)
    }
}

/// Report the 1-based line and byte-column of `offset` within `content`.
fn resolve(content: &[u8], offset: usize) -> Option<Inspection> {
    // An offset past the end means the caller passed a position from a
    // different buffer; refuse rather than report a bogus location.
    if offset > content.len() {
        return None;
    }

    let mut line = 1usize;
    let mut col = 1usize;
    for &b in &content[..offset] {
        if b == b'\n' {
            line += 1;
            col = 1;
        } else {
            col += 1;
        }
    }

    Some(Inspection {
        format: "txt".into(),
        summary: format!("line: {line}  col: {col}"),
        detail: json!({ "line": line, "col": col }),
    })
}
