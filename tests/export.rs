//! Tests for export: planning, manifest, and pulling to disk.
//!
//! Defines: tests for folder naming (basename + stable path hash), manifest
//! content + total size, pull layout/content, and the --max-size refusal.
//! Uses: `common` (fixture builder), `mf_zipgrep::{engine, export}`,
//! `serde_json`, `tempfile`.

mod common;

use std::fs;

use common::{FileSpec, build_zip};
use mf_zipgrep::engine::search_archive;
use mf_zipgrep::export::{self, PullOutcome};
use mf_zipgrep::filter::PathFilter;
use regex::bytes::Regex;

/// Build a two-file archive (same basename, different dirs) and search it.
fn findings_two_infoplists() -> (Vec<u8>, mf_zipgrep::engine::Findings) {
    let files = [
        FileSpec::stored("AppA/Info.plist", b"key TARGET one"),
        FileSpec::stored("AppB/Info.plist", b"key TARGET two"),
    ];
    let zip = build_zip(&files, false);
    let findings = search_archive(
        &zip,
        &Regex::new("TARGET").unwrap(),
        false,
        &PathFilter::new(&[]),
    )
    .unwrap();
    (zip, findings)
}

#[test]
fn plan_names_folders_by_basename_and_stable_hash() {
    let (_zip, findings) = findings_two_infoplists();
    let plan = export::plan(&findings.files);

    assert_eq!(plan.items.len(), 2);
    for item in &plan.items {
        // <basename>_<10 hex>
        assert!(item.folder.starts_with("Info.plist_"));
        let hash = item.folder.strip_prefix("Info.plist_").unwrap();
        assert_eq!(hash.len(), 10);
        assert!(hash.bytes().all(|b| b.is_ascii_hexdigit()));
    }
    // Different internal paths -> different folders (no collision).
    assert_ne!(plan.items[0].folder, plan.items[1].folder);
    // total size is the sum of the two files.
    assert_eq!(plan.total_size, 14 + 14);
}

#[test]
fn hash_is_stable_across_runs() {
    let (_z1, f1) = findings_two_infoplists();
    let (_z2, f2) = findings_two_infoplists();
    let p1 = export::plan(&f1.files);
    let p2 = export::plan(&f2.files);
    // Same internal path => same folder name every time (recognisable).
    assert_eq!(p1.items[0].folder, p2.items[0].folder);
}

#[test]
fn manifest_lists_files_with_total_size() {
    let (_zip, findings) = findings_two_infoplists();
    let plan = export::plan(&findings.files);

    let mut buf = Vec::new();
    export::write_manifest(&plan, &mut buf).unwrap();
    let json: serde_json::Value = serde_json::from_slice(&buf).unwrap();

    assert_eq!(json["file_count"], 2);
    assert_eq!(json["total_size"], 28);
    let first = &json["files"][0];
    assert_eq!(first["internal_path"], "AppA/Info.plist");
    assert!(
        first["output_path"]
            .as_str()
            .unwrap()
            .ends_with("/Info.plist")
    );
    assert_eq!(first["offsets"][0], 4); // "key " precedes TARGET
}

#[test]
fn pull_writes_files_under_their_folders() {
    let (zip, findings) = findings_two_infoplists();
    let plan = export::plan(&findings.files);
    let dir = tempfile::tempdir().unwrap();

    let outcome = export::pull(&plan, &zip, &findings.files, dir.path(), None).unwrap();
    match outcome {
        PullOutcome::Pulled { files, .. } => assert_eq!(files, 2),
        PullOutcome::Refused { .. } => panic!("should not refuse without a cap"),
    }

    // Each file lands at DIR/<folder>/Info.plist with its real content.
    for item in &plan.items {
        let path = dir.path().join(&item.folder).join("Info.plist");
        let content = fs::read(&path).unwrap();
        assert!(content.starts_with(b"key TARGET"));
    }
}

#[test]
fn pull_refuses_when_over_max_size() {
    let (zip, findings) = findings_two_infoplists();
    let plan = export::plan(&findings.files);
    let dir = tempfile::tempdir().unwrap();

    // Cap below the 28-byte total.
    let outcome = export::pull(&plan, &zip, &findings.files, dir.path(), Some(10)).unwrap();
    match outcome {
        PullOutcome::Refused { total_size, cap } => {
            assert_eq!(total_size, 28);
            assert_eq!(cap, 10);
        }
        PullOutcome::Pulled { .. } => panic!("should have refused"),
    }
    // Nothing was written.
    assert_eq!(fs::read_dir(dir.path()).unwrap().count(), 0);
}

#[test]
fn pull_from_manifest_round_trips() {
    let (zip, findings) = findings_two_infoplists();
    let plan = export::plan(&findings.files);

    // Write the manifest, then read it back and pull from it.
    let mut buf = Vec::new();
    export::write_manifest(&plan, &mut buf).unwrap();
    let manifest = export::read_manifest(&buf[..]).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let outcome = export::pull_from_manifest(&manifest, &zip, dir.path(), None).unwrap();
    match outcome {
        PullOutcome::Pulled { files, skipped, .. } => {
            assert_eq!(files, 2);
            assert_eq!(skipped, 0);
        }
        PullOutcome::Refused { .. } => panic!("should not refuse without a cap"),
    }

    // Each file exists at the path the manifest recorded.
    for entry in &manifest.files {
        let mut path = dir.path().to_path_buf();
        for seg in entry.output_path.split('/') {
            path.push(seg);
        }
        assert!(path.exists(), "missing {}", path.display());
    }
}

#[test]
fn pull_from_manifest_skips_missing_entries() {
    let (zip, _findings) = findings_two_infoplists();
    let manifest = export::Manifest {
        total_size: 5,
        file_count: 1,
        files: vec![export::ManifestEntry {
            internal_path: "does/not/exist".into(),
            output_path: "ghost_0000000000/exist".into(),
            size: 5,
            compressed: false,
            offsets: vec![],
        }],
    };
    let dir = tempfile::tempdir().unwrap();

    let outcome = export::pull_from_manifest(&manifest, &zip, dir.path(), None).unwrap();
    match outcome {
        PullOutcome::Pulled { files, skipped, .. } => {
            assert_eq!(files, 0);
            assert_eq!(skipped, 1);
        }
        PullOutcome::Refused { .. } => panic!("should not refuse"),
    }
}
