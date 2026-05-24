//! Integration tests for result formatting (txt / json / csv).
//!
//! Defines: tests asserting each format's content, plus colour behaviour for
//! txt. Builds `MatchRecord`s directly (no archive needed) so the formatting is
//! tested in isolation from parsing/searching.
//! Uses: `mf_zipgrep::{models, output}`, `serde_json` (to parse JSON back).

use mf_zipgrep::models::{Inspection, MatchRecord};
use mf_zipgrep::output::{OutputFormat, write_counts, write_results};
use serde_json::json;

/// A match whose surrounding bytes are binary (contain a NUL).
fn binary_record() -> Vec<MatchRecord> {
    vec![MatchRecord {
        archive: None,
        path: "db.sqlite".into(),
        file_start: 0,
        file_offset: 4096, // 0x1000
        archive_offset: 4096,
        compressed: false,
        line: b"\x00\x01record\x00data".to_vec(),
        match_in_line: 2..8,
        inspection: None,
    }]
}

fn sample() -> Vec<MatchRecord> {
    vec![
        MatchRecord {
            archive: None,
            path: "sub/b.log".into(),
            file_start: 100,
            file_offset: 17,
            archive_offset: 117,
            compressed: false,
            line: b"another SECRET line".to_vec(),
            match_in_line: 8..14,
            inspection: None,
        },
        MatchRecord {
            archive: None,
            path: "a.txt".into(),
            file_start: 200,
            file_offset: 12,
            archive_offset: 212,
            compressed: false,
            line: b"secret token: ABC".to_vec(),
            match_in_line: 0..6,
            inspection: None,
        },
    ]
}

fn render(records: &[MatchRecord], format: OutputFormat, colourise: bool) -> String {
    let mut buf = Vec::new();
    write_results(records, format, colourise, false, &mut buf).unwrap();
    String::from_utf8(buf).unwrap()
}

#[test]
fn txt_lists_path_offset_and_textual_line() {
    let out = render(&sample(), OutputFormat::Txt, false);

    // path:0x<file_offset hex>:<line> for textual content (0x11 = 17, 0xc = 12).
    let lines: Vec<&str> = out.lines().collect();
    assert_eq!(lines[0], "sub/b.log:0x11:another SECRET line");
    assert_eq!(lines[1], "a.txt:0xc:secret token: ABC");
}

#[test]
fn txt_colourises_only_the_match() {
    let out = render(&sample(), OutputFormat::Txt, true);

    // The matched substring is wrapped in bold-red ON / reset OFF escapes.
    assert!(out.contains("\x1b[1;31mSECRET\x1b[0m"));
    // Non-matching text is untouched.
    assert!(out.contains("another \x1b[1;31mSECRET\x1b[0m line"));
}

#[test]
fn json_round_trips_all_fields() {
    let out = render(&sample(), OutputFormat::Json, false);

    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = &parsed[0];
    assert_eq!(first["path"], "sub/b.log");
    assert_eq!(first["file_start"], 100);
    assert_eq!(first["file_offset"], 17);
    assert_eq!(first["archive_offset"], 117);
    assert_eq!(first["compressed"], false);
    assert_eq!(first["line"], "another SECRET line");
}

#[test]
fn csv_has_header_and_rows() {
    let out = render(&sample(), OutputFormat::Csv, false);

    let mut lines = out.lines();
    assert_eq!(
        lines.next().unwrap(),
        "archive,path,file_start,file_offset,archive_offset,compressed,format,context,line"
    );
    // Single archive => empty archive column; format/context empty without inspect.
    assert_eq!(
        lines.next().unwrap(),
        ",sub/b.log,100,17,117,false,,,another SECRET line"
    );
}

