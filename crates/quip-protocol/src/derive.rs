//! `PoW` nonce derivation: BLAKE3 over (`last_proof`, miner, salt).
//!
//! Golden-pinned against `conformance/golden_vectors.json` `derive_nonce`.

/// Derive the canonical 32-byte `PoW` nonce.
///
/// Input order is load-bearing: `last_proof` then `miner` then `salt`.
/// Mirrors `shared.quantum_proof_of_work.derive_nonce` and
/// `quantum_validation::derive_nonce`.
#[must_use]
pub fn derive_nonce(last_proof: [u8; 32], miner: [u8; 32], salt: [u8; 32]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    let _ = h.update(&last_proof);
    let _ = h.update(&miner);
    let _ = h.update(&salt);
    *h.finalize().as_bytes()
}

/// BLAKE3 initial chaining value, the SHA-256 IV.
const IV: [u32; 8] = [
    0x6A09_E667,
    0xBB67_AE85,
    0x3C6E_F372,
    0xA54F_F53A,
    0x510E_527F,
    0x9B05_688C,
    0x1F83_D9AB,
    0x5BE0_CD19,
];
/// BLAKE3 domain flag on the first block of a chunk.
const CHUNK_START: u32 = 1;
/// BLAKE3 domain flag on the last block of a chunk.
const CHUNK_END: u32 = 2;
/// BLAKE3 domain flag on the block that yields the root hash.
const ROOT: u32 = 8;

/// The BLAKE3 mixing function `G`.
fn g(a: &mut u32, b: &mut u32, c: &mut u32, d: &mut u32, mx: u32, my: u32) {
    *a = a.wrapping_add(*b).wrapping_add(mx);
    *d = (*d ^ *a).rotate_right(16);
    *c = c.wrapping_add(*d);
    *b = (*b ^ *c).rotate_right(12);
    *a = a.wrapping_add(*b).wrapping_add(my);
    *d = (*d ^ *a).rotate_right(8);
    *c = c.wrapping_add(*d);
    *b = (*b ^ *c).rotate_right(7);
}

/// BLAKE3 compression of one block with chunk counter zero. Returns the first
/// eight output words: the next chaining value, or under [`ROOT`] the hash.
fn compress(cv: &[u32; 8], m: &[u32; 16], block_len: u32, flags: u32) -> [u32; 8] {
    let [c0, c1, c2, c3, c4, c5, c6, c7] = *cv;
    let [i0, i1, i2, i3, ..] = IV;
    // Words 12 and 13 hold the chunk counter, which stays zero for a
    // one-chunk input.
    let mut state = [
        c0, c1, c2, c3, c4, c5, c6, c7, i0, i1, i2, i3, 0, 0, block_len, flags,
    ];
    let [v0, v1, v2, v3, v4, v5, v6, v7, v8, v9, v10, v11, v12, v13, v14, v15] = &mut state;
    let mut m = *m;
    for _ in 0..7 {
        let [m0, m1, m2, m3, m4, m5, m6, m7, m8, m9, m10, m11, m12, m13, m14, m15] = m;
        g(v0, v4, v8, v12, m0, m1);
        g(v1, v5, v9, v13, m2, m3);
        g(v2, v6, v10, v14, m4, m5);
        g(v3, v7, v11, v15, m6, m7);
        g(v0, v5, v10, v15, m8, m9);
        g(v1, v6, v11, v12, m10, m11);
        g(v2, v7, v8, v13, m12, m13);
        g(v3, v4, v9, v14, m14, m15);
        // The BLAKE3 message permutation.
        m = [
            m2, m6, m3, m10, m7, m0, m4, m13, m1, m11, m12, m5, m9, m14, m15, m8,
        ];
    }
    let mut out = [0u32; 8];
    for ((word, low), high) in out.iter_mut().zip(&state).zip(state.iter().skip(8)) {
        *word = low ^ high;
    }
    out
}

