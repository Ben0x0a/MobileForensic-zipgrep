//! In-file deep search: the inspector API, format detection, and dispatch.
//!
//! Defines: the [`Inspector`] trait (the standard interface every format
//! implements), the inspector registry, and the public entry points
//! [`inspect`] and [`sidecars_for`].
//! Used by: `engine.rs` (`--inspect`) and `export.rs` (sidecar pulling).
//! Uses: `crate::models::Inspection` and the per-format submodules.
//!
//! ── The inspector API ───────────────────────────────────────────────────────
//! An *inspector* turns a raw byte `offset` into a meaningful location inside a
//! recognised format. Every inspector is a zero-sized type implementing
//! [`Inspector`], lives in its own submodule, and is listed once in the
//! `INSPECTORS` registry. The core logic (detection order, dispatch, the public
//! entry points) is identical across formats and lives here; an inspector only
//! supplies its extensions, a header check, the resolver, and (optionally) its
//! sidecar files.
//!
//! Shared helpers (`line_at`, `looks_like_xml`, `contains`) are kept here and
//! documented so several inspectors can reuse them without duplicating code.
//!
//! Adding one: copy `docs/inspector-template.rs` into a new submodule, implement
//! [`Inspector`], and add it to the `INSPECTORS` registry. See
//! `docs/inspectors.md`.
//!
//! Currently implemented: TXT, JSON, XML, SQLite, CSV, plist (XML + binary).

use crate::models::Inspection;

mod csv;
mod json;
mod plist;
mod sqlite;
mod txt;
mod xml;

/// The interface every format inspector implements — the inspector API.
///
/// Implementors are zero-sized marker types (e.g. `struct Sqlite;`). The dispatch
/// here calls these methods; an inspector never has to touch detection ordering
/// or output plumbing.
pub trait Inspector: Sync {
    /// File-name extensions (lowercase, no dot) used for fallback detection when
    /// the content has no recognisable header.
    fn extensions(&self) -> &'static [&'static str];

    /// Recognise the format from the content's header/magic. Preferred over the
    /// extension, because forensic file names are often wrong or absent. Return
    /// `false` for formats with no signature (JSON, CSV, TXT).
    fn detect(&self, content: &[u8]) -> bool;

    /// Resolve a match at `offset` to a meaningful location. Return `None` when
    /// the offset can't be placed; the caller then emits a plain record.
    fn inspect(&self, content: &[u8], offset: usize) -> Option<Inspection>;

    /// Sidecar file suffixes pulled alongside a matched file of this format
    /// (e.g. SQLite's `-wal`). Default: none. Declaring them here is all it takes
    /// for `pull` to fetch them — see [`sidecars_for`].
    fn sidecars(&self) -> &'static [&'static str] {
        &[]
    }
}

/// Every inspector, in **detection-priority order**: `detect` (header) checks run
/// top-to-bottom, so list more specific formats first — e.g. plist before
/// generic XML, since both begin with `<?xml`.
static INSPECTORS: &[&dyn Inspector] = &[
    &sqlite::Sqlite,
    &plist::Plist,
    &xml::Xml,
    &json::Json,
    &csv::Csv,
    &txt::Txt,
];

/// Produce inspection context for a match at `offset` in `content`.
///
/// Returns `None` when the format is unsupported or undetected — the caller then
/// emits the plain match record.
pub fn inspect(name: &str, content: &[u8], offset: usize) -> Option<Inspection> {
    detect(name, content)?.inspect(content, offset)
}

/// Sidecar suffixes to pull alongside a matched file (empty if unrecognised).
///
/// Lets `export` fetch each format's associated files without hard-coding them:
/// the list comes from the matching inspector's [`Inspector::sidecars`].
pub fn sidecars_for(name: &str, content: &[u8]) -> &'static [&'static str] {
    detect(name, content).map_or(&[], |insp| insp.sidecars())
}

/// Pick the inspector for a file: the first whose header matches, else the first
/// claiming the file-name extension.
fn detect(name: &str, content: &[u8]) -> Option<&'static dyn Inspector> {
    if let Some(insp) = INSPECTORS.iter().copied().find(|i| i.detect(content)) {
        return Some(insp);
    }
    let base = name.rsplit('/').next().unwrap_or(name);
    let ext = base.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase())?;
    INSPECTORS
        .iter()
        .copied()
        .find(|i| i.extensions().contains(&ext.as_str()))
}

// ── Shared helpers (reused by several inspectors) ───────────────────────────

/// 1-based line number of `offset` within `content` (newline count + 1).
///
/// Used by the text-based inspectors (JSON, XML, plist-XML) so their summaries
/// can cite a line, like the TXT inspector.
pub(crate) fn line_at(content: &[u8], offset: usize) -> usize {
    let end = offset.min(content.len());
    1 + content[..end].iter().filter(|&&b| b == b'\n').count()
}

/// True if `content` looks like XML: an `<?xml` declaration, after an optional
/// UTF-8 BOM. Shared by the XML and plist inspectors' `detect`.
pub(crate) fn looks_like_xml(content: &[u8]) -> bool {
    let body = content.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(content);
    body.starts_with(b"<?xml")
}

/// True if `haystack` contains `needle`.
pub(crate) fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack.windows(needle.len()).any(|w| w == needle)
}
