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

## txt

One line per match:

```
path:file_offset:archive_offset:line
```

- `--colour` wraps the matched bytes in ANSI bold-red (terminal only).
- DEFLATE matches show `~archive_offset`.
- With `--inspect`, a `  [format summary]` tag is appended.

Long lines (common in binary files, which have few newlines) are truncated to a
window around the match with `…` markers; offsets are unaffected.

## json

A single pretty-printed array. Inspection fields are present only when
`--inspect` matched a format:

```json
[
  {
    "path": "private/var/.../sms.db",
    "file_start": 4096,
    "file_offset": 5242880,
    "archive_offset": 4096,
    "compressed": true,
    "format": "sqlite",
    "context": { "page": 1281, "table": "message", "rowid": 4213, "column": "text" },
    "line": "…hello…"
  }
]
```

`context` is format-specific (see below).

## csv

A header row plus one row per match. Columns are fixed (so the set never varies):

```
path,file_start,file_offset,archive_offset,compressed,format,context,line
```

`format` and `context` are empty strings unless `--inspect` matched. `context`
is the human one-liner (the same text as the txt tag).

## Inspection `context` by format

| `format` | `context` (json) | summary (txt/csv) |
|---|---|---|
| `txt`    | `{ "line": N, "col": N }` | `line N, col N` |
| `json`   | `{ "path": "$.a.b[2]" }` | `$.a.b[2]` |
| `xml`    | `{ "path": "/a/b/c" }` | `/a/b/c` |
| `csv`    | `{ "row": N, "col": N, "header": "..." }` | `row N, col N (header)` |
| `plist`  | `{ "path": "$.Account.Servers[1]" }` | `$.Account.Servers[1]` |
| `bplist` | `{ "path": "...", "object": N }` | `$.Account.Servers[1]` |
| `sqlite` (in a row) | `{ "page": N, "table": "...", "rowid": N, "column": "..." }` | `table T, rowid R, column C` |
| `sqlite` (elsewhere) | `{ "page": N, "page_offset": N }` | `page N, offset N (not in a table cell)` |

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
