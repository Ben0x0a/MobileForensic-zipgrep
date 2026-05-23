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
- **Per-file hashes on pull**: record a SHA-256 of each pulled file in the
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
5. **Per-file match cap / counts** — `--count` (matches per file) and
   `--max-matches N` for fast triage and to bound output.

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
12. **`verify` subcommand** — given a manifest or an offset, re-read the archive
    and confirm the bytes still match (and re-hash), for independent checking.

---

## Done in v1 (for reference)

- STORED + DEFLATE search; ZIP64; memory-mapped, SIMD, multi-threaded.
- Four offsets per match; txt / json / csv output; match highlighting.
- `--path` wildcard filter; live progress hint (terminal only).
- Deep inspection: TXT, JSON, XML, CSV, plist (XML + binary), SQLite.
- `pull` with stable `<basename>_<hash>` layout, manifest, size cap, and
  manifest re-ingestion (`pull --from-manifest`).
