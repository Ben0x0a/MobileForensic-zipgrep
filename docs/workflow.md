# Workflows

End-to-end recipes for common forensic tasks.

## 1. Find where a value occurs

```
mf-zipgrep search 'IMSI' acquisition.zip
```

Output is `path:0x<file_offset>` plus, for textual files, the matched line. The
hex offset lets you jump straight to the bytes (e.g. with a hex editor) and the
path tells you which file inside the archive it came from. Binary files show the
location only (no content); use `--inspect` to resolve them.

Useful modifiers:

- `-i` case-insensitive, `-F` literal (treat the pattern as plain text).
- `--colour` to highlight the match (auto on a terminal).
- `--path '*.db'` to limit the search to certain files (repeatable).

## 2. Narrow to specific files

The `--path` filter matches an entry's **internal path** with wildcards
(`*` matches any run including `/`, `?` matches one character):

```
# only SQLite databases and plists
mf-zipgrep search 'token' acquisition.zip --path '*.sqlite' --path '*.plist'

# anything under a Messages container
mf-zipgrep search 'hello' acquisition.zip --path '*/Messages/*'
```

Exclude with `--not-path` (takes precedence over `--path`):

```
mf-zipgrep search 'token' acquisition.zip --not-path '*Caches*' --not-path '*.log'
```

**Media files (images/video/audio) are skipped by default** — they contain no
searchable text and are most of an acquisition's size. Pass `--include-media` to
search them anyway. Filtering happens before the search, so it also scopes what
gets pulled.

## 3. Understand a match (deep inspection)

Add `--inspect` to resolve a match to a meaningful location inside structured
files:

```
mf-zipgrep search 'alice@example.com' acquisition.zip --path '*.sqlite' --inspect
# sms.db:0x500000  [sqlite  table: message  column: sender  row: 4213  cell: alice@example.com]
```

In `--format json`, inspection appears as a nested `context` object you can
filter on programmatically (e.g. all hits in a given SQLite table).

## 4. Machine-readable output

```
mf-zipgrep search 'token' acquisition.zip --format json -o hits.json
mf-zipgrep search 'token' acquisition.zip --format csv  -o hits.csv
```

Pipe-friendly too: when stdout is not a terminal there is no colour and no
progress noise on stderr.

## 5. Pull matched files out — the manifest workflow

The recommended pattern separates *deciding what to pull* from *pulling it*:

```
# 1) Search and record the matched files (and the total size) without copying.
mf-zipgrep search 'private_key' acquisition.zip --manifest hits.json

#    stderr: "manifest: 37 files, 412300191 bytes total -> hits.json"

# 2) Review hits.json / the total. Then pull, with a guard rail.
mf-zipgrep pull acquisition.zip --from-manifest hits.json --to ./pulled --max-size 1G
```

- The manifest is **re-ingestable**: it records each matched file's internal
  path, its assigned output path, size, compressed flag, and match offsets, plus
  a precomputed `total_size`.
- `pull` does **not** search again — it locates each listed file by path and
  copies it out, reusing the recorded output path.
- `--max-size` makes `pull` (and inline `--pull`) **refuse** if the total
  exceeds the cap (it writes nothing), so you never accidentally explode a
  hundred gigabytes onto disk.

### One-step variant

If you don't need the review step:

```
mf-zipgrep search 'private_key' acquisition.zip --pull ./pulled --max-size 1G --manifest hits.json
```

## 6. Search several archives at once

List several archives after the PATTERN, or point `-r` at a directory to search
every `*.zip` under it:

```
mf-zipgrep search 'IMSI' case-a.zip case-b.zip
mf-zipgrep search -r 'IMSI' ./acquisitions/
```

With more than one archive each result is tagged with its source: in txt the
archive joins the path like a folder (`case-a.zip/private/.../sms.db:0x..`); under
`-r` the archive is shown **relative to the directory** (`sub/case-b.zip/...`).
json/csv carry it as an `archive` field/column. A single archive is untagged.
`--pull`/`--manifest` require a single archive.

## 7. Output layout of pulled files

Each matched file is written to:

```
DIR/<basename>_<hash>/<basename>
```

- The file **keeps its real name** inside the folder, so extensions and
  associations still work.
- The folder is the basename plus a short, **stable** hash of the file's internal
  path — so the *same* logical file lands in the *same-named* folder across
  different acquisitions, and you learn to recognise recurrent files.
- Collisions are impossible by construction; in the rare event two paths hash the
  same, `_0x<offset>` is appended.

Example:

```
pulled/
  sms.db_a3f2c1d0e5/sms.db
  Info.plist_7b1c9e4f02/Info.plist
```

When a matched file is a SQLite database, its **sidecars** (`-wal`, `-shm`,
`-journal`) are pulled into the same folder if present in the archive — so the
database opens complete (uncommitted rows live in the WAL):

```
pulled/
  sms.db_a3f2c1d0e5/sms.db
  sms.db_a3f2c1d0e5/sms.db-wal
```

## Notes

- DEFLATE entries are decompressed automatically; their `file_offset` is the
  position in the decompressed stream and `archive_offset` is the compressed
  blob's start (flagged `compressed`).
- Directory entries and unsupported compression methods are skipped.
