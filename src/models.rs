//! Shared data containers for mf-zipgrep.
//!
//! Defines: `Method` (the supported compression methods), `Entry` (a located
//! file inside the archive), `SearchHit` (one regex match within an entry's
//! data) and `MatchRecord` (an archive-level match enriched with every offset,
//! ready for output).
//! Used by: `zip.rs` (produces `Entry`), `search.rs` (consumes `Entry`,
//! produces `SearchHit`), `main.rs`/`output.rs` (via `MatchRecord`).
//! Uses: only `std::ops::Range` — these are plain, dependency-free data types,
//! kept in their own module so the parser and the search engine can each depend
//! on the data shape without depending on each other.

use std::ops::Range;

use serde::{Deserialize, Serialize};

/// Run-level metadata: the tool, the query, and every filter in effect.
///
/// Recorded so machine-readable output and the export manifest/report are
/// self-describing — a result file states exactly which archives, pattern, and
/// filters produced it. Embedded in JSON output, the manifest, and the export
/// report; never shown in txt/csv (which stay line-oriented).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunInfo {
    /// Tool name (`mf-zipgrep`).
    pub tool: String,
    /// Tool version (`CARGO_PKG_VERSION`).
    pub version: String,
    /// The pattern as given on the command line.
    pub pattern: String,
    /// Pattern was treated as a literal string (`-l`), not a regex.
    pub literal: bool,
    /// Case-insensitive matching (`-i`).
    pub ignore_case: bool,
    /// Pattern was matched against file paths, not content (`--match-path`).
    pub match_path: bool,
    /// Deep inspection was on (`--inspect`).
    pub inspect: bool,
    /// Source archives, as full filesystem paths.
    pub archives: Vec<String>,
    /// `--path` include globs.
    pub path_globs: Vec<String>,
    /// `--not-path` exclude globs.
    pub not_path_globs: Vec<String>,
    /// `--type` format/category allowlist.
    pub types: Vec<String>,
    /// Media files were excluded (`--exclude-media` or `--fast`).
    pub exclude_media: bool,
}

/// Compression method of a searchable entry.
///
/// Only the two methods that appear in mobile-forensic archives are modelled;
/// any other method is dropped by the parser rather than represented here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Stored,
    Deflate,
}

/// One searchable file located inside the archive.
///
/// `data_offset`/`data_len` describe the entry's bytes as they sit in the
/// archive: the literal content for STORED, or the compressed stream for
/// DEFLATE. `uncompressed_size` is the logical file size (equal to `data_len`
/// for STORED).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub name: String,
    pub method: Method,
    pub data_offset: u64,
    pub data_len: u64,
    pub uncompressed_size: u64,
}

/// Format-specific context for a match, produced by an inspector (`--inspect`).
///
/// `summary` is a human one-liner (used in txt/csv); `detail` is a structured
/// object (used as the nested `context` in JSON). Keeping both lets each output
/// format show inspection at its natural fidelity.
#[derive(Debug, Clone, PartialEq)]
pub struct Inspection {
    /// Detected format, e.g. "txt", "json", "xml", "sqlite".
    pub format: String,
    /// Human-readable one-liner, e.g. `line 12, col 4` or `$.users[3].token`.
    pub summary: String,
    /// Structured detail for machine-readable output.
    pub detail: serde_json::Value,
}

/// A single regex match within an entry.
///
/// The line is kept as raw bytes (not a decoded `String`) so the caller can
/// splice in colour escapes at exact byte positions before lossily decoding —
/// decoding first would shift byte offsets and break match highlighting on
/// non-UTF-8 forensic data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchHit {
    /// Byte offset of the match start within the entry's (uncompressed) data.
    pub offset: u64,
    /// Raw bytes of the line containing the match (trailing `\r`/`\n` removed).
    pub line: Vec<u8>,
    /// Byte range of the match within `line`, for highlighting.
    pub match_in_line: Range<usize>,
}

/// An archive-level match, enriched with every offset the caller might want.
///
/// This is the unit handed to `output.rs`. The offsets answer "where":
/// - `file_start`: where the matching file's data begins in the archive,
/// - `file_offset`: the match position *within the file's logical content*,
/// - `archive_offset`: the match's absolute byte position in the archive.
///
/// For STORED entries `file_start + file_offset == archive_offset`. For DEFLATE
/// entries the match lives in the decompressed stream, which has no single
/// archive byte, so `archive_offset` is set to `file_start` (the compressed
/// blob's start) and `compressed` is `true` to flag that it is approximate.
///
/// `Eq` is intentionally not derived: `Inspection::detail` is a
/// `serde_json::Value`, which is only `PartialEq` (it may hold floats).
#[derive(Debug, Clone, PartialEq)]
pub struct MatchRecord {
    /// Source archive display label, set only when more than one archive is
    /// searched in a run (so single-archive txt/csv output is unchanged).
    pub archive: Option<String>,
    /// Full filesystem path of the source archive, set for every record. Used by
    /// JSON output so each result names its origin; txt/csv use `archive`.
    pub archive_path: Option<String>,
    /// File name and path inside the archive.
    pub path: String,
    pub file_start: u64,
    pub file_offset: u64,
    pub archive_offset: u64,
    /// True when the entry was DEFLATE — see `archive_offset` note above.
    pub compressed: bool,
    /// Raw bytes of the line containing the match (trailing `\r`/`\n` removed).
    pub line: Vec<u8>,
    /// Byte range of the match within `line`, for highlighting.
    pub match_in_line: Range<usize>,
    /// Format-specific context, present only when `--inspect` matched a format.
    pub inspection: Option<Inspection>,
}

impl MatchRecord {
    /// Assemble an archive-level record from an entry and one of its hits.
    ///
    /// Centralised here (rather than in `main`) so the STORED-vs-DEFLATE offset
    /// rule has a single, unit-tested home.
    pub fn new(entry: &Entry, hit: SearchHit) -> Self {
        let compressed = entry.method == Method::Deflate;
        // STORED: exact archive byte. DEFLATE: no exact byte exists, so point at
        // the compressed blob's start and let `compressed` flag the caveat.
        let archive_offset = if compressed {
            entry.data_offset
        } else {
            entry.data_offset + hit.offset
        };
        Self {
            archive: None,
            archive_path: None,
            path: entry.name.clone(),
            file_start: entry.data_offset,
            file_offset: hit.offset,
            archive_offset,
            compressed,
            line: hit.line,
            match_in_line: hit.match_in_line,
            inspection: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hit() -> SearchHit {
        SearchHit {
            offset: 17,
            line: b"x".to_vec(),
            match_in_line: 0..1,
        }
    }

    #[test]
    fn stored_archive_offset_is_exact() {
        let entry = Entry {
            name: "a".into(),
            method: Method::Stored,
            data_offset: 100,
            data_len: 50,
            uncompressed_size: 50,
        };
        let rec = MatchRecord::new(&entry, hit());
        assert_eq!(rec.archive_offset, 117);
        assert!(!rec.compressed);
    }

    #[test]
    fn deflate_archive_offset_falls_back_to_file_start() {
        let entry = Entry {
            name: "a".into(),
            method: Method::Deflate,
            data_offset: 100,
            data_len: 20,
            uncompressed_size: 50,
        };
        let rec = MatchRecord::new(&entry, hit());
        assert_eq!(rec.archive_offset, 100); // blob start, not 117
        assert_eq!(rec.file_offset, 17); // still the decompressed position
        assert!(rec.compressed);
    }
}
