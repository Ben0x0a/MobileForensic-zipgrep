# Inspectors (`--inspect`)

An *inspector* turns a raw match offset into a meaningful location inside a
recognised file format. Inspection is opt-in (`--inspect`) and never changes the
match itself — it only adds `format` + `context` to the output.

## Format detection (header-first)

`inspect::detect` chooses a format using the **content header first**, then the
file extension as a fallback. Forensic file names are often wrong, renamed, or
absent, so a reliable in-content signature is trusted over the extension; the
extension only decides formats that have no magic bytes (JSON, CSV, TXT).

| Detected as | By magic (header) | By extension |
|---|---|---|
| SQLite | `SQLite format 3\0` | `.sqlite .sqlite3 .db .sqlitedb` |
| plist (binary) | `bplist00` | `.plist` |
| plist (XML) | `<?xml …` containing `<plist`/`DOCTYPE plist` | `.plist` |
| XML | `<?xml …` (non-plist) | `.xml` |
| JSON | — | `.json` |
| CSV | — | `.csv` |
| TXT | — | `.txt .log .text` |

A file that matches no inspector is reported as a plain match (no `context`).

## What each inspector resolves

### TXT
Counts newlines up to the offset → 1-based **line** and (byte) **column**.

### JSON
A single-pass scanner tracks the path stack and reports the **key path** of the
value (or key) containing the offset, e.g. `$.users[3].token`. (serde gives a
parsed tree but no byte spans, so this is hand-rolled; it locates, it does not
validate.)

### XML
Uses quick-xml's event byte positions to track open elements and reports the
**element path**, e.g. `/plist/dict/string`. Attribute-level resolution is a
future refinement.

### CSV
A quote-aware scan (RFC 4180: `"`-quoted fields, `""` escapes) reports the 1-based
**row** and **column** plus the **header** name (the column's value in row 1).
Commas/newlines inside quotes do not miscount.

### plist (XML and binary)
Both encodings resolve to the same dict-key / array-index **path**, e.g.
`$.Account.Servers[1]`.

- XML plists are walked as XML, tracking `<key>` names and array positions.
- Binary plists (`bplist00`) are parsed via the trailer + offset table: the
  object whose byte span contains the offset is located, then a path to it is
  found by walking the object graph from the root.

### SQLite
Parses the database header (page size), then the b-tree:

- If the offset lands in a **live table-leaf cell**, it resolves to
  `table + rowid + column` (column names come from the `CREATE TABLE` SQL in the
  schema; the cell's record format gives the column byte spans).
- Otherwise — freelist pages, free blocks, interior/overflow pages, unallocated
  space — it reports just `page` + `offset-in-page`.

This split is intentional: forensically, "which page/offset" is still useful when
a byte isn't part of a current row (e.g. a deleted record in free space).

## The inspector API

Every format is an `Inspector` (the trait in `src/inspect/mod.rs`):

```rust
pub trait Inspector: Sync {
    fn extensions(&self) -> &'static [&'static str]; // fallback detection
    fn detect(&self, content: &[u8]) -> bool;        // header/magic (preferred)
    fn inspect(&self, content: &[u8], offset: usize) -> Option<Inspection>;
    fn sidecars(&self) -> &'static [&'static str] { &[] } // associated files
}
```

Inspectors are zero-sized types listed once in `INSPECTORS`. The shared core —
detection order (header-first, then extension), dispatch, and output — lives in
`mod.rs`; an inspector only supplies its format specifics. The per-match
`Inspection { format, summary, detail }` carries the short `format` tag, the
labelled human one-liner (`summary`, used by the txt tag and csv `context`), and
the structured `serde_json::Value` (`detail`, used by json). Shared helpers
(`line_at`, `looks_like_xml`, `contains`) live in `mod.rs` and are documented so
inspectors reuse rather than duplicate them.

### Adding one

1. Copy [`inspector-template.rs`](inspector-template.rs) to
   `src/inspect/<format>.rs` and `mod <format>;` it in `src/inspect/mod.rs`.
2. Fill in `extensions` / `detect` / `inspect` (and `sidecars` if any).
3. Register it: add `&<format>::Foo` to `INSPECTORS` in `src/inspect/mod.rs` —
   **list more specific formats first**, because `detect` (header) checks run in
   order (e.g. plist before generic XML).
4. Reuse shared helpers; add new ones to `mod.rs` with a doc comment.
5. Add a test, ideally against a committed fixture in `tests/fixtures/`.

### Sidecars (associated files)

A format declares the files pulled alongside a match via `sidecars()` — suffixes
appended to the file name, e.g. SQLite returns `["-wal", "-shm", "-journal"]`.
`pull` fetches them automatically (`inspect::sidecars_for`), so adding a format's
sidecars is one line in its inspector — `export.rs` needs no change.

## Planned

ABX (Android Binary XML) and SEGB (Apple record container) are planned but need
real sample files to implement reliably — see [roadmap.md](roadmap.md).
