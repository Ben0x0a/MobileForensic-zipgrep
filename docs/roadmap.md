# Roadmap

Status legend: **Planned** = agreed/discussed; **Proposed** = suggestion awaiting
your validation.

---

## Planned (discussed)

### More inspectors — ABX and SEGB
- **ABX** (Android Binary XML, e.g. `packages.xml` on Android 12+): magic
  `ABX\0`, a token stream with an interned string pool. Resolves to an element
  path like the XML inspector.
- **SEGB** (Apple record container — KnowledgeC, biome, locationd): resolves a
  match to the containing **record** (index + offset, plus record metadata).
- Both are **blocked on real sample files** to validate against — implementing
  binary parsers blind would risk silently-wrong results. Drop samples into
  `tests/fixtures/` and they slot into the existing inspector framework.

### Export sidecar / associated files
When exporting a matched file, also export its **associated files** so the artefact
is complete and analysable later:
- **SQLite**: the `-wal`, `-shm`, and `-journal` sidecars (uncommitted data lives
  in the WAL; analysing the `.db` without it can miss or misread recent records).
- Generalise to a per-format "associated files" rule for future formats.
Recorded for later (user 2026-05-23).

### iOS GUID → app-package resolution for `--path`
On iOS, app data lives under randomised GUID directories
(`.../Applications/<GUID>/...`). Resolve those GUIDs to human app names
(bundle IDs) so filters and output read naturally:
- Build a GUID → bundle-ID map from the acquisition itself
  (`MobileInstallation`/`installd` plists, `iTunesMetadata.plist`,
  `.com.apple.mobile_container_manager.metadata.plist`).
- Let `--path` match on the resolved app name (e.g. `--app WhatsApp`) and show
  the app name alongside the GUID in output.

---

## Planned — forensic integrity & execution log

A first-class **chain-of-custody / traceability** feature:

- **Execution log** (e.g. `--log run.json`, or always-on sidecar): record the
  tool version, UTC timestamp, the exact command line and parameters, the
  archive path, match/file counts, and a summary of results — a reproducible
  account of what was done.
- **Before/after archive hash** (e.g. `--attest`): compute a cryptographic hash
  (SHA-256) of the archive **before** the run and **again after**, and record
  both, certifying the evidence was not modified.
  - Note: mf-zipgrep already opens the archive **read-only** and never writes to
    it, so integrity holds by construction; the before/after hash is a signed
    *attestation* of that fact for court/report use.
  - Trade-off: hashing a multi-hundred-GB archive twice is I/O-heavy, so make it
    opt-in; consider recording a hash supplied by the acquisition tool when
    present, to avoid a full re-read.
- **Per-file hashes on export**: record a SHA-256 of each exported file in the
  manifest, so exported artefacts carry their own integrity value.

---

## Proposed — for your validation

Grouped by theme. Each is a suggestion; tell me which to commit.

### Search power
1. **Multiple patterns / IOC sweep** — `-e PAT` (repeatable) and `-f FILE`
   (patterns from a file) to hunt many indicators (numbers, emails, hashes) in
   one pass. High value for triage.
2. **Encoding-aware literals** — for `-F` literals, optionally also match the
   **UTF-16LE** form, so a phone number or name is found in both UTF-8 and
   Windows/iOS UTF-16 artefacts.
3. **Built-in presets** — `--preset email|phone|imei|imsi|url|...` shipping
   vetted regexes for common forensic artefacts.
4. **Exclude filter** — `--not-path GLOB` to complement `--path` (e.g. skip
   `*/Caches/*`).
5. **Per-file match cap** — `--max-matches N` to bound output for fast triage.
   (`--count` is **done** — shipped.)

### Performance
13. **Skip media to go faster** — by default (or via `--skip-media`), skip image
    and video files (jpg, png, heic, gif, mp4, mov, …, detected by magic and
    extension) so the scan spends time only on data likely to contain text/IOCs.
    Large acquisitions are mostly photos/videos, so this is a big speed win.
    Add `--include-media` to override.

### Inputs
6. **tar / tar.gz acquisitions** — many full-filesystem iOS images are `.tar`
   (sometimes gzip-wrapped). Same engine, a different container reader. Likely
   the single biggest reach extension.
7. **Multiple archives in one run** — accept several archives (or a directory)
   and tag each result with its source archive.
8. **Nested archives** — optionally recurse into zip-in-zip / archive-in-archive.

### Output & analysis
9. **Byte-context window** — `--context N` to show N bytes either side of a match
   (the binary-friendly analogue of grep's `-A/-B/-C`).
10. **SQLite row dump** — with `--inspect`, optionally include the full row's
    column values (not just the matched column) for richer context.
11. **Group-by / summary report** — counts of matches per file, per format, per
    SQLite table; an executive summary for a report.

### Verification
12. **`--verify` option** (secure, slower) — opt-in flag on `search`/`export` that
    hashes the archive **before and after** the run and re-reads matched bytes to
    confirm they still match, producing an integrity-checked result. Trades speed
    for a court-defensible guarantee; pairs with the execution log above.

---

## Done in v2

- **Skip media by default** (#13) + `--include-media`.
- **`--not-path` exclude filter** (#4).
- **Export SQLite sidecars** (`-wal`/`-shm`/`-journal`) alongside a matched DB.
- **`--verify`** (#12) — SHA-256 the archive before & after (integrity attestation).
- **Multiple archives / directory** (#7) — search several archives (or a folder
  of them) in one run, tagging each result with its source archive.

## Done since v2

- **`--type` file-type filter** — header-first (then extension) format/category
  filter, reusing the inspector registry. One inspector per media format
  (`media` category); the media skip is now just `--type` excluding `media`.
- **`--match-path`** — apply the pattern to each file's internal path and list
  matching files (no content read); composes with `--export`/`--manifest`.
- **SQLite column type + BLOB dispatch** — `column [TYPE]` in output; a BLOB cell
  is classified by signature and, when recognised (e.g. an embedded `bplist`),
  resolved by that inspector (`blob_format` / `blob_context`).
- **`pull` → `export`** — the subcommand and `--pull` flag are now `export` /
  `--export` (the file-copying action; *extract* stays reserved for `--inspect`).

## v3 (later)

Everything else below remains for v3 — notably multi-pattern / IOC sweep
(`-e`/`-f`), tar / tar.gz inputs, nested archives, iOS GUID → app-name, presets,
UTF-16 literals, byte `--context`, SQLite row dump, summary report,
`--max-matches`, the execution log, ABX/SEGB inspectors (need sample files), and
the long-file refactor.

---

## Maintenance

- **Split long files thematically.** Files over ~400 lines as of v1:
  `inspect/sqlite.rs` (header / varint+record / b-tree walk / schema-columns),
  `inspect/plist.rs` (XML plist vs binary `bplist`), and `main.rs` (CLI arg
  structs / `run_search` / `run_export` / progress reporter). Split when next
  touched.

## Done in v1 (for reference)

- STORED + DEFLATE search; ZIP64; memory-mapped, SIMD, multi-threaded.
- Four offsets per match; txt / json / csv output; match highlighting.
- `--path` wildcard filter; live progress hint (terminal only).
- Deep inspection: TXT, JSON, XML, CSV, plist (XML + binary), SQLite.
- `export` with stable `<basename>_<hash>` layout, manifest, size cap, and
  manifest re-ingestion (`export --from-manifest`).
- `--count` (per-file match counts); output never raw-dumps binary content; hex
  offsets in txt; labelled `--inspect` tags with decoded SQLite cell values.
