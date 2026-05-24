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
  engine.rs     orchestration: parse + parallel search (or --match-path) -> Findings
  filter.rs     EntryFilter: path globs (--path/--not-path) + --type / media skip
  fast.rs       the --fast preset's customisable exclude list
  inspect/      deep "what does this match mean" inspectors + file-type detection
    mod.rs        Inspector trait (name/category/detect/inspect) + registry +
                  detection (header-first) + detect_type (drives --type/skip-media)
    txt.rs json.rs xml.rs csv.rs plist.rs sqlite.rs   (resolve offsets)
    media/        the `media` category: mod.rs (macro + magic + re-exports) and
                  one file per format (jpeg.rs … mp3.rs), classification only
  output.rs     format match records to txt / json / csv
  export.rs     plan output paths, write manifest, export files (+ sidecars) to disk
```

## Data flow

```
archive bytes (mmap)
      │
      ▼
 zip::parse_entries ──► Vec<Entry>            (central-directory walk, ZIP64)
      │
      ▼
 filter.selects(path) ──► entries to search   (--path/--not-path, path-only)
      │
      ▼  (rayon, in parallel, per entry)
 search::entry_content ─► STORED: borrow mmap slice
                          DEFLATE: inflate into an owned buffer
      │
      ▼  inspect::detect_type (header-first) ─► filter.accepts_type
      │                         (--type allowlist / media skip; drop if excluded)
      │
      ▼
 search::search_bytes  ─► Vec<SearchHit>  (offset, line preview, match span)
      │                       │
      │                       └─(--inspect)─► inspect::inspect ─► Inspection
      ▼
 engine::Findings { records: Vec<MatchRecord>, files: Vec<MatchedFile> }
      │                         │
      ▼                         ▼
 output::write_results     export::plan ─► write_manifest / export_files
 (txt / json / csv)        (DIR/<basename>_<hash>/<basename>)
```

`engine::search_archive` produces **both** outputs in a single pass:

- `records` — one `MatchRecord` per match (for display/output).
- `files` — one `MatchedFile` per matched file, de-duplicated (for the export step).

So printing matches and exporting files never re-scan the archive.

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
- **Inspectors are a small API, reused for three jobs.** Each format is an
  `Inspector` (trait in `inspect/mod.rs`): `name`, `category`, `extensions`,
  `detect` (header), `inspect` (resolve), and optional `sidecars`. The same
  registry powers (1) deep inspection, (2) `--type`/media filtering via
  `detect_type` (header-first, then extension), and (3) BLOB classification
  inside SQLite via `detect_by_header` — so a file format is described once.
  Media formats are *classification-only* (one file per format, built on the
  `media_inspector!` macro); they detect but do not resolve. Adding a format is a
  new submodule plus one line in `INSPECTORS`. A format's associated files (e.g.
  SQLite's `-wal`) are declared by its `sidecars()`, which `export` copies
  automatically. See `docs/inspectors.md` + `inspector-template.rs`.
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
