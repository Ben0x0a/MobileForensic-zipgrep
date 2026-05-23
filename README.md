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
private/var/.../CellularUsage.db:4096:5242880:...IMSI 208...
```

- **~40–100× faster than `zipgrep`** on STORED archives (memory-mapped, no
  per-file process spawn, SIMD search).
- **Tells you *where*, always** — every match reports the file path plus three
  byte offsets (see [Offsets](#offsets)).
- **STORED + DEFLATE**: uncompressed entries are searched in place; DEFLATE
  entries are decompressed on demand.
- **Deep inspection** (`--inspect`): for recognised formats, resolves a match to
  a meaningful location — a SQLite `table/rowid/column`, a JSON/plist key path,
  an XML element path, a CSV row/column, …
- **Pull matched files out** with a re-ingestable manifest and a size cap.
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
# Find a string and show where it is (path : offset-in-file : offset-in-zip : line)
mf-zipgrep search 'secret' case.zip

# Case-insensitive, literal (not a regex)
mf-zipgrep search -i -F 'O2 UK' case.zip

# Restrict to certain files, and resolve matches inside them
mf-zipgrep search 'token' case.zip --path '*.sqlite' --path '*.plist' --inspect

# Machine-readable output to a file
mf-zipgrep search 'token' case.zip --format json -o hits.json

# Record matched files (with total size), review, then pull them out
mf-zipgrep search 'token' case.zip --manifest hits.json
mf-zipgrep pull case.zip --from-manifest hits.json --to ./pulled --max-size 500MB
```

---

## Commands

mf-zipgrep has two subcommands:

| Command | Purpose |
|---|---|
| `search PATTERN ARCHIVE` | Search the archive; print/record matches; optionally pull files. |
| `pull ARCHIVE --from-manifest FILE --to DIR` | Re-ingest a manifest and copy the listed files out (no search). |

### `search`

```
mf-zipgrep search PATTERN ARCHIVE [options]
```

| Flag | Meaning |
|---|---|
| `-i`, `--ignore-case` | Case-insensitive matching. |
| `-F`, `--fixed-strings` | Treat PATTERN as a literal string, not a regex. |
| `-E`, `--extended-regexp` | Accepted for grep muscle memory; no-op (the engine is already ERE-like). |
| `--path GLOB` | Only search files whose internal path matches the wildcard. Repeatable. |
| `--inspect` | Resolve matches inside recognised formats (see [Inspection](#deep-inspection)). |
| `--format txt\|json\|csv` | Output format (default `txt`). |
| `-o`, `--output FILE` | Write results to a file instead of stdout. |
| `--colour[=auto\|always\|never]` | Highlight matches (txt to a terminal). `--color` also accepted. |
| `-j`, `--threads N` | Search threads (default: one per CPU core). |
| `--manifest FILE` | Write a re-ingestable manifest of matched files (+ total size). |
| `--pull DIR` | Also copy matched files out, in one step. |
| `--max-size SIZE` | Refuse to pull if the matched total exceeds SIZE (e.g. `200MB`, `1G`). |

### `pull`

```
mf-zipgrep pull ARCHIVE --from-manifest FILE --to DIR [--max-size SIZE]
```

Re-ingests a manifest written by `search --manifest` and copies the listed files
out of the archive — **without searching again**. Honours `--max-size`.

> **Vocabulary:** *pull* = copy files out of the archive; *extract* is reserved
> for extracting *meaning* (the `--inspect` analysis).

---

## Output

Default txt is one line per match:

```
path:file_offset:archive_offset:line
```

`json` emits a single array of objects; `csv` emits a header row plus one row per
match. Both carry every field; txt shows the two most useful offsets. See
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
single archive byte, so `archive_offset` is the compressed blob's start (shown as
`~N` in txt, and `compressed: true` in json/csv).

---

## Deep inspection (`--inspect`)

When a matching file's format is recognised (by content header first, file
extension as fallback), mf-zipgrep resolves the match to a meaningful location.
It appends `[format summary]` in txt, and a nested `context` object in json.

| Format | Resolves a match to |
|---|---|
| TXT | line and column |
| JSON | key path, e.g. `$.users[3].token` |
| XML | element path, e.g. `/plist/dict/string` |
| CSV | row, column, and header name |
| plist (XML & binary `bplist`) | dict-key / array-index path, e.g. `$.Account.Servers[1]` |
| SQLite | `table`, `rowid`, `column` for live table rows; otherwise `page` + offset-in-page |

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
  to it.
- Offsets are **byte-accurate** for STORED entries (verifiable with `dd`/a hex
  editor).
- Long "lines" in binary files are capped to a window around the match for
  display; the reported offsets are exact regardless.
- Search is case-sensitive unless `-i`; `--path` matching is case-sensitive.

---

## License

Not yet chosen — see the TODO in `Cargo.toml` before publishing.
