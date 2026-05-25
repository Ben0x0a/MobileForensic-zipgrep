//! Entry content acquisition: borrow STORED bytes, inflate DEFLATE.
//!
//! Defines: `entry_content`, returning an entry's logical content as a borrowed
//! mmap slice (STORED) or an owned, decompressed buffer (DEFLATE), plus the
//! private `inflate` helper.
//! Used by: `search::search_entry`, `engine`, and `export` (so a DEFLATE entry
//! is decompressed once and reused for searching, inspection, and export).
//! Uses: `flate2` (raw DEFLATE), `crate::models::{Entry, Method}`, `anyhow`.
//!
//! STORED entries are returned in place over the memory-mapped archive (no copy);
//! DEFLATE entries are decompressed into an owned buffer, so reported offsets are
//! positions within the *decompressed* stream.

use std::borrow::Cow;
use std::io::Read;

use anyhow::{Context, Result};
use flate2::read::DeflateDecoder;

use crate::models::{Entry, Method};

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
