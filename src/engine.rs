//! Search orchestration: archive → findings (match records + matched files).
//!
//! Defines: `Findings`, `MatchedFile`, and `search_archive`, which parses an
//! archive's entries and searches them in parallel, returning both the
//! per-match records (for output) and the de-duplicated matched files (for the
//! pull step).
//! Used by: `main.rs` (the binary) and `tests/engine.rs`.
//! Uses: `crate::zip` (parse), `crate::search` (content + per-entry search),
//! `crate::inspect` (deep search), `crate::models`, `rayon` (parallelism),
//! `regex::bytes`.
//!
//! Why this lives in the library rather than `main`: it lets the end-to-end
//! search be unit-tested directly, and keeps `main` to CLI parsing and I/O.
//! Thread-pool sizing is the caller's concern (set before calling); here we
//! just use rayon's current global pool. Both outputs come from a single search
//! pass, so printing and pulling never re-scan the archive.

use anyhow::Result;
use rayon::prelude::*;
use regex::bytes::Regex;

use crate::filter::PathFilter;
use crate::models::{Entry, MatchRecord};
use crate::{inspect, search, zip};

/// A file that contained at least one match, with the offsets of those matches.
///
/// Carries the full `Entry` so the pull step can re-read (and decompress) the
/// file's content without re-parsing the archive.
pub struct MatchedFile {
    pub entry: Entry,
    pub offsets: Vec<u64>,
}

/// The result of searching an archive.
pub struct Findings {
    /// One record per match, in entry-then-match order (for output).
    pub records: Vec<MatchRecord>,
    /// One entry per matched file, de-duplicated (for the pull step).
    pub files: Vec<MatchedFile>,
}

/// Reports scan progress. Implemented by the UI; the engine only calls it.
///
/// Kept UI-free (no I/O, no formatting) so the library has no terminal
/// dependency — `main` provides the on-screen reporter.
pub trait Progress: Sync {
    /// Called once with the number of entries that will be searched.
    fn set_total(&self, total: usize);
    /// Called once per entry after it has been searched.
    fn inc(&self);
}

/// A `Progress` that does nothing — the default for tests and piped output.
pub struct NoProgress;

impl Progress for NoProgress {
    fn set_total(&self, _total: usize) {}
    fn inc(&self) {}
}

/// Parse `archive` and return all matches of `re`, in entry order.
///
/// `filter` restricts which entries are searched by internal path; an empty
/// filter searches everything. When `deep` is set, each match is annotated with
/// format-specific context where the file's format is recognised (see
/// [`crate::inspect`]).
pub fn search_archive(
    archive: &[u8],
    re: &Regex,
    deep: bool,
    filter: &PathFilter,
) -> Result<Findings> {
    search_with_progress(archive, re, deep, filter, &NoProgress)
}

/// Like [`search_archive`], but reports scan progress via `progress`.
///
/// HOW: the entries to search are selected first (so the total is known up
/// front), then searched in parallel; rayon's `collect` into a `Vec` preserves
/// input order, so both outputs stay deterministic (entry order, then match
/// order within each entry). Each entry's content is obtained once and reused
/// for searching, inspection, and the matched-file list.
pub fn search_with_progress(
    archive: &[u8],
    re: &Regex,
    deep: bool,
    filter: &PathFilter,
    progress: &dyn Progress,
) -> Result<Findings> {
    let entries = zip::parse_entries(archive)?;
    let targets: Vec<&Entry> = entries.iter().filter(|e| filter.matches(&e.name)).collect();
    progress.set_total(targets.len());

    let per_entry = targets
        .par_iter()
        .map(
            |&entry| -> Result<(Vec<MatchRecord>, Option<MatchedFile>)> {
                let content = search::entry_content(archive, entry)?;
                let hits = search::search_bytes(&content, re);

                let result = if hits.is_empty() {
                    (Vec::new(), None)
                } else {
                    let offsets = hits.iter().map(|hit| hit.offset).collect();
                    let records = hits
                        .into_iter()
                        .map(|hit| {
                            let mut record = MatchRecord::new(entry, hit);
                            if deep {
                                record.inspection = inspect::inspect(
                                    &entry.name,
                                    &content,
                                    record.file_offset as usize,
                                );
                            }
                            record
                        })
                        .collect();
                    let file = MatchedFile {
                        entry: entry.clone(),
                        offsets,
                    };
                    (records, Some(file))
                };

                progress.inc();
                Ok(result)
            },
        )
        .collect::<Result<Vec<_>>>()?;

    let mut records = Vec::new();
    let mut files = Vec::new();
    for (entry_records, matched_file) in per_entry {
        records.extend(entry_records);
        if let Some(file) = matched_file {
            files.push(file);
        }
    }

    Ok(Findings { records, files })
}