#[test]
fn txt_shows_hex_file_offset_for_compressed() {
    let records = vec![MatchRecord {
        archive: None,
        path: "c.bin".into(),
        file_start: 50,
        file_offset: 10,
        archive_offset: 50, // blob start for a DEFLATE entry
        compressed: true,
        line: b"NEEDLE inside".to_vec(),
        match_in_line: 0..6,
        inspection: None,
    }];

    let out = render(&records, OutputFormat::Txt, false);

    // txt shows the (hex) offset within the decompressed file; the compressed
    // flag and blob start live in the json/csv `compressed`/`archive_offset`.
    assert_eq!(out.lines().next().unwrap(), "c.bin:0xa:NEEDLE inside");
}

fn inspected() -> Vec<MatchRecord> {
    vec![MatchRecord {
        archive: None,
        path: "a.txt".into(),
        file_start: 0,
        file_offset: 12,
        archive_offset: 12,
        compressed: false,
        line: b"has TARGET here".to_vec(),
        match_in_line: 4..10,
        inspection: Some(Inspection {
            format: "txt".into(),
            summary: "line 2, col 5".into(),
            detail: json!({ "line": 2, "col": 5 }),
        }),
    }]
}

#[test]
fn txt_appends_inspection_summary() {
    let out = render(&inspected(), OutputFormat::Txt, false);

    assert_eq!(
        out.lines().next().unwrap(),
        "a.txt:0xc:has TARGET here  [txt  line 2, col 5]"
    );
}

#[test]
fn json_nests_inspection_context() {
    let out = render(&inspected(), OutputFormat::Json, false);

    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    let first = &parsed[0];
    assert_eq!(first["format"], "txt");
    assert_eq!(first["context"]["line"], 2);
    assert_eq!(first["context"]["col"], 5);
}

#[test]
fn csv_fills_format_and_context_columns() {
    let out = render(&inspected(), OutputFormat::Csv, false);

    let row = out.lines().nth(1).unwrap();
    // ...,compressed,format,context,line
    assert!(row.contains(",txt,\"line 2, col 5\","));
}

#[test]
fn txt_suppresses_binary_line() {
    let out = render(&binary_record(), OutputFormat::Txt, false);
    // Binary content is never dumped: only path:0x<offset> (0x1000 = 4096).
    assert_eq!(out.lines().next().unwrap(), "db.sqlite:0x1000");
}

#[test]
fn json_omits_line_for_binary() {
    let out = render(&binary_record(), OutputFormat::Json, false);
    let parsed: serde_json::Value = serde_json::from_str(&out).unwrap();
    assert!(parsed[0].get("line").is_none()); // no binary content
    assert_eq!(parsed[0]["file_offset"], 4096); // location still reported
}

#[test]
fn count_writes_one_line_per_file() {
    let mut buf = Vec::new();
    write_counts(
        &[("a/x.db", 3), ("b/y.txt", 1)],
        OutputFormat::Txt,
        &mut buf,
    )
    .unwrap();
    let out = String::from_utf8(buf).unwrap();
    assert_eq!(
        out.lines().collect::<Vec<_>>(),
        vec!["a/x.db:3", "b/y.txt:1"]
    );
}

#[test]
fn tags_source_archive_when_set() {
    let mut recs = sample();
    for r in &mut recs {
        r.archive = Some("case.zip".into());
    }

    // txt joins the archive to the path like a folder.
    let txt = render(&recs, OutputFormat::Txt, false);
    assert!(
        txt.lines()
            .next()
            .unwrap()
            .starts_with("case.zip/sub/b.log:0x11:")
    );

    // json gains an `archive` field.
    let json = render(&recs, OutputFormat::Json, false);
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed[0]["archive"], "case.zip");

    // csv populates the leading archive column.
    let csv = render(&recs, OutputFormat::Csv, false);
    assert!(
        csv.lines()
            .nth(1)
            .unwrap()
            .starts_with("case.zip,sub/b.log,")
    );
}

#[test]
fn format_parses_from_str() {
    assert_eq!("json".parse::<OutputFormat>().unwrap(), OutputFormat::Json);
    assert_eq!("CSV".parse::<OutputFormat>().unwrap(), OutputFormat::Csv);
    assert!("yaml".parse::<OutputFormat>().is_err());
}
