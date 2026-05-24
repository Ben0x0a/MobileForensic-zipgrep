# Output formats, offsets, and the inspection schema

## Offsets

Every match carries the location, completely:

| Field | Meaning |
|---|---|
| `path` | File name and path inside the archive. |
| `file_start` | Byte offset where the matching file's data begins in the archive. |
| `file_offset` | The match position **within the file's logical content**. |
| `archive_offset` | The match's **absolute** byte position in the archive. |

For **STORED** entries `archive_offset == file_start + file_offset`, and it is
byte-accurate — you can seek to it directly:

```
dd if=acquisition.zip bs=1 skip=<archive_offset> count=16 2>/dev/null
```

For **DEFLATE** entries the match exists only in the decompressed stream, which
has no single byte in the archive. There:

- `file_offset` is the position in the **decompressed** data;
- `archive_offset` is set to `file_start` (the compressed blob's start);
- the record is flagged **compressed** (`~` prefix in txt, `compressed: true`
  in json/csv).

**Output rules:** at most one line per match, and binary file content is never
raw-dumped. The matched line is shown only when it looks **textual**; binary
files (SQLite, bplist, …) contribute location only. Per-format context is opt-in
via `--inspect`.

**Multiple archives:** when more than one archive is searched in a run, each
result is tagged with its source archive — a `archive:` prefix in txt, an
`archive` field in json, and the leading `archive` column in csv. With a single
archive there is no tag (json omits the field; the csv column is empty).

## txt

One line per match:

```
path:0x<file_offset>[:line][  [format  labelled summary]]
```

- The offset is **hex** (`0x…`) to match how analysts read a hex editor.
- `:line` (the matched line) appears only for **textual** files; binary files
  show just `path:0x<offset>`.
- `--colour` wraps the matched bytes in ANSI bold-red (terminal only).
- With `--inspect`, a labelled `  [format  key: value  …]` tag is appended.

Examples:

```
notes.txt:0x1a2:the meeting is at 5pm
notes.txt:0x1a2:the meeting is at 5pm  [txt  line: 12  col: 4]
sms.db:0x500000                          (binary: location only)
sms.db:0x500000  [sqlite  table: message  column: text  row: 4213  cell: hello there]
```

> Note: the absolute `archive_offset` and `compressed` flag are not in txt (they
> are in json/csv). txt favours the in-file offset, which is what you seek to.

## json

A single pretty-printed array. `line` appears only for textual matches;
`format`/`context` only with `--inspect`. Offsets are decimal integers (valid
JSON; use hex in txt for human reading):

```json
[
  {
    "path": "private/var/.../sms.db",
    "file_start": 4096,
    "file_offset": 5242880,
    "archive_offset": 4096,
    "compressed": true,
    "format": "sqlite",
    "context": { "page": 1281, "table": "message", "rowid": 4213, "column": "text", "cell": "hello there" }
  }
]
```

`context` is format-specific (see below). For binary formats the decoded value
lives in `context` (e.g. `cell`); for text formats the surrounding `line` is the
content.

## csv

A header row plus one row per match. Columns are fixed (so the set never varies):

```
archive,path,file_start,file_offset,archive_offset,compressed,format,context,line
```

`archive` is empty unless several archives were searched; `format`/`context` are
empty unless `--inspect` matched; `line` is empty for binary files. `context` is
the human labelled one-liner (the same text as the txt tag).

## counts (`--count`)

One line per file (only files with at least one match):

```
sms.db:3
app.json:1
```

`--format json` emits `[{ "path": …, "count": N }]`; `--format csv` emits a
`path,count` table.

## Inspection `context` by format

| `format` | `context` (json) | summary (txt tag / csv) |
|---|---|---|
| `txt`    | `{ "line": N, "col": N }` | `line: N  col: N` |
| `json`   | `{ "path": "$.a.b[2]", "line": N }` | `key: $.a.b[2]  line: N` |
| `xml`    | `{ "path": "/a/b/c", "line": N }` | `path: /a/b/c  line: N` |
| `csv`    | `{ "row": N, "col": N, "header": "..." }` | `row: N  col: N  header: H` |
| `plist`  | `{ "path": "$.Account.Servers[1]", "line": N }` | `key: $.Account.Servers[1]  line: N` |
| `bplist` | `{ "path": "...", "object": N }` | `key: $.Account.Servers[1]` |
| `sqlite` (in a row) | `{ "page": N, "table": "...", "rowid": N, "column": "...", "cell": "..." }` | `table: T  column: C  row: R  cell: V` |
| `sqlite` (elsewhere) | `{ "page": N, "page_offset": N }` | `page: N  offset: N  (not in a table cell)` |

`cell` (and the txt `cell:` field) is the **decoded**, length-capped, text-safe
value — a TEXT/INTEGER/REAL value as text, a NULL as `NULL`, a BLOB as
`<blob N bytes>`. Raw bytes are never shown.

## Manifest schema (`--manifest` / `pull --from-manifest`)

```json
{
  "total_size": 412300191,
  "file_count": 37,
  "files": [
    {
      "internal_path": "private/var/.../sms.db",
      "output_path": "sms.db_a3f2c1d0e5/sms.db",
      "size": 5242880,
      "compressed": false,
      "offsets": [5242880, 5300000]
    }
  ]
}
```

- `total_size` is the sum of `size` over all matched files — known *before* you
  pull.
- `output_path` is the relative path the file will be written to under `--to`.
- `offsets` are the `file_offset`s of every match in that file.
- `pull --from-manifest` reuses `output_path` and locates each file by
  `internal_path`; missing entries are reported as skipped.
