#![no_main]
//! Robustness fuzzing of type-aware content merging (`fs_content_merge`):
//! content-kind sniffing and the line-level three-way text merge that runs on
//! arbitrary user file bytes whenever a merge hits a structural conflict.

use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::fs_content_merge::{
    default_mergers, detect_kind, merge_content, ContentKind, ContentMergeResult,
};

const MAX_LEN: usize = 8 * 1024;

#[derive(Arbitrary, Debug)]
struct Input {
    base: Option<Vec<u8>>,
    ours: Vec<u8>,
    theirs: Vec<u8>,
}

fn check_kind(bytes: &[u8]) -> ContentKind {
    let kind = detect_kind(bytes);
    if bytes.starts_with(b"SQLite format 3\0") {
        assert_eq!(kind, ContentKind::Sqlite);
    } else if bytes.contains(&0) {
        assert_eq!(kind, ContentKind::Binary, "NUL byte means binary");
    } else if std::str::from_utf8(bytes).is_ok() {
        assert_eq!(kind, ContentKind::Text, "NUL-free UTF-8 means text");
    } else {
        assert_eq!(kind, ContentKind::Binary);
    }
    // as_str must be total.
    assert!(!kind.as_str().is_empty());
    kind
}

fuzz_target!(|input: Input| {
    let mut base = input.base;
    if let Some(b) = &mut base {
        b.truncate(MAX_LEN);
    }
    let mut ours = input.ours;
    ours.truncate(MAX_LEN);
    let mut theirs = input.theirs;
    theirs.truncate(MAX_LEN);

    if let Some(b) = &base {
        check_kind(b);
    }
    let ours_kind = check_kind(&ours);
    let theirs_kind = check_kind(&theirs);

    let mergers = default_mergers();
    let result = merge_content(&mergers, base.as_deref(), &ours, &theirs);

    match result {
        ContentMergeResult::Clean(merged) => {
            // Only the text merger can produce a clean result, and a text
            // merge of UTF-8 inputs must yield UTF-8 output.
            assert_eq!(ours_kind, ContentKind::Text);
            assert_eq!(theirs_kind, ContentKind::Text);
            assert!(
                std::str::from_utf8(&merged).is_ok(),
                "clean text merge must produce valid UTF-8"
            );
        }
        ContentMergeResult::Conflict(c) => {
            if ours_kind != theirs_kind {
                // Sides disagreeing on kind is never auto-merged.
                assert_eq!(c.kind, ContentKind::Binary);
            }
            if c.kind == ContentKind::Text {
                // A text conflict must carry markers and both diffs.
                assert!(c.markers.is_some());
                assert!(c.diff_ours.is_some());
                assert!(c.diff_theirs.is_some());
            } else {
                assert!(c.markers.is_none());
            }
        }
    }
});
