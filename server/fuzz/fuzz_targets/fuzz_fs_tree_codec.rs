#![no_main]
use arbitrary::Arbitrary;
use libfuzzer_sys::fuzz_target;
use server::engine::fs_store::{chunk_key, Content, Entry, Manifest};
use server::engine::fs_tree::{components_of, path_of, tree_key, TreeChild, TreeNode, TREE_PREFIX};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Arbitrary, Debug)]
struct ArbEntry {
    mode: u32,
    size: u64,
    inline: Option<Vec<u8>>,
    chunk_hashes: Vec<[u8; 32]>,
    symlink: Option<String>,
}

impl ArbEntry {
    fn build(self) -> Entry {
        let content = match self.inline {
            Some(mut b) => {
                b.truncate(64 * 1024);
                Content::Inline(b)
            }
            None => {
                let mut hashes = self.chunk_hashes;
                hashes.truncate(1024);
                Content::Chunks(hashes)
            }
        };
        Entry {
            mode: self.mode,
            size: self.size,
            content,
            symlink: self.symlink.map(|s| PathBuf::from(bound(&s, 256))),
        }
    }
}

/// Bound an arbitrary string (char-safe) so round-trip inputs stay focused on
/// codec correctness rather than allocation size.
fn bound(s: &str, max_chars: usize) -> String {
    s.chars().take(max_chars).collect()
}

#[derive(Arbitrary, Debug)]
struct ArbChild {
    file: Option<ArbEntry>,
    dir: Option<[u8; 32]>,
}

#[derive(Arbitrary, Debug)]
enum CodecInput {
    /// Arbitrary blob bytes decoded as a tree node — exactly what
    /// `FsStore::get_node` does with data from the blob backend, which may be
    /// corrupted. Must error gracefully, never panic or overallocate.
    DecodeTreeNode(Vec<u8>),
    /// Same for a bare `Entry` and a flat `Manifest`.
    DecodeEntry(Vec<u8>),
    DecodeManifest(Vec<u8>),
    /// Structured round-trip: encode -> decode must be the identity, and the
    /// encoding must be deterministic (content ids depend on it).
    RoundTripTree(Vec<(String, ArbChild)>),
    RoundTripManifest(Vec<(String, ArbEntry)>),
    /// Path normalization invariants shared by the mount and the tree.
    NormalizePath(String),
    /// Blob key formatting for tree nodes and chunks.
    KeyFormat([u8; 32]),
}

/// Bound raw decode inputs for local runs with a large -max_len (CI caps
/// inputs at 65536 anyway). Deliberately no bincode size-limit options here:
/// production (`FsStore::get_node`) decodes blob bytes with a plain
/// `bincode::deserialize`, whose slice reader bounds-checks length prefixes
/// against the remaining input before allocating — the target must exercise
/// exactly that configuration.
const MAX_RAW: usize = 1024 * 1024;

fuzz_target!(|input: CodecInput| {
    match input {
        CodecInput::DecodeTreeNode(mut bytes) => {
            bytes.truncate(MAX_RAW);
            let _ = bincode::deserialize::<TreeNode>(&bytes);
        }
        CodecInput::DecodeEntry(mut bytes) => {
            bytes.truncate(MAX_RAW);
            let _ = bincode::deserialize::<Entry>(&bytes);
        }
        CodecInput::DecodeManifest(mut bytes) => {
            bytes.truncate(MAX_RAW);
            let _ = bincode::deserialize::<Manifest>(&bytes);
        }
        CodecInput::RoundTripTree(children) => {
            let mut node = TreeNode::default();
            for (name, child) in children.into_iter().take(32) {
                node.children.insert(
                    bound(&name, 64),
                    TreeChild {
                        file: child.file.map(ArbEntry::build),
                        dir: child.dir,
                    },
                );
            }
            let bytes = bincode::serialize(&node).expect("serialize tree node");
            let again = bincode::serialize(&node).expect("serialize tree node twice");
            assert_eq!(bytes, again, "tree node encoding must be deterministic");
            let back: TreeNode = bincode::deserialize(&bytes).expect("decode own encoding");
            assert_eq!(back, node, "tree node round-trip");
        }
        CodecInput::RoundTripManifest(files) => {
            let mut entries = BTreeMap::new();
            for (path, entry) in files.into_iter().take(32) {
                entries.insert(PathBuf::from(bound(&path, 256)), entry.build());
            }
            let m = Manifest { entries };
            let bytes = bincode::serialize(&m).expect("serialize manifest");
            let back: Manifest = bincode::deserialize(&bytes).expect("decode own encoding");
            assert_eq!(back, m, "manifest round-trip");
        }
        CodecInput::NormalizePath(s) => {
            let comps = components_of(Path::new(&s));
            for c in &comps {
                assert!(!c.is_empty(), "normalized component must not be empty");
                assert!(
                    c.as_str() != "." && c.as_str() != "..",
                    "normalized component must be plain"
                );
                assert!(!c.contains('/'), "normalized component must be a single name");
            }
            // Normalization must be idempotent: re-parsing the rebuilt path
            // yields the same components.
            let rebuilt = path_of(&comps);
            assert_eq!(
                components_of(&rebuilt),
                comps,
                "components_of/path_of must round-trip"
            );
        }
        CodecInput::KeyFormat(hash) => {
            let tk = tree_key(&hash);
            assert!(tk.starts_with(TREE_PREFIX));
            assert_eq!(tk.len(), TREE_PREFIX.len() + 64);
            assert!(tk[TREE_PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
            let ck = chunk_key(&hash);
            assert!(ck.starts_with("fschunk:"));
            assert_eq!(ck.len(), "fschunk:".len() + 64);
            // The two key spaces must never collide in the shared blob backend.
            assert_ne!(tk, ck);
        }
    }
});
