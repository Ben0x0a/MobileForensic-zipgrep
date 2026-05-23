//! Path filtering with shell-style wildcards.
//!
//! Defines: `PathFilter`, which decides whether an entry's internal path should
//! be searched, based on zero or more wildcard patterns.
//! Used by: `engine` (skips non-matching entries) and `main` (builds it from
//! `--path`).
//! Uses: only the standard library.
//!
//! Wildcards: `*` matches any run of characters *including* `/`, and `?` matches
//! exactly one character. This "everything including separators" rule is the
//! intuitive one for forensic filtering — `*.db` matches a `.db` file at any
//! depth, and `*Library*` matches any path containing `Library`. Matching is
//! case-sensitive (paths are compared exactly).

/// A set of wildcard patterns; an empty set matches everything.
pub struct PathFilter {
    patterns: Vec<String>,
}

impl PathFilter {
    /// Build a filter from wildcard patterns (e.g. from repeated `--path`).
    pub fn new(patterns: &[String]) -> Self {
        Self {
            patterns: patterns.to_vec(),
        }
    }

    /// Whether no patterns were supplied (so every entry is searched).
    pub fn is_empty(&self) -> bool {
        self.patterns.is_empty()
    }

    /// True if `path` should be searched: always when no patterns are set,
    /// otherwise when it matches at least one pattern.
    pub fn matches(&self, path: &str) -> bool {
        self.patterns.is_empty()
            || self
                .patterns
                .iter()
                .any(|p| wildcard_match(p.as_bytes(), path.as_bytes()))
    }
}

/// Match `text` against a `*`/`?` wildcard `pattern`.
///
/// HOW: a linear two-pointer scan with backtracking. `star`/`mark` remember the
/// most recent `*` and how far `text` had advanced, so that when a later literal
/// fails we can let that `*` swallow one more character and retry — O(n·m) worst
/// case but linear in practice.
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
            // Backtrack: let the last `*` consume one more character of `text`.
            p = star_pos + 1;
            mark += 1;
            t = mark;
        } else {
            return false;
        }
    }

    // Any trailing pattern must be only `*` to match the empty remainder.
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
    fn literal_and_full_match() {
        assert!(wildcard_match(b"a/b", b"a/b"));
        assert!(!wildcard_match(b"a/b", b"a/c"));
        assert!(wildcard_match(b"*", b"anything/at/all"));
    }

    #[test]
    fn empty_filter_matches_all() {
        let f = PathFilter::new(&[]);
        assert!(f.is_empty());
        assert!(f.matches("any/path"));
    }

    #[test]
    fn any_of_several_patterns() {
        let f = PathFilter::new(&["*.db".to_string(), "*.sqlite".to_string()]);
        assert!(f.matches("a/x.db"));
        assert!(f.matches("a/y.sqlite"));
        assert!(!f.matches("a/z.txt"));
    }
}
