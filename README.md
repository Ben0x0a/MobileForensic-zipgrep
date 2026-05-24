# mf-zipgrep

Fast, forensic-aware regex search **inside** ZIP acquisitions — a `zipgrep` built
for mobile-forensic images.

Mobile acquisitions are frequently delivered as large ZIP archives of a phone's
file system. Searching them with `zipgrep` is painfully slow: it spawns a process
per entry and copies every byte through pipes. mf-zipgrep memory-maps the archive
and runs a SIMD regex engine straight over the bytes — and because acquisition
ZIPs usually store entries **uncompressed** (STORED), there is nothing to
decompress on the hot path.

```
$ mf-zipgrep search 'IMSI' acquisition.zip
private/var/.../CellularUsage.db:0x1f4a:...IMSI 208...
```

- **~40–100× faster than `zipgrep`** on STORED archives (memory-mapped, no
  per-file process spawn, SIMD search).
- **Tells you *where*, always** — every match reports the file path plus three
  byte offsets (see [Offsets](#offsets)).
- **STORED + DEFLATE**: uncompressed entries are searched in place; DEFLATE
  entries are decompressed on demand.
- **Deep inspection** (`--inspect`): for recognised formats, resolves a match to
  a meaningful location — a SQLite `table/column [TYPE]/rowid` (plus the embedded
  format when the cell is a BLOB), a JSON/plist key path, an XML element path, a
  CSV row/column, …
- **Filter by file type** (`--type`): keep only a format or a whole category
  (e.g. `--type sqlite`, `--type media`), recognised by content **header first**,
  then extension — the same detection the inspectors use.
- **Find files by path** (`--match-path`): apply the pattern to each file's
  internal **path** instead of its content — list every file whose path matches.
- **Export matched files out** with a re-ingestable manifest and a size cap.
- **Multi-threaded**, with a live progress hint on a terminal.

> Status: **v1**. Some inspectors (ABX, SEGB) are still planned — see
> [docs/roadmap.md](docs/roadmap.md).

---

## Install

Requires a Rust toolchain (2024 edition, e.g. Rust 1.95+).

```
cargo build --release
# binary at: target/release/mf-zipgrep
```

---

## Quick start

```
# Find a string and show where it is (path:0x<offset-in-file>:line)
mf-zipgrep search 'secret' case.zip

# Case-insensitive, literal (not a regex)
mf-zipgrep search -i -F 'O2 UK' case.zip

# Restrict to certain files, and resolve matches inside them
mf-zipgrep search 'token' case.zip --path '*.sqlite' --path '*.plist' --inspect

# Only search databases (detected by header, not just extension)
mf-zipgrep search 'token' case.zip --type database --inspect

# List every file whose path contains "banking" (no content searched)
mf-zipgrep search 'banking' case.zip --match-path

# Machine-readable output to a file
mf-zipgrep search 'token' case.zip --format json -o hits.json

# Record matched files (with total size), review, then export them out
mf-zipgrep search 'token' case.zip --manifest hits.json
mf-zipgrep export case.zip --from-manifest hits.json --to ./exported --max-size 500MB
```

---

## Commands

mf-zipgrep has two subcommands:

| Command | Purpose |
|---|---|
| `search PATTERN ARCHIVE...` | Search one or more archives (or directories, with `-r`); print/record matches; optionally export files. |
| `export ARCHIVE --from-manifest FILE --to DIR` | Re-ingest a manifest and copy the listed files out (no search). |

### `search`

```
mf-zipgrep search PATTERN ARCHIVE... [options]
```

Grep-style: the **PATTERN comes first**, then the archives. `ARCHIVE` may be
repeated, and with `-r` a directory argument is searched recursively for its
`*.zip` files. With more than one archive, each result is tagged with its source
(see [Output](#output)). `--export`/`--manifest` require a single archive.

| Flag | Meaning |
|---|---|
| `-i`, `--ignore-case` | Case-insensitive matching. |
| `-F`, `--fixed-strings` | Treat PATTERN as a literal string, not a regex. |
| `-E`, `--extended-regexp` | Accepted for grep muscle memory; no-op (the engine is already ERE-like). |
| `-r`, `--recursive` | Search directory arguments recursively for `*.zip` files. |
| `--path GLOB` | Only search files whose internal path matches the wildcard. Repeatable. |
| `--not-path GLOB` | Skip files matching the wildcard (takes precedence over `--path`). Repeatable. |
| `--type TYPE` | Only search files of a format (`sqlite`, `jpeg`, …) or category (`media`, `database`, `structured`, `text`). Header-first, then extension. Repeatable. |
| `--match-path` | Match the PATTERN against each file's internal path instead of its content; list the files whose path matches (no content is read). |
| `--include-media` | Search image/video/audio files too (skipped by default — see below). |
| `--fast` | Speed preset: skip media + all cores + a customisable exclude list (`src/fast.rs`). |
| `--inspect` | Resolve matches inside recognised formats (see [Inspection](#deep-inspection)). |
| `-c`, `--count` | Print only the match count per file (one line per file). |
| `--format txt\|json\|csv` | Output format (default `txt`). |
| `-o`, `--output FILE` | Write results to a file instead of stdout. |
| `--colour[=auto\|always\|never]` | Highlight matches (txt to a terminal). `--color` also accepted. |
| `-j`, `--threads N` | Search threads (default: one per CPU core). |
| `--manifest FILE` | Write a re-ingestable manifest of matched files (+ total size). |
| `--export DIR` | Also copy matched files out, in one step. |
| `--max-size SIZE` | Refuse to export if the matched total exceeds SIZE (e.g. `200MB`, `1G`). |
| `--verify` | SHA-256 the archive before and after the run; report whether it changed (integrity attestation). |

### `export`

```
mf-zipgrep export ARCHIVE --from-manifest FILE --to DIR [--max-size SIZE]
```

Re-ingests a manifest written by `search --manifest` and copies the listed files
out of the archive — **without searching again**. Honours `--max-size`.

> **Vocabulary:** *export* = copy files out of the archive; *extract* is reserved
> for extracting *meaning* (the `--inspect` analysis).

---

## Output

One line per match; binary file content is **never** raw-dumped. Default txt:

```
path:0x<file_offset>[:line]
```

The offset is hex (like a hex editor). The matched `line` is shown only for
**textual** files; binary files (SQLite, bplist, …) show just `path:0x<offset>`.
`--inspect` appends a labelled tag; `--count` prints `path:count` per file.

```
notes.txt:0x1a2:the meeting is at 5pm
sms.db:0x500000                                  (binary: location only)
sms.db:0x500000  [sqlite  table: message  column: text  row: 4213  cell: hello there]
```

`json` emits a single array of objects; `csv` a header row plus one row per
match (offsets are decimal there). See
[docs/output-and-offsets.md](docs/output-and-offsets.md) for the full schema.

### Offsets

Every match answers "where", completely:

| Field | Meaning |
|---|---|
| `path` | File name and path inside the archive. |
| `file_start` | Where the matching file's data begins in the archive. |
| `file_offset` | The match position **inside the file**. |
| `archive_offset` | The match's **absolute** byte position in the archive (`= file_start + file_offset` for STORED). |

For **DEFLATE** entries the match lives in the decompressed stream, which has no
single archive byte, so `archive_offset` (in json/csv) is the compressed blob's
start, flagged `compressed: true`. txt shows only the in-file offset.

---

## Deep inspection (`--inspect`)

When a matching file's format is recognised (by content header first, file
extension as fallback), mf-zipgrep resolves the match to a meaningful location.
It appends `[format summary]` in txt, and a nested `context` object in json.

| Format | Resolves a match to |
|---|---|
| TXT | line and column |
| JSON | key path + line, e.g. `$.users[3].token` |
| XML | element path + line, e.g. `/plist/dict/string` |
| CSV | row, column, and header name |
| plist (XML & binary `bplist`) | dict-key / array-index path, e.g. `$.Account.Servers[1]` |
| SQLite | `table`, `column [TYPE]`, `row`, and the **decoded cell value** for live rows; otherwise `page` + offset-in-page |

For a SQLite **BLOB** cell, the blob's own signature is checked and, if it is a
recognised format (e.g. an embedded `bplist`), that inspector resolves it too —
so a match reads `column: payload [BLOB] … blob: bplist  key: $.Account.Servers`.

See [docs/inspectors.md](docs/inspectors.md) for details and [docs/roadmap.md](docs/roadmap.md)
for planned formats (ABX, SEGB).

---

## Documentation

- [docs/architecture.md](docs/architecture.md) — modules, data flow, design decisions.
- [docs/workflow.md](docs/workflow.md) — forensic workflows end to end.
- [docs/output-and-offsets.md](docs/output-and-offsets.md) — output formats, offsets, inspection schema.
- [docs/inspectors.md](docs/inspectors.md) — supported formats, detection, adding one.
- [docs/roadmap.md](docs/roadmap.md) — planned features.

---

## Forensic notes

- The archive is opened **read-only** and memory-mapped; mf-zipgrep never writes
  to it. `--verify` adds a SHA-256 attestation (hash before & after the run) for
  chain-of-custody — the hash matches the system `shasum -a 256`.
- Offsets are **byte-accurate** for STORED entries (verifiable with `dd`/a hex
  editor).
- Long "lines" in binary files are capped to a window around the match for
  display; the reported offsets are exact regardless.
- Search is case-sensitive unless `-i`; `--path`/`--not-path` matching is case-sensitive.
- **Media files (images/video/audio) are skipped by default** — they hold no
  searchable text and dominate acquisition size. Use `--include-media` to search
  them (e.g. to scan for text embedded in a file mislabelled as media).

---

## License

Not yet chosen — see the TODO in `Cargo.toml` before publishing.
