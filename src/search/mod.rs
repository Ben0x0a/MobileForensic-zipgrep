//! Per-entry byte search: obtain an entry's content, then scan it for a regex.
//!
//! Defines: the search module surface — [`entry_content`] (borrow/decompress an
//! entry's bytes), [`search_bytes`] (run the regex, with line previews), and
//! [`search_entry`] (the two combined for one entry).
//! Used by: `engine` (orchestration), `export` (content), and the tests.
//! Uses: `content` (STORED/DEFLATE access) and `scan` (regex + preview).
//!
//! Why split: content acquisition (an mmap borrow vs DEFLATE inflate) and match
//! scanning (regex, line bounds, preview windowing) are independent concerns, so
//! each lives in its own file while this module ties them together.

mod content;
mod scan;

pub use content::entry_content;
pub use scan::search_bytes;

use anyhow::Result;
use regex::bytes::Regex;

use crate::models::{Entry, SearchHit};

/// Search one entry for every (non-overlapping) match of `re`.
pub fn search_entry(archive: &[u8], entry: &Entry, re: &Regex) -> Result<Vec<SearchHit>> {
    let content = entry_content(archive, entry)?;
    Ok(search_bytes(&content, re))
}
