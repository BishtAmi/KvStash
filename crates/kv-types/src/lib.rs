use std::fmt;
use std::num::NonZeroUsize;

/// A truncated BLAKE3 digest identifying a prefix of model tokens.
#[derive(Clone, Copy, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ChunkKey([u8; 16]);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidChunkSize;

impl fmt::Display for InvalidChunkSize {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("chunk size must be greater than zero")
    }
}

impl std::error::Error for InvalidChunkSize {}

impl ChunkKey {
    pub const LENGTH: usize = 16;

    pub const fn from_bytes(bytes: [u8; Self::LENGTH]) -> Self {
        Self(bytes)
    }

    pub const fn as_bytes(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl fmt::Display for ChunkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

impl fmt::Debug for ChunkKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ChunkKey({self})")
    }
}

/// Hashes one token chunk together with its model identity and preceding prefix.
///
/// Lengths and a presence marker are included to keep the encoded inputs
/// unambiguous. Token values use little-endian encoding for cross-platform
/// consistency.
pub fn chain_hash(model_fp: &[u8], prev: Option<&ChunkKey>, tokens: &[u32]) -> ChunkKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"kvstash/chunk-key/v1");
    hasher.update(&(model_fp.len() as u64).to_le_bytes());
    hasher.update(model_fp);

    match prev {
        Some(key) => {
            hasher.update(&[1]);
            hasher.update(key.as_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }

    hasher.update(&(tokens.len() as u64).to_le_bytes());
    for token in tokens {
        hasher.update(&token.to_le_bytes());
    }

    let digest = hasher.finalize();
    let mut key = [0; ChunkKey::LENGTH];
    key.copy_from_slice(&digest.as_bytes()[..ChunkKey::LENGTH]);
    ChunkKey::from_bytes(key)
}

/// Returns a key for each complete token chunk.
///
/// A trailing partial chunk is omitted because its KV data may not have the
/// same shape as a complete chunk. An empty token slice returns no keys.
pub fn chunk_keys(
    model_fp: &[u8],
    tokens: &[u32],
    chunk_size: usize,
) -> Result<Vec<ChunkKey>, InvalidChunkSize> {
    let chunk_size = NonZeroUsize::new(chunk_size).ok_or(InvalidChunkSize)?;
    let mut keys = Vec::with_capacity(tokens.len() / chunk_size.get());
    let mut prev = None;

    for chunk in tokens.chunks_exact(chunk_size.get()) {
        let key = chain_hash(model_fp, prev.as_ref(), chunk);
        keys.push(key);
        prev = Some(key);
    }

    Ok(keys)
}

#[cfg(test)]
mod tests {
    use super::{chain_hash, chunk_keys, ChunkKey};
    use proptest::prelude::*;

    proptest! {
        #[test]
        fn equal_prefixes_produce_equal_keys(
            model_fp in prop::collection::vec(any::<u8>(), 0..128),
            prefix in prop::collection::vec(any::<u32>(), 0..128),
            chunk in prop::collection::vec(any::<u32>(), 0..128),
        ) {
            let parent_a = chain_hash(&model_fp, None, &prefix);
            let parent_b = chain_hash(&model_fp.clone(), None, &prefix.clone());
            prop_assert_eq!(
                chain_hash(&model_fp, Some(&parent_a), &chunk),
                chain_hash(&model_fp, Some(&parent_b), &chunk),
            );
        }
    }

    #[test]
    fn model_and_parent_are_part_of_the_key() {
        let parent = chain_hash(b"model-a", None, &[1, 2]);
        let other_parent = chain_hash(b"model-a", None, &[1, 3]);

        let key = chain_hash(b"model-a", Some(&parent), &[4, 5]);
        assert_ne!(key, chain_hash(b"model-b", Some(&parent), &[4, 5]));
        assert_ne!(key, chain_hash(b"model-a", Some(&other_parent), &[4, 5]));
        assert_ne!(key, chain_hash(b"model-a", Some(&parent), &[4, 6]));
    }

    #[test]
    fn shared_prefix_shares_keys_then_diverges() {
        let tokens: Vec<u32> = (0..64).collect();
        let mut diverging_tokens = tokens.clone();
        diverging_tokens[40] = 9999;

        let original = chunk_keys(b"model", &tokens, 16).unwrap();
        let diverging = chunk_keys(b"model", &diverging_tokens, 16).unwrap();

        assert_eq!(&original[..2], &diverging[..2]);
        assert_ne!(original[2], diverging[2]);
        assert_ne!(original[3], diverging[3]);
    }

    #[test]
    fn chunk_keys_omit_trailing_partial_chunk() {
        let keys = chunk_keys(b"model", &[1, 2, 3, 4, 5], 2).unwrap();
        assert_eq!(keys.len(), 2);
    }

    #[test]
    fn chunk_keys_reject_zero_chunk_size() {
        assert!(chunk_keys(b"model", &[1, 2, 3], 0).is_err());
    }

    #[test]
    fn key_formats_as_lowercase_hex() {
        let key = ChunkKey::from_bytes([0xab; ChunkKey::LENGTH]);
        assert_eq!(key.to_string(), "abababababababababababababababab");
    }
}
