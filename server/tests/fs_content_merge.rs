//! Type-aware content merge: detection + line-level text 3-way (diffy).

use server::engine::fs_content_merge::{
    default_mergers, detect_kind, merge_content, ContentKind, ContentMergeResult,
};

fn merge(base: Option<&[u8]>, ours: &[u8], theirs: &[u8]) -> ContentMergeResult {
    merge_content(&default_mergers(), base, ours, theirs)
}

#[test]
fn detects_text_binary_and_sqlite() {
    assert_eq!(detect_kind(b"hello\nworld\n"), ContentKind::Text);
    assert_eq!(detect_kind(b""), ContentKind::Text);
    assert_eq!(detect_kind(&[0x00, 0x01, 0x02, 0xff]), ContentKind::Binary);
    assert_eq!(detect_kind(b"SQLite format 3\0rest..."), ContentKind::Sqlite);
}

#[test]
fn text_edits_to_different_lines_merge_cleanly() {
    let base = b"line1\nline2\nline3\n";
    let ours = b"OURS\nline2\nline3\n"; // changed line 1
    let theirs = b"line1\nline2\nTHEIRS\n"; // changed line 3
    match merge(Some(base), ours, theirs) {
        ContentMergeResult::Clean(bytes) => {
            let s = String::from_utf8(bytes).unwrap();
            assert_eq!(s, "OURS\nline2\nTHEIRS\n");
        }
        ContentMergeResult::Conflict(c) => panic!("expected clean text merge, got {c:?}"),
    }
}

#[test]
fn text_edits_to_same_line_conflict_with_markers_and_diffs() {
    let base = b"line1\nline2\nline3\n";
    let ours = b"line1\nOURS\nline3\n";
    let theirs = b"line1\nTHEIRS\nline3\n";
    match merge(Some(base), ours, theirs) {
        ContentMergeResult::Conflict(c) => {
            assert_eq!(c.kind, ContentKind::Text);
            let markers = c.markers.expect("text conflict carries markers");
            assert!(markers.contains("OURS") && markers.contains("THEIRS"));
            assert!(markers.contains("<<<<<<<") && markers.contains(">>>>>>>"));
            assert!(c.diff_ours.unwrap().contains("OURS"));
            assert!(c.diff_theirs.unwrap().contains("THEIRS"));
        }
        ContentMergeResult::Clean(_) => panic!("expected a same-line conflict"),
    }
}

#[test]
fn binary_content_is_not_auto_merged() {
    let base: &[u8] = &[0u8, 1, 2, 3];
    let ours: &[u8] = &[0u8, 9, 9, 9];
    let theirs: &[u8] = &[0u8, 8, 8, 8];
    match merge(Some(base), ours, theirs) {
        ContentMergeResult::Conflict(c) => {
            assert_eq!(c.kind, ContentKind::Binary);
            assert!(c.markers.is_none());
        }
        ContentMergeResult::Clean(_) => panic!("binary must not auto-merge"),
    }
}

#[test]
fn sqlite_is_detected_but_left_as_conflict_extension_point() {
    let mut ours = b"SQLite format 3\0".to_vec();
    ours.extend_from_slice(&[1, 2, 3]);
    let mut theirs = b"SQLite format 3\0".to_vec();
    theirs.extend_from_slice(&[4, 5, 6]);
    match merge(None, &ours, &theirs) {
        ContentMergeResult::Conflict(c) => assert_eq!(c.kind, ContentKind::Sqlite),
        ContentMergeResult::Clean(_) => panic!("no built-in sqlite merger yet"),
    }
}
