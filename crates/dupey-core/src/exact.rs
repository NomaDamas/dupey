use sha2::{Digest, Sha256};

/// SHA-256 of canonical text. Same content after extract => same digest.
pub fn exact_hash(text: &str) -> [u8; 32] {
    let digest = Sha256::digest(text.as_bytes());
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub fn exact_hash_hex(text: &str) -> String {
    hex::encode(exact_hash(text))
}

/// SHA-256 of original file bytes, used for exact matching when extraction
/// produced no comparable text.
pub fn byte_hash(bytes: &[u8]) -> [u8; 32] {
    let digest = Sha256::digest(bytes);
    let mut out = [0u8; 32];
    out.copy_from_slice(&digest);
    out
}

pub fn byte_hash_hex(bytes: &[u8]) -> String {
    hex::encode(byte_hash(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stable_and_sensitive() {
        assert_eq!(exact_hash("hello"), exact_hash("hello"));
        assert_ne!(exact_hash("hello"), exact_hash("hello "));
    }

    #[test]
    fn byte_hash_is_stable_and_sensitive() {
        assert_eq!(byte_hash_hex(b"hello"), byte_hash_hex(b"hello"));
        assert_ne!(byte_hash_hex(b"hello"), byte_hash_hex(b"hello "));
    }
}
