//! Integration tests for the ZIP central-directory parser.
//!
//! Defines: tests covering entry enumeration, exact data-offset resolution,
//! ZIP64 offset recovery, method detection (STORED/DEFLATE), skipping of
//! unsupported methods, and truncation errors.
//! Uses: `common` (fixture builder) and `mf_zipgrep::{zip, models}`.

mod common;

use common::{FileSpec, build_zip};
use mf_zipgrep::models::{Entry, Method};
use mf_zipgrep::zip::parse_entries;

/// The bytes the parser points at must be exactly the file's stored data.
fn entry_bytes<'a>(archive: &'a [u8], e: &Entry) -> &'a [u8] {
    &archive[e.data_offset as usize..(e.data_offset + e.data_len) as usize]
}

#[test]
fn enumerates_all_stored_entries() {
    let files = [
        FileSpec::stored("a.txt", b"hello"),
        FileSpec::stored("sub/b.log", b"world!!"),
    ];
    let zip = build_zip(&files, false);

    let entries = parse_entries(&zip).unwrap();

    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].name, "a.txt");
    assert_eq!(entries[1].name, "sub/b.log");
}

#[test]
fn resolves_data_offset_and_method() {
    let files = [
        FileSpec::stored("a.txt", b"hello"),
        FileSpec::deflate("z.bin", b"\x01\x02\x03", 9),
    ];
    let zip = build_zip(&files, false);

    let entries = parse_entries(&zip).unwrap();

    assert_eq!(entries[0].method, Method::Stored);
    assert_eq!(entry_bytes(&zip, &entries[0]), b"hello");
    assert_eq!(entries[1].method, Method::Deflate);
    assert_eq!(entries[1].uncompressed_size, 9);
    assert_eq!(entry_bytes(&zip, &entries[1]), b"\x01\x02\x03");
}

#[test]
fn skips_unsupported_methods() {
    let files = [
        FileSpec::stored("keep.txt", b"keep me"),
        // Method 99 (e.g. AES/other) is neither STORED nor DEFLATE -> skipped.
        FileSpec {
            name: "drop.bin",
            data: b"\x00\x00",
            method: 99,
            uncompressed_size: 2,
        },
    ];
    let zip = build_zip(&files, false);

    let entries = parse_entries(&zip).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].name, "keep.txt");
}

#[test]
fn recovers_zip64_local_offset() {
    let files = [FileSpec::stored("big.bin", b"ZIP64 payload")];
    let zip = build_zip(&files, true);

    let entries = parse_entries(&zip).unwrap();

    assert_eq!(entries.len(), 1);
    assert_eq!(entry_bytes(&zip, &entries[0]), b"ZIP64 payload");
}

#[test]
fn errors_on_truncated_archive() {
    let files = [FileSpec::stored("a.txt", b"hello")];
    let zip = build_zip(&files, false);
    // Drop the EOCD so the archive can no longer be located.
    let truncated = &zip[..zip.len() - 10];

    assert!(parse_entries(truncated).is_err());
}
