use sha2::{Digest, Sha256};
pub fn stable_shard(path: &str, total: usize) -> usize {
    let digest = Sha256::digest(path.as_bytes());
    (u64::from_be_bytes(digest[..8].try_into().unwrap()) % total as u64) as usize
}
