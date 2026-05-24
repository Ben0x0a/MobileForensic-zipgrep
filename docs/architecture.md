# Architecture

mf-zipgrep is a Rust **library crate** (`mf_zipgrep`) with a thin **binary** on
top. All logic lives in the library so it is unit-testable without the CLI; the
binary is only argument parsing and I/O.

```
src/
  lib.rs        library root (declares the modules below)
  main.rs       BINARY: clap subcommands, mmap, progress reporter, I/O wiring
  models.rs     data containers: Method, Entry, SearchHit, MatchRecord, Inspection
  zip.rs        ZIP central-directory parser (STORED + DEFLATE, ZIP64)
  search.rs     per-entry byte search (regex::bytes) + DEFLATE inflate + line preview
  engine.rs     orchestration: parse + parallel search -> Findings; Progress trait
  filter.rs     EntryFilter: include/exclude globs (--path/--not-path) + skip-media
  fast.rs       the --fast preset's customisable exclude list
  inspect/      deep "what does this match mean" inspectors
    mod.rs        Inspector trait + registry + detection (header-first) + dispatch
    txt.rs json.rs xml.rs csv.rs plist.rs sqlite.rs
  output.rs     format match records to txt / json / csv
  export.rs     plan output paths, write manifest, pull files (+ sidecars) to disk
```

## Data flow

```
archive bytes (mmap)
      │
      ▼
 zip::parse_entries ──► Vec<Entry>            (central-directory walk, ZIP64)
      │
      ▼
 filter (EntryFilter) ──► entries to search  (--path/--not-path, skip-media)
      │
      ▼  (rayon, in parallel, per entry)
 search::entry_content ─► STORED: borrow mmap slice
                          DEFLATE: inflate into an owned buffer
      │
      ▼
 search::search_bytes  ─► Vec<SearchHit>  (offset, line preview, match span)
      │                       │
      │                       └─(--inspect)─► inspect::inspect ─► Inspection
      ▼
 engine::Findings { records: Vec<MatchRecord>, files: Vec<MatchedFile> }
      │                         │
      ▼                         ▼
 output::write_results     export::plan ─► write_manifest / pull
 (txt / json / csv)        (DIR/<basename>_<hash>/<basename>)
```

`engine::search_archive` produces **both** outputs in a single pass:

- `records` — one `MatchRecord` per match (for display/output).
- `files` — one `MatchedFile` per matched file, de-duplicated (for the pull step).

So printing matches and pulling files never re-scan the archive.

## Why these choices

- **CD-first parsing.** The Central Directory is the authoritative list of
  entries (local headers may carry zeroed sizes when a data descriptor is used).
  Parsing it first gives exact data ranges and each entry's compression method.
  The true data offset is then resolved from each Local File Header, whose
  name/extra lengths may differ from the CD's (a classic ZIP-parsing pitfall).
- **mmap + `regex::bytes`.** STORED data is already uncompressed on disk, so a
  memory-mapped, SIMD byte-regex search runs with no copy and no decompression.
  Matching is on `&[u8]`, never `&str`, because forensic data is arbitrary bytes.
- **Parallelism via rayon.** Entries are searched in parallel; `collect` into a
  `Vec` preserves entry order, so output is deterministic. ~2.5× over single
  thread on a warm cache.
- **Library has no UI.** `engine::Progress` is a trait the engine calls; the
  terminal reporter lives in `main`. `output::OutputFormat` parses via `FromStr`,
  not clap's `ValueEnum`, so the core never depends on the CLI framework.
- **Inspectors are a small API.** Each format is an `Inspector` (trait in
  `inspect/mod.rs`): `extensions`, `detect` (header), `inspect` (resolve), and
  optional `sidecars`. The shared core (header-first detection,
  dispatch, output) lives in `mod.rs`; adding a format is a new submodule
  implementing the trait plus one line in the `INSPECTORS` registry. A format's
  associated files (e.g. SQLite's `-wal`) are declared by its `sidecars()`, which
  `export` pulls automatically. See `docs/inspectors.md` + `inspector-template.rs`.
- **Errors, not panics.** All parsing uses bounds-checked reads that return
  `Result`; anything an inspector can't resolve degrades gracefully (e.g. SQLite
  falls back to `page + offset`) rather than erroring — forensic inputs are
  often partial or corrupt.

## Key types (`models.rs`)

- `Method` — `Stored` or `Deflate` (the only methods searched).
- `Entry` — a searchable file: name, method, `data_offset`, `data_len`,
  `uncompressed_size`.
- `SearchHit` — one match within an entry: `offset`, the line preview bytes, and
  the match's range within the line (for highlighting).
- `MatchRecord` — an archive-level match with all offsets + optional
  `Inspection`. `MatchRecord::new` is the single home of the STORED-vs-DEFLATE
  offset rule.
- `Inspection` — `{ format, summary, detail }`: a human one-liner plus a
  structured JSON value.

## Testing

Tests live in `tests/` and use the library directly (the CLI is not exercised by
tests, so CLI changes don't churn them). ZIP fixtures are **hand-built byte by
byte** (`tests/common`) so they are deterministic and need no external `zip`
tool. Binary-format inspectors are tested against committed real fixtures
(`tests/fixtures/`: a SQLite DB from `sqlite3`, a plist + bplist from `plutil`).

`cargo test` runs 70+ tests; `cargo clippy --all-targets -- -D warnings` is clean.
