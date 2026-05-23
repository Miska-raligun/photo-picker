use super::{FeatureExtractor, PhotoFeatures};
use crate::error::Result;
use crate::ingest::PhotoRef;
use image::DynamicImage;
use image_hasher::{HashAlg, Hasher, HasherConfig};

pub struct HashOnlyExtractor {
    phash_hasher: Hasher,
    dhash_hasher: Hasher,
}

impl Default for HashOnlyExtractor {
    fn default() -> Self {
        Self::new()
    }
}

impl HashOnlyExtractor {
    pub fn new() -> Self {
        // 8×8 = 64-bit hashes for both algorithms.
        let phash_hasher = HasherConfig::new()
            .hash_alg(HashAlg::Mean)
            .hash_size(8, 8)
            .to_hasher();
        let dhash_hasher = HasherConfig::new()
            .hash_alg(HashAlg::Gradient)
            .hash_size(8, 8)
            .to_hasher();
        Self { phash_hasher, dhash_hasher }
    }
}

impl FeatureExtractor for HashOnlyExtractor {
    fn extract(&self, photo: &PhotoRef, thumb: &DynamicImage) -> Result<PhotoFeatures> {
        let phash = bytes_to_u64(self.phash_hasher.hash_image(thumb).as_bytes());
        let dhash = bytes_to_u64(self.dhash_hasher.hash_image(thumb).as_bytes());
        Ok(PhotoFeatures::hashes_only(photo.id, phash, dhash))
    }
}

fn bytes_to_u64(b: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let len = b.len().min(8);
    buf[..len].copy_from_slice(&b[..len]);
    u64::from_be_bytes(buf)
}

pub fn hamming(a: u64, b: u64) -> u32 {
    (a ^ b).count_ones()
}
