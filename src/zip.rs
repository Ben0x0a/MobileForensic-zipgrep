//! ZIP central-directory parser (STORED + DEFLATE entries, with ZIP64 support).
//!
//! Defines: `parse_entries`, which reads a ZIP archive's Central Directory and
//! resolves, for every searchable file, the byte range of its data inside the
//! archive along with its compression method.
//! Used by: `main.rs` (orchestration) — the returned ranges feed `search.rs`.
//! Uses: `crate::models::{Entry, Method}` (data containers) and `anyhow` (error
//! context). All integer decoding is done by hand so the parsing flow stays
//! explicit and learnable.
//!
//! Why CD-first instead of a blind linear scan: the Central Directory is the
//! authoritative record of every entry (local headers may carry zeroed sizes
//! when a data descriptor is used). Parsing it first gives exact data ranges,
//! lets us skip header bytes, and tells us each entry's method.

use anyhow::{Context, Result, bail, ensure};

use crate::models::{Entry, Method};

// --- ZIP record signatures (little-endian on disk) ---------------------------
// These are fixed by the ZIP specification (APPNOTE.TXT), not operator-tunable,
// so they live here as format constants rather than in any config.
const SIG_EOCD: u32 = 0x0605_4b50; // End Of Central Directory
const SIG_ZIP64_EOCD_LOCATOR: u32 = 0x0706_4b50; // ZIP64 EOCD locator
const SIG_ZIP64_EOCD: u32 = 0x0606_4b50; // ZIP64 EOCD record
const SIG_CDFH: u32 = 0x0201_4b50; // Central Directory File Header
const SIG_LFH: u32 = 0x0403_4b50; // Local File Header

const METHOD_STORED: u16 = 0; // compression method 0 = no compression
const METHOD_DEFLATE: u16 = 8; // compression method 8 = DEFLATE
const ZIP64_EXTRA_ID: u16 = 0x0001; // header id of the ZIP64 extended-info field

/// Outcome of decoding one Central Directory header.
///
/// Modelled as an enum (rather than `Option<Entry>`) because "this entry uses a
/// method we do not search" is an *expected* outcome, not an error: only STORED
/// and DEFLATE are kept, and a named variant says so at the call site.
enum CdScan {
    Searchable(Entry),
    Skipped,
}

/// Parse the archive and return every searchable (STORED or DEFLATE) entry.
///
/// HOW:
///   1. locate the EOCD at the tail of the file,
///   2. follow the ZIP64 records if the EOCD fields are saturated,
///   3. walk the Central Directory, decoding one header per entry,
///   4. keep the STORED and DEFLATE entries.
pub fn parse_entries(data: &[u8]) -> Result<Vec<Entry>> {
    let eocd = find_eocd(data).context("could not locate End Of Central Directory record")?;

    // Central-directory location/count, possibly upgraded by ZIP64 below.
    let mut cd_offset = read_u32(data, eocd + 16)? as u64;
    let mut total_entries = read_u16(data, eocd + 10)? as u64;

    // A saturated (all-ones) field means the real value lives in the ZIP64
    // records; reading the 32-bit field as the truth would point us at the
    // wrong offset and corrupt the whole parse, so we must redirect.
    let cd_offset_saturated = read_u32(data, eocd + 16)? == u32::MAX;
    let entries_saturated = read_u16(data, eocd + 10)? == u16::MAX;
    if cd_offset_saturated || entries_saturated {
        let z = read_zip64_eocd(data, eocd)?;
        cd_offset = z.cd_offset;
        total_entries = z.total_entries;
    }

    let mut entries = Vec::new();
    let mut pos = cd_offset as usize;
    for _ in 0..total_entries {
        let (scan, next) = parse_cd_header(data, pos)?;
        if let CdScan::Searchable(entry) = scan {
            entries.push(entry);
        }
        pos = next;
    }

    Ok(entries)
}

/// Resolved ZIP64 central-directory location.
struct Zip64Eocd {
    cd_offset: u64,
    total_entries: u64,
}

