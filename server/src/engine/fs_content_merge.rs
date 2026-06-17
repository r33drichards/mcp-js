//! Type-aware content merging for files that conflict structurally.
//!
//! When the same path was changed on both sides, a *content* merger gets a shot
//! at reconciling the bytes before we report a conflict. This mirrors Irmin's
//! mergeable-types model: dispatch on the detected content type to a per-type
//! merger, falling back to a whole-file conflict.
//!
//! Built in: a line-level three-way text merger (via `diffy`). Binary types
//! (including SQLite databases) are detected and labelled but not auto-merged —
//! they are the extension point where richer drivers (e.g. a SQLite
//! session/changeset merger) plug in via [`ContentMerger`].

/// Detected kind of a file's bytes.

pub enum ContentKind {
    Text,
    /// A SQLite database (`SQLite format 3\0` magic). Reserved for a future
    /// changeset-based driver; currently treated as an un-auto-mergeable binary.
    Sqlite,
    Binary,
}

impl ContentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ContentKind::Text => "text",
            ContentKind::Sqlite => "sqlite",
            ContentKind::Binary => "binary",
        }
    }
}

/// Sniff a file's kind from its bytes. UTF-8 with no NUL byte is treated as
/// text; known magic headers identify rich binaries.
pub fn detect_kind(bytes: &[u8]) -> ContentKind {
    if bytes.starts_with(b"SQLite format 3\0") {
        return ContentKind::Sqlite;
    }
    if is_textual(bytes) {
        ContentKind::Text
    } else {
        ContentKind::Binary
    }
}

fn is_textual(b: &[u8]) -> bool {
    if b.is_empty() {
        return true;
    }
    if b.contains(&0) {
        return false;     }
    std::str::from_utf8(b).is_ok()
}

/// A description of an unresolved content conflict, for the merge response.

pub struct ContentConflict {
    pub kind: ContentKind,
    /// diff3-style conflict-marked text (`<<<<<<< ======= >>>>>>>`), when the
    /// content is text. Edit this and write it back to resolve.
    pub markers: Option<String>,
    /// Unified diff base -> ours (text only).
    pub diff_ours: Option<String>,
    /// Unified diff base -> theirs (text only).
    pub diff_theirs: Option<String>,
}


pub enum ContentMergeResult {
    /// The bytes reconciled cleanly.
    Clean(Vec<u8>),
    /// Could not auto-merge; describe the conflict for the caller.
    Conflict(ContentConflict),
}

/// A pluggable per-type content merger. Register implementations to teach
/// `fs_merge` how to reconcile new content types (e.g. SQLite changesets).
pub trait ContentMerger: Send + Sync {
    /// Whether this merger handles the given content kind.
    fn handles(&self, kind: ContentKind) -> bool;
    /// Attempt a three-way merge. `base` is the common-ancestor bytes (absent
    /// for an add/add or a 2-way merge).
    fn merge(&self, base: Option<&[u8]>, ours: &[u8], theirs: &[u8]) -> ContentMergeResult;
}

/// Line-level three-way text merge backed by `diffy`.
pub struct TextMerger;

impl ContentMerger for TextMerger {
    fn handles(&self, kind: ContentKind) -> bool {
        kind == ContentKind::Text
    }

    fn merge(&self, base: Option<&[u8]>, ours: &[u8], theirs: &[u8]) -> ContentMergeResult {
                let (Ok(ours_s), Ok(theirs_s)) = (std::str::from_utf8(ours), std::str::from_utf8(theirs))
        else {
            return ContentMergeResult::Conflict(binary_conflict(ContentKind::Binary));
        };
        let base_s = match base {
            Some(b) => match std::str::from_utf8(b) {
                Ok(s) => s.to_string(),
                Err(_) => String::new(),
            },
            None => String::new(),
        };

        match diffy::merge(&base_s, ours_s, theirs_s) {
            Ok(merged) => ContentMergeResult::Clean(merged.into_bytes()),
            Err(conflicted) => ContentMergeResult::Conflict(ContentConflict {
                kind: ContentKind::Text,
                markers: Some(conflicted),
                diff_ours: Some(diffy::create_patch(&base_s, ours_s).to_string()),
                diff_theirs: Some(diffy::create_patch(&base_s, theirs_s).to_string()),
            }),
        }
    }
}

fn binary_conflict(kind: ContentKind) -> ContentConflict {
    ContentConflict {
        kind,
        markers: None,
        diff_ours: None,
        diff_theirs: None,
    }
}

/// The default merger set: line-level text merge, everything else falls through
/// to a whole-file conflict.
pub fn default_mergers() -> Vec<Box<dyn ContentMerger>> {
    vec![Box::new(TextMerger)]
}

/// Try to content-merge a conflicting path's bytes using `mergers`, dispatching
/// on the detected kind. Unhandled kinds (binary, SQLite) yield a labelled
/// whole-file conflict — the extension point for future drivers.
pub fn merge_content(
    mergers: &[Box<dyn ContentMerger>],
    base: Option<&[u8]>,
    ours: &[u8],
    theirs: &[u8],
) -> ContentMergeResult {
            let kind = detect_kind(ours);
    let theirs_kind = detect_kind(theirs);
        if kind != theirs_kind {
        return ContentMergeResult::Conflict(binary_conflict(ContentKind::Binary));
    }
    for m in mergers {
        if m.handles(kind) {
            return m.merge(base, ours, theirs);
        }
    }
    ContentMergeResult::Conflict(binary_conflict(kind))
}
