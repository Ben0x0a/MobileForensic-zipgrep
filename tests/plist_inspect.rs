//! Tests for the plist inspector (XML plists and binary plists).
//!
//! Defines: tests that a match offset resolves to a dict-key / array-index path
//! in both encodings, and that detection is header-first.
//! Uses: `mf_zipgrep::inspect` and the committed `fixtures/sample.plist`
//! (XML) and `fixtures/sample.bplist` (binary, produced by `plutil`). Both
//! encode `{ Account: { Username: "XML_NEEDLE", Servers: ["first",
//! "ARRAY_NEEDLE"] } }`.

use mf_zipgrep::inspect::inspect;

const XML: &[u8] = include_bytes!("fixtures/sample.plist");
const BIN: &[u8] = include_bytes!("fixtures/sample.bplist");

fn at(hay: &[u8], needle: &str) -> usize {
    hay.windows(needle.len())
        .position(|w| w == needle.as_bytes())
        .expect("needle not found in fixture")
}

#[test]
fn xml_plist_resolves_nested_key_path() {
    let insp = inspect("sample.plist", XML, at(XML, "XML_NEEDLE")).unwrap();
    assert_eq!(insp.format, "plist");
    assert_eq!(insp.detail["path"], "$.Account.Username");
}

#[test]
fn xml_plist_resolves_array_index() {
    let insp = inspect("sample.plist", XML, at(XML, "ARRAY_NEEDLE")).unwrap();
    assert_eq!(insp.detail["path"], "$.Account.Servers[1]");
}

#[test]
fn binary_plist_resolves_nested_key_path() {
    let insp = inspect("sample.bplist", BIN, at(BIN, "XML_NEEDLE")).unwrap();
    assert_eq!(insp.format, "bplist");
    assert_eq!(insp.detail["path"], "$.Account.Username");
}

#[test]
fn binary_plist_resolves_array_index() {
    let insp = inspect("sample.bplist", BIN, at(BIN, "ARRAY_NEEDLE")).unwrap();
    assert_eq!(insp.format, "bplist");
    assert_eq!(insp.detail["path"], "$.Account.Servers[1]");
}

#[test]
fn binary_plist_detected_by_magic_despite_extension() {
    // Wrong extension, but the bplist00 magic wins.
    let insp = inspect("note.txt", BIN, at(BIN, "ARRAY_NEEDLE")).unwrap();
    assert_eq!(insp.format, "bplist");
}