/// Read the ZIP64 EOCD via the locator that sits 20 bytes before the EOCD.
///
/// HOW: the 20-byte locator immediately precedes the EOCD and stores the
/// absolute offset of the ZIP64 EOCD record, which in turn holds the real
/// 64-bit central-directory offset and entry count.
fn read_zip64_eocd(data: &[u8], eocd: usize) -> Result<Zip64Eocd> {
    // If the EOCD claimed ZIP64 but no locator fits before it, the archive is
    // malformed and any offset we compute would be garbage — fail loudly.
    let locator = eocd
        .checked_sub(20)
        .context("file too small for a ZIP64 EOCD locator")?;
    ensure!(
        read_u32(data, locator)? == SIG_ZIP64_EOCD_LOCATOR,
        "expected ZIP64 EOCD locator signature before EOCD"
    );

    let z64 = read_u64(data, locator + 8)? as usize; // offset of ZIP64 EOCD record
    ensure!(
        read_u32(data, z64)? == SIG_ZIP64_EOCD,
        "expected ZIP64 EOCD record signature"
    );

    Ok(Zip64Eocd {
        total_entries: read_u64(data, z64 + 32)?,
        cd_offset: read_u64(data, z64 + 48)?,
    })
}

/// Parse one Central Directory File Header at `pos`.
///
/// Returns the scan outcome plus the offset of the next header, so the caller
/// can keep walking the directory regardless of whether this entry was kept.
fn parse_cd_header(data: &[u8], pos: usize) -> Result<(CdScan, usize)> {
    // A wrong signature here means we have lost sync with the directory; every
    // subsequent field would be misread, so stop rather than emit junk.
    ensure!(
        read_u32(data, pos)? == SIG_CDFH,
        "bad central-directory header signature at offset {pos}"
    );

    let method_code = read_u16(data, pos + 10)?;
    let mut comp_size = read_u32(data, pos + 20)? as u64;
    let mut uncomp_size = read_u32(data, pos + 24)? as u64;
    let name_len = read_u16(data, pos + 28)? as usize;
    let extra_len = read_u16(data, pos + 30)? as usize;
    let comment_len = read_u16(data, pos + 32)? as usize;
    let mut local_offset = read_u32(data, pos + 42)? as u64;

    // HOW: the variable-length name/extra/comment fields follow the 46-byte
    // fixed header in that order; summing them gives the next header's offset.
    let name_start = pos + 46;
    let extra_start = name_start + name_len;
    let comment_start = extra_start + extra_len;
    let next = comment_start + comment_len;

    // Saturated 32-bit fields are carried in the ZIP64 extra field (id 0x0001),
    // in a fixed order: uncompressed, compressed, local-offset, disk. We must
    // read the real values from there or we would scan the wrong byte range.
    let uncomp_saturated = uncomp_size == u32::MAX as u64;
    let comp_saturated = comp_size == u32::MAX as u64;
    let offset_saturated = local_offset == u32::MAX as u64;
    if uncomp_saturated || comp_saturated || offset_saturated {
        let z = read_zip64_extra(
            data,
            extra_start,
            extra_len,
            uncomp_saturated,
            comp_saturated,
            offset_saturated,
        )?;
        if let Some(v) = z.uncompressed {
            uncomp_size = v;
        }
        if let Some(v) = z.compressed {
            comp_size = v;
        }
        if let Some(v) = z.local_offset {
            local_offset = v;
        }
    }

    // Only STORED and DEFLATE are searchable; any other method is skipped.
    let method = match method_code {
        METHOD_STORED => Method::Stored,
        METHOD_DEFLATE => Method::Deflate,
        _ => return Ok((CdScan::Skipped, next)),
    };

    let name = String::from_utf8_lossy(slice(data, name_start, name_len)?).into_owned();
    let data_offset = local_data_offset(data, local_offset)?;

    Ok((
        CdScan::Searchable(Entry {
            name,
            method,
            data_offset,
            data_len: comp_size,
            uncompressed_size: uncomp_size,
        }),
        next,
    ))
}

/// Values recovered from a ZIP64 extended-information extra field.
struct Zip64Extra {
    uncompressed: Option<u64>,
    compressed: Option<u64>,
    local_offset: Option<u64>,
}

