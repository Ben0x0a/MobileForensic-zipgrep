//! Tests for the SQLite inspector against a real database fixture.
//!
//! Defines: tests that a match offset resolves to table/rowid/column for live
//! table-leaf cells, falls back to page+offset elsewhere, and that header-first
//! detection works on a misnamed file.
//! Uses: `mf_zipgrep::inspect` and the committed `fixtures/messages.sqlite`
//! (built with sqlite3: tables `messages(id,sender,body)` and
//! `notes(note_id,content)`).

use mf_zipgrep::inspect::inspect;

const DB: &[u8] = include_bytes!("fixtures/messages.sqlite");

/// Byte offset of the first occurrence of `needle` in the database.
fn at(needle: &str) -> usize {
    DB.windows(needle.len())
        .position(|w| w == needle.as_bytes())
        .expect("needle not found in fixture")
}

#[test]
fn resolves_text_value_to_table_rowid_column() {
    let insp = inspect("messages.sqlite", DB, at("UNIQUE_NEEDLE_42")).unwrap();
    assert_eq!(insp.format, "sqlite");
    assert_eq!(insp.detail["table"], "messages");
    assert_eq!(insp.detail["rowid"], 3);
    assert_eq!(insp.detail["column"], "body");
    // The decoded cell content is included (text-safe), not raw bytes.
    assert!(
        insp.detail["cell"]
            .as_str()
            .unwrap()
            .contains("UNIQUE_NEEDLE_42")
    );
}

#[test]
fn resolves_a_different_column() {
    let insp = inspect("messages.sqlite", DB, at("FINDSENDER")).unwrap();
    assert_eq!(insp.detail["table"], "messages");
    assert_eq!(insp.detail["rowid"], 2);
    assert_eq!(insp.detail["column"], "sender");
}

#[test]
fn resolves_a_second_table() {
    let insp = inspect("notes.sqlite", DB, at("MARKER_NOTE")).unwrap();
    assert_eq!(insp.detail["table"], "notes");
    assert_eq!(insp.detail["rowid"], 1);
    assert_eq!(insp.detail["column"], "content");
}

#[test]
fn non_cell_region_falls_back_to_page_and_offset() {
    // Offset 50 is inside the 100-byte database header on page 1.
    let insp = inspect("messages.sqlite", DB, 50).unwrap();
    assert_eq!(insp.format, "sqlite");
    assert_eq!(insp.detail["page"], 1);
    assert_eq!(insp.detail["page_offset"], 50);
    assert!(insp.detail.get("table").is_none());
}

#[test]
fn detection_uses_header_over_extension() {
    // Misnamed .txt, but the SQLite magic header wins.
    let insp = inspect("evidence.txt", DB, at("UNIQUE_NEEDLE_42")).unwrap();
    assert_eq!(insp.format, "sqlite");
    assert_eq!(insp.detail["table"], "messages");
}
