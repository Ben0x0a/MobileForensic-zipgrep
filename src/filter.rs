//! Entry filtering: decide which archive entries are worth searching.
//!
//! Defines: `EntryFilter`, combining include globs (`--path`), exclude globs
//! (`--not-path`), and a skip-media switch into one `selects(path)` predicate.
//! Used by: `engine` (skips entries before searching) and `main` (builds it from
//! the CLI flags).
//! Uses: only the standard library.
//!
//! Wildcards: `*` matches any run of characters *including* `/`, and `?` matches
//! exactly one character — the intuitive rule for forensic filtering (`*.db`
//! matches a `.db` file at any depth). Matching is case-sensitive.
//!
//! Media skip: phone acquisitions are dominated (often ~80%) by photos and
//! videos, which contain no searchable text, so media files are skipped by
//! default for speed; `--include-media` searches them anyway.

/// Which entries to search: include/exclude globs plus a media skip.
pub struct EntryFilter {
    /// `--path` globs; empty means "every entry" (subject to the rules below).
    include: Vec<String>,
    /// `--not-path` globs; an entry matching any of these is skipped.
    exclude: Vec<String>,
    /// Skip image/video/audio files (recognised by extension).
    skip_media: bool,
}

impl EntryFilter {
    /// Build a filter from the CLI flags.
    pub fn new(include: &[String], exclude: &[String], skip_media: bool) -> Self {
        Self {
            include: include.to_vec(),
            exclude: exclude.to_vec(),
            skip_media,
        }
    }

    /// A filter that selects every entry (used by tests and as a neutral base).
    pub fn all() -> Self {
        Self {
            include: Vec::new(),
            exclude: Vec::new(),
            skip_media: false,
        }
    }

    /// Whether `path` should be searched.
    ///
    /// An entry is searched when it matches an include glob (or none were
    /// given), matches no exclude glob, and — unless media is being searched —
    /// is not a media file.
    pub fn selects(&self, path: &str) -> bool {
        if !self.include.is_empty() && !self.include.iter().any(|g| matches(g, path)) {
            return false;
        }
        if self.exclude.iter().any(|g| matches(g, path)) {
            return false;
        }
        if self.skip_media && is_media(path) {
            return false;
        }
        true
    }
}

/// True if `path`'s extension is a known image/video/audio type.
fn is_media(path: &str) -> bool {
    let base = path.rsplit('/').next().unwrap_or(path);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase());
    matches!(
        ext.as_deref(),
        Some(
            // images
            "jpg" | "jpeg" | "png" | "gif" | "bmp" | "tif" | "tiff" | "heic" | "heif" | "webp"
            | "ico" | "dng" | "cr2" | "nef"
            // video
            | "mp4" | "mov" | "m4v" | "avi" | "mkv" | "webm" | "3gp" | "3g2" | "mpg" | "mpeg"
            | "wmv" | "flv"
            // audio
            | "mp3" | "m4a" | "aac" | "wav" | "flac" | "ogg" | "oga" | "opus" | "wma" | "amr"
            | "caf" | "aiff"
        )
    )
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

    #[test]
    fn all_selects_everything() {
        let f = EntryFilter::all();
        assert!(f.selects("any/path.txt"));
        assert!(f.selects("photo.jpg")); // media not skipped by `all`
    }

    #[test]
    fn include_restricts_to_matching() {
        let f = EntryFilter::new(&["*.db".into()], &[], false);
        assert!(f.selects("a/x.db"));
        assert!(!f.selects("a/x.txt"));
    }

    #[test]
    fn exclude_rejects_matching() {
        let f = EntryFilter::new(&[], &["*/Caches/*".into()], false);
        assert!(f.selects("a/Documents/x.db"));
        assert!(!f.selects("a/Caches/x.db"));
    }

    #[test]
    fn exclude_wins_over_include() {
        let f = EntryFilter::new(&["*.db".into()], &["*/Caches/*".into()], false);
        assert!(!f.selects("a/Caches/x.db"));
    }

    #[test]
    fn skip_media_drops_images_and_video() {
        let f = EntryFilter::new(&[], &[], true);
        assert!(!f.selects("DCIM/IMG_0001.JPG")); // case-insensitive extension
        assert!(!f.selects("clip.mp4"));
        assert!(!f.selects("song.mp3"));
        assert!(f.selects("notes.txt"));
        assert!(f.selects("db.sqlite"));
    }

    #[test]
    fn include_media_keeps_them() {
        let f = EntryFilter::new(&[], &[], false);
        assert!(f.selects("DCIM/IMG_0001.jpg"));
    }
}