/// Decode the ZIP64 extra field (header id 0x0001) within an entry's extra
/// area.
///
/// HOW: the extra area is a sequence of `(id: u16, size: u16, body)` blocks; we
/// walk it until we find id 0x0001, then read the 8-byte fields that are
/// present. A field is present only when its 32-bit counterpart was saturated,
/// always in the order uncompressed, compressed, local-offset — so we skip or
/// read each in turn driven by the `want_*` flags.
fn read_zip64_extra(
    data: &[u8],
    extra_start: usize,
    extra_len: usize,
    want_uncomp: bool,
    want_comp: bool,
    want_offset: bool,
) -> Result<Zip64Extra> {
    let mut p = extra_start;
    let end = extra_start + extra_len;
    while p + 4 <= end {
        let id = read_u16(data, p)?;
        let size = read_u16(data, p + 2)? as usize;
        let body = p + 4;
        if id == ZIP64_EXTRA_ID {
            let mut q = body;
            let uncompressed = if want_uncomp {
                let v = read_u64(data, q)?;
                q += 8;
                Some(v)
            } else {
                None
            };
            let compressed = if want_comp {
                let v = read_u64(data, q)?;
                q += 8;
                Some(v)
            } else {
                None
            };
            let local_offset = if want_offset {
                Some(read_u64(data, q)?)
            } else {
                None
            };
            return Ok(Zip64Extra {
                uncompressed,
                compressed,
                local_offset,
            });
        }
        p = body + size;
    }
    // We only call this when a field was saturated, so a missing 0x0001 block
    // means the archive contradicts itself; refuse rather than guess an offset.
    bail!("ZIP64 extra field expected but not found");
}

/// Compute where an entry's data actually starts.
///
/// WHY this reads the Local File Header rather than reusing the Central
/// Directory's extra length: the LFH carries its *own* name/extra lengths,
/// which may differ from the CD's. Using the CD's extra length here is a
/// classic ZIP-parsing bug that lands the data offset in the wrong place.
fn local_data_offset(data: &[u8], local_offset: u64) -> Result<u64> {
    let p = local_offset as usize;
    ensure!(
        read_u32(data, p)? == SIG_LFH,
        "bad local file header signature at offset {p}"
    );
    let name_len = read_u16(data, p + 26)? as u64;
    let extra_len = read_u16(data, p + 28)? as u64;
    Ok(local_offset + 30 + name_len + extra_len)
}

/// Scan backwards from the file tail for the EOCD signature.
///
/// HOW: the EOCD can be followed by a comment of up to 65535 bytes, so we search
/// the last (22 + 65535) bytes from the end and, on each candidate, confirm the
/// stored comment length is consistent with the distance to end-of-file.
/// WHY the consistency check: the signature bytes can legitimately appear
/// inside a comment; the length check rejects those false positives.
fn find_eocd(data: &[u8]) -> Result<usize> {
    const EOCD_MIN: usize = 22;
    ensure!(data.len() >= EOCD_MIN, "file smaller than an EOCD record");

    let max_back = (EOCD_MIN + u16::MAX as usize).min(data.len());
    let start = data.len() - max_back;
    for pos in (start..=data.len() - EOCD_MIN).rev() {
        if read_u32(data, pos)? == SIG_EOCD {
            let comment_len = read_u16(data, pos + 20)? as usize;
            if pos + EOCD_MIN + comment_len == data.len() {
                return Ok(pos);
            }
        }
    }
    bail!("no EOCD signature found in tail of file");
}

// --- Bounds-checked little-endian readers ------------------------------------
// Every multi-byte read goes through these so an out-of-bounds offset becomes a
// clean error instead of a panic — important when parsing untrusted evidence.

fn slice(data: &[u8], off: usize, len: usize) -> Result<&[u8]> {
    data.get(off..off + len)
        .with_context(|| format!("read of {len} bytes at offset {off} is out of bounds"))
}

fn read_u16(data: &[u8], off: usize) -> Result<u16> {
    let b = slice(data, off, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

fn read_u32(data: &[u8], off: usize) -> Result<u32> {
    let b = slice(data, off, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

fn read_u64(data: &[u8], off: usize) -> Result<u64> {
    let b = slice(data, off, 8)?;
    Ok(u64::from_le_bytes([
        b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
    ]))
}
