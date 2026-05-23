//! Byte-level regex search over an entry's data (STORED or DEFLATE).
//!
//! Defines: `search_entry`, which runs a compiled byte regex against one
//! entry's logical content and reports each match together with its enclosing
//! line and the match's position within that line.
//! Used by: `main.rs` (orchestration).
//! Uses: `crate::models::{Entry, Method, SearchHit}` (data shapes),
//! `regex::bytes` (the search engine), `flate2` (DEFLATE decompression),
//! `anyhow` (errors).
//!
//! Why `regex::bytes` rather than `regex` on `&str`: forensic data is arbitrary
//! bytes (mixed encodings, binary blobs), so matching must happen on `&[u8]` —
//! decoding to UTF-8 first would either fail or corrupt the data. For the same
//! reason, this module hands back raw line bytes and lets the caller render.
//!
//! STORED entries are searched in place over the memory-mapped archive (no
//! copy); DEFLATE entries are decompressed into an owned buffer first, so their
//! reported offsets are positions within the *decompressed* stream.

use std::borrow::Cow;
use std::io::Read;
use std::ops::Range;

use anyhow::{Context, Result};
use flate2::read::DeflateDecoder;
use regex::bytes::Regex;

use crate::models::{Entry, Method, SearchHit};

/// Maximum preview length (bytes) of the line shown for a match.
///
/// Binary files (the whole reason we search bytes) have almost no newlines, so a
/// "line" can span the entire file. Longer lines are truncated to a window
/// around the match so output stays readable and memory stays bounded. The
/// reported byte offsets are exact regardless — this caps only the preview text.
const MAX_PREVIEW: usize = 200;

/// Unicode horizontal ellipsis (U+2026) marking a truncated line edge.
const ELLIPSIS: &[u8] = "…".as_bytes();

/// Return an entry's logical content: a borrowed slice of the archive for
/// STORED, or an owned decompressed buffer for DEFLATE.
///
/// Exposed so callers (the engine) can search *and* inspect the same content
/// without decompressing a DEFLATE entry twice.
pub fn entry_content<'a>(archive: &'a [u8], entry: &Entry) -> Result<Cow<'a, [u8]>> {
    let start = entry.data_offset as usize;
    let end = start + entry.data_len as usize;
    // A data range outside the file means the Central Directory disagreed with
    // the archive's real size; bailing here surfaces a corrupt/truncated image
    // rather than silently searching the wrong bytes.
    let raw = archive
        .get(start..end)
        .with_context(|| format!("data range of {} is out of bounds", entry.name))?;

    match entry.method {
        Method::Stored => Ok(Cow::Borrowed(raw)),
        Method::Deflate => {
            let inflated = inflate(raw, entry.uncompressed_size)
                .with_context(|| format!("failed to inflate {}", entry.name))?;
            Ok(Cow::Owned(inflated))
        }
    }
}

/// Search one entry for every (non-overlapping) match of `re`.
pub fn search_entry(archive: &[u8], entry: &Entry, re: &Regex) -> Result<Vec<SearchHit>> {
    let content = entry_content(archive, entry)?;
    Ok(search_bytes(&content, re))
}

/// Inflate a raw DEFLATE stream (ZIP method 8 stores no zlib header).
///
/// `expected_size` is only a capacity hint to avoid reallocations; the decoder
/// reads to the end of the stream regardless.
fn inflate(compressed: &[u8], expected_size: u64) -> Result<Vec<u8>> {
    let mut decoder = DeflateDecoder::new(compressed);
    let mut out = Vec::with_capacity(expected_size as usize);
    decoder.read_to_end(&mut out)?;
    Ok(out)
}

/// Run the regex over a haystack, producing one hit per match.
pub fn search_bytes(haystack: &[u8], re: &Regex) -> Vec<SearchHit> {
    let mut hits = Vec::new();
    for m in re.find_iter(haystack) {
        let (line_start, line_end) = line_bounds(haystack, m.start(), m.end());
        let (line, match_in_line) = preview(haystack, line_start, line_end, m.start(), m.end());
        hits.push(SearchHit {
            offset: m.start() as u64,
            line,
            match_in_line,
        });
    }
    hits
}

/// Build the line shown for a match: the whole line when short, otherwise a
/// `MAX_PREVIEW`-byte window centred on the match with `…` edge markers.
///
/// Returns the preview bytes and the match's byte range within them.
fn preview(
    haystack: &[u8],
    line_start: usize,
    line_end: usize,
    match_start: usize,
    match_end: usize,
) -> (Vec<u8>, Range<usize>) {
    // Window bounds within the haystack.
    let (win_start, win_end) = if line_end - line_start <= MAX_PREVIEW {
        (line_start, line_end)
    } else {
        // Keep a little context before the match, then a fixed-width window.
        let lead = (MAX_PREVIEW / 4).min(match_start - line_start);
        let start = match_start - lead;
        (start, (start + MAX_PREVIEW).min(line_end))
    };

    let truncated_left = win_start > line_start;
    let truncated_right = win_end < line_end;

    let mut window = haystack[win_start..win_end].to_vec();
    let mut match_lo = match_start - win_start;
    let mut match_hi = (match_end - win_start).min(window.len());

    // Strip a trailing CR only when this is the real end of the line, so CRLF
    // files display cleanly without dropping a byte mid-window.
    if !truncated_right && window.last() == Some(&b'\r') {
        window.pop();
        match_lo = match_lo.min(window.len());
        match_hi = match_hi.min(window.len());
    }

    // Assemble with ellipsis markers, shifting the match range past any prefix.
    let mut line = Vec::with_capacity(window.len() + 2 * ELLIPSIS.len());
    if truncated_left {
        line.extend_from_slice(ELLIPSIS);
    }
    let shift = line.len();
    line.extend_from_slice(&window);
    if truncated_right {
        line.extend_from_slice(ELLIPSIS);
    }

    (line, (match_lo + shift)..(match_hi + shift))
}

/// Expand a match span outward to the enclosing line (newline-delimited).
///
/// HOW: walk left from the match start to the previous `\n` (exclusive) and
/// right from the match end to the next `\n`, clamping to the haystack bounds.
/// Returns `[line_start, line_end)`, mirroring grep's notion of "the line that
/// contains the match".
fn line_bounds(haystack: &[u8], match_start: usize, match_end: usize) -> (usize, usize) {
    let line_start = haystack[..match_start]
        .iter()
        .rposition(|&b| b == b'\n')
        .map_or(0, |i| i + 1);
    let line_end = haystack[match_end..]
        .iter()
        .position(|&b| b == b'\n')
        .map_or(haystack.len(), |i| match_end + i);
    (line_start, line_end)
}
