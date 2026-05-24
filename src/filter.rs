//! Entry filtering: decide which archive entries are worth searching.
//!
//! Defines: `EntryFilter`, which combines include globs (`--path`), exclude
//! globs (`--not-path`), a file-type allowlist (`--type`), and the media skip.
//! It splits into two predicates: `selects(path)` (path-only, applied before
//! reading any bytes) and `accepts_type(TypeInfo)` (applied after detecting the
//! format from the content header).
//! Used by: `engine` (skips entries before/while searching) and `main` (builds
//! it from the CLI flags).
//! Uses: `crate::inspect::TypeInfo` (the detected format/category).
//!
//! Wildcards: `*` matches any run of characters *including* `/`, and `?` matches
//! exactly one character — the intuitive rule for forensic filtering (`*.db`
//! matches a `.db` file at any depth). Matching is case-sensitive.
//!
//! Media skip: phone acquisitions are dominated (often ~80%) by photos and
//! videos, which contain no searchable text, so files whose detected category is
//! `media` are skipped by default for speed; `--include-media` searches them.
//! The skip is just the `--type` machinery applied to the `media` category, so
//! detection lives in one place (the inspectors), not a duplicated extension list.

use crate::inspect::TypeInfo;

/// Which entries to search: path include/exclude globs, a `--type` allowlist,
/// and the media skip.
pub struct EntryFilter {
    /// `--path` globs; empty means "every entry" (subject to the rules below).
    include: Vec<String>,
    /// `--not-path` globs; an entry matching any of these is skipped.
    exclude: Vec<String>,
    /// `--type` values (format names or categories); empty means "any type".
    types: Vec<String>,
    /// Skip files whose detected category is `media` (when no `--type` is set).
    skip_media: bool,
}

impl EntryFilter {
    /// Build a filter from the CLI flags.
    pub fn new(include: &[String], exclude: &[String], types: &[String], skip_media: bool) -> Self {
        Self {
            include: include.to_vec(),
            exclude: exclude.to_vec(),
            types: types.to_vec(),
            skip_media,
        }
    }

    /// A filter that selects every entry (used by tests and as a neutral base).
    pub fn all() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            types: Vec::new(),
            skip_media: false,
        }
    }

    /// Whether `path` passes the path-only filters (include/exclude globs).
    ///
    /// This runs before any content is read; the type/media decision is made
    /// separately by [`accepts_type`](Self::accepts_type) once the header has
    /// been inspected.
    pub fn selects(&self, path: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|g| matches(g, path)) {
            return false;
        }
        if self.exclude.iter().any(|g| matches(g, path)) {
            return false;
        }
        true
    }

    /// Whether an entry of the detected type should be searched.
    ///
    /// `info` is the format/category from `inspect::detect_type` (header-first,
    /// then extension), or `None` when no inspector claims the file.
    ///
    /// - With `--type`: keep only files whose format name **or** category is in
    ///   the allowlist (an unrecognised file, `None`, is excluded). The explicit
    ///   allowlist takes over, so the media skip does not also apply.
    /// - Without `--type`: keep everything, except — when `skip_media` is set —
    ///   files whose category is `media`.
    pub fn accepts_type(&self, info: Option<TypeInfo>) -> bool {
        if !self.types.is_empty() {
            return info.is_some_and(|i| {
                self.types
                    .iter()
                    .any(|t| t.as_str() == i.name || t.as_str() == i.category)
            });
        }
        if self.skip_media && info.is_some_and(|i| i.category == "media") {
            return false;
        }
        true
    }
}

/// True if `path` matches the `*`/`?` wildcard `pattern`.
fn matches(pattern: &str, path: &str) -> bool {
    wildcard_match(pattern.as_bytes(), path.as_bytes())
}

/// Match `text` against a `*`/`?` wildcard `pattern`.
///
/// HOW: a linear two-pointer scan with backtracking. `star`/`mark` remember the
/// most recent `*` and how far `text` had advanced, so when a later literal
/// fails we let that `*` swallow one more character and retry.
fn wildcard_match(pattern: &[u8], text: &[u8]) -> bool {
    let (mut p, mut t) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;

    while t < text.len() {
        if p < pattern.len() && (pattern[p] == b'?' || pattern[p] == text[t]) {
            p += 1;
            t += 1;
        } else if p < pattern.len() && pattern[p] == b'*' {
            star = Some(p);
            mark = t;
            p += 1;
        } else if let Some(star_pos) = star {
            p = star_pos + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }

    while p < pattern.len() && pattern[p] == b'*' {
        p += 1;
    }
    p == pattern.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn star_crosses_separators() {
        assert!(wildcard_match(b"*.db", b"a/b/c.db"));
        assert!(wildcard_match(b"*Library*", b"x/Library/y"));
        assert!(!wildcard_match(b"*.db", b"c.txt"));
    }

    #[test]
    fn question_matches_one_char() {
        assert!(wildcard_match(b"?at", b"cat"));
        assert!(!wildcard_match(b"?at", b"at"));
    }

    fn media() -> Option<TypeInfo> {
        Some(TypeInfo {
            name: "jpeg",
            category: "media",
        })
    }
    fn sqlite() -> Option<TypeInfo> {
        Some(TypeInfo {
            name: "sqlite",
            category: "database",
        })
    }

    #[test]
    fn all_selects_everything() {
        let f = EntryFilter::all();
        assert!(f.selects("any/path.txt"));
        assert!(f.selects("photo.jpg")); // selects() is path-only
        assert!(f.accepts_type(media())); // and `all` keeps media too
    }

    #[test]
    fn include_restricts_to_matching() {
        let f = EntryFilter::new(&["*.db".into()], &[], &[], false);
        assert!(f.selects("a/x.db"));
        assert!(!f.selects("a/x.txt"));
    }

    #[test]
    fn exclude_rejects_matching() {
        let f = EntryFilter::new(&[], &["*/Caches/*".into()], &[], false);
        assert!(f.selects("a/Documents/x.db"));
        assert!(!f.selects("a/Caches/x.db"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = EntryFilter::new(&["*.db".into()], &["*/Caches/*".into()], &[], false);
        assert!(!f.selects("a/Caches/x.db"));
    }

    #[test]
    fn skip_media_drops_media_category() {
        let f = EntryFilter::new(&[], &[], &[], true);
        assert!(!f.accepts_type(media())); // image/video/audio dropped
        assert!(f.accepts_type(sqlite())); // non-media kept
        assert!(f.accepts_type(None)); // unrecognised type still searched
    }

    #[test]
    fn include_media_keeps_media() {
        let f = EntryFilter::new(&[], &[], &[], false); // skip_media off
        assert!(f.accepts_type(media()));
    }

    #[test]
    fn type_allowlist_matches_name_or_category() {
        let by_name = EntryFilter::new(&[], &[], &["sqlite".into()], true);
        assert!(by_name.accepts_type(sqlite()));
        assert!(!by_name.accepts_type(media()));
        assert!(!by_name.accepts_type(None));

        // A category value selects the whole family; it also overrides the media
        // skip, so `--type media` keeps media even though skip_media is set.
        let by_category = EntryFilter::new(&[], &[], &["media".into()], true);
        assert!(by_category.accepts_type(media()));
        assert!(!by_category.accepts_type(sqlite()));
    }
}
