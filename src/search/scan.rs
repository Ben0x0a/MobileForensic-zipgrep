//! Match scanning: run a byte regex and build a readable line preview per match.
//!
//! Defines: `search_bytes` (one `SearchHit` per match) plus the private
//! `preview`/`line_bounds` helpers and the preview-size constants.
//! Used by: `search::search_entry`, `engine`.
//! Uses: `regex::bytes` (the engine), `crate::models::SearchHit`.
//!
//! Why `regex::bytes` rather than `regex` on `&str`: forensic data is arbitrary
//! bytes (mixed encodings, binary blobs), so matching must happen on `&[u8]` —
//! decoding to UTF-8 first would either fail or corrupt the data. Raw line bytes
//! are handed back so the caller can render (and colourise) them.

use std::ops::Range;

use regex::bytes::Regex;

use crate::models::SearchHit;

/// Maximum preview length (bytes) of the line shown for a match.
///
/// Binary files (the whole reason we search bytes) have almost no newlines, so a
/// "line" can span the entire file. Longer lines are truncated to a window
/// around the match so output stays readable and memory stays bounded. The
/// reported byte offsets are exact regardless — this caps only the preview text.
const MAX_PREVIEW: usize = 200;

/// Unicode horizontal ellipsis (U+2026) marking a truncated line edge.
const ELLIPSIS: &[u8] = "…".as_bytes();

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