/// Fill `out` with the little-endian words of `bytes`.
fn read_words(out: &mut [u32], bytes: &[u8; 32]) {
    for (word, chunk) in out.iter_mut().zip(bytes.chunks_exact(4)) {
        let mut le = [0u8; 4];
        le.copy_from_slice(chunk);
        *word = u32::from_le_bytes(le);
    }
}

/// [`derive_nonce`] for many salts that share `last_proof` and `miner`.
///
/// The input is 96 bytes, so BLAKE3 compresses two 64-byte blocks. The first,
/// `last_proof || miner`, is the same for every salt, and [`Self::new`]
/// compresses it once. Each [`Self::derive`] then compresses only `salt` and
/// 32 zero bytes, about half the work of [`derive_nonce`]. Output is
/// byte-identical to [`derive_nonce`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NonceDeriver {
    /// Chaining value after the first block.
    cv: [u32; 8],
}

impl NonceDeriver {
    /// Compress the shared first block.
    #[must_use]
    pub fn new(last_proof: [u8; 32], miner: [u8; 32]) -> Self {
        let mut m = [0u32; 16];
        let (head, tail) = m.split_at_mut(8);
        read_words(head, &last_proof);
        read_words(tail, &miner);
        Self {
            cv: compress(&IV, &m, 64, CHUNK_START),
        }
    }

    /// Nonce for one salt.
    #[must_use]
    pub fn derive(&self, salt: [u8; 32]) -> [u8; 32] {
        // Words 8 to 15 are the zero padding after the 32-byte salt.
        let mut m = [0u32; 16];
        read_words(&mut m, &salt);
        let hash = compress(&self.cv, &m, 32, CHUNK_END | ROOT);
        let mut nonce = [0u8; 32];
        for (bytes, word) in nonce.chunks_exact_mut(4).zip(hash) {
            bytes.copy_from_slice(&word.to_le_bytes());
        }
        nonce
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_golden_vector() {
        // conformance/golden_vectors.json derive_nonce[0]
        let last = [0u8; 32];
        let miner = [1u8; 32];
        let salt = [2u8; 32];
        let nonce = derive_nonce(last, miner, salt);
        // b4179357b751254ed0e68b5e969dcb50e73fd8c56be192b79d286ff2722d6a72
        let expected: [u8; 32] = [
            0xb4, 0x17, 0x93, 0x57, 0xb7, 0x51, 0x25, 0x4e, 0xd0, 0xe6, 0x8b, 0x5e, 0x96, 0x9d,
            0xcb, 0x50, 0xe7, 0x3f, 0xd8, 0xc5, 0x6b, 0xe1, 0x92, 0xb7, 0x9d, 0x28, 0x6f, 0xf2,
            0x72, 0x2d, 0x6a, 0x72,
        ];
        assert_eq!(nonce, expected);
    }

    #[test]
    fn input_order_is_load_bearing() {
        let a = derive_nonce([0u8; 32], [1u8; 32], [2u8; 32]);
        let b = derive_nonce([1u8; 32], [0u8; 32], [2u8; 32]);
        assert_ne!(a, b);
    }

    /// Deterministic, varied 32-byte values for cross-checks.
    fn sample(seed: u8) -> [u8; 32] {
        *blake3::hash(&[seed]).as_bytes()
    }

    #[test]
    fn deriver_matches_derive_nonce() {
        for k in 0..64u8 {
            let (last, miner, salt) = (sample(k), sample(k ^ 0x40), sample(k ^ 0x80));
            let deriver = NonceDeriver::new(last, miner);
            assert_eq!(
                deriver.derive(salt),
                derive_nonce(last, miner, salt),
                "case {k}"
            );
        }
    }

    #[test]
    fn salt_is_load_bearing() {
        // A bug that dropped `salt` from the hash would pass `input_order_is_
        // load_bearing` (which only permutes last_proof/miner); pin salt too.
        let a = derive_nonce([0u8; 32], [1u8; 32], [2u8; 32]);
        let b = derive_nonce([0u8; 32], [1u8; 32], [3u8; 32]);
        assert_ne!(a, b);
    }
}
