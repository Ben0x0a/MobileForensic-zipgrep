//! The `--fast` preset — speed-oriented defaults bundled behind one flag.
//!
//! Defines: `FAST_EXCLUDE_GLOBS`, the editable list of path globs that `--fast`
//! skips, on top of the always-on speed options.
//! Used by: `main` (when `--fast` is set, these globs are added to the filter's
//! excludes).
//! Uses: nothing.
//!
//! What `--fast` does today: skip media files (already the default) + use all
//! CPU cores (already the default) + skip the path globs listed below. As the
//! preset grows, add more here.
//!
//! ── Customising the preset ──────────────────────────────────────────────────
//! Edit `FAST_EXCLUDE_GLOBS` to tune what `--fast` treats as noise. Each entry is
//! a wildcard matched against an entry's internal path, exactly like `--not-path`
//! (`*` matches any run including `/`, `?` matches one character). The list is
//! kept here, in one obvious place, so it is easy to extend without touching the
//! search logic. For one-off exclusions, prefer `--not-path` on the command line.
//!
//! Kept intentionally empty for now (so `--fast` == skip-media + multithread).
//! Examples you might add as you identify reliably-noisy locations:
//!   "*/Caches/*", "*/tmp/*", "*.log"

/// Path globs skipped by `--fast`, in addition to the always-on speed options.
/// See the module docs for how to customise this.
pub const FAST_EXCLUDE_GLOBS: &[&str] = &[
    // (none yet — add path globs here to extend the --fast preset)
];
