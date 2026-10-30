//! `ChaCha8` keystream and substream seeding.

use core::fmt;
use core::num::NonZeroU32;

use crate::digest::BufferedHasher;

/// The `ChaCha` constants, the ASCII bytes of `expand 32-byte k` as LE words.
pub const SIGMA: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574];

/// Read a 32-byte seed as eight little-endian key words.
#[must_use]
pub fn key_words(seed: &[u8; 32]) -> [u32; 8] {
    let mut key = [0u32; 8];
    for (word, chunk) in key.iter_mut().zip(seed.chunks_exact(4)) {
        let mut bytes = [0u8; 4];
        bytes.copy_from_slice(chunk);
        *word = u32::from_le_bytes(bytes);
    }
    key
}

/// Deterministic `ChaCha8` stream (8 rounds, 64-bit block counter from 0).
///
/// Not a general-purpose CSPRNG.
#[derive(Debug)]
pub struct ChaCha8Rng {
    /// Expanded 256-bit key as eight little-endian words.
    key: [u32; 8],
    /// Block counter (advances after each 16-word block).
    counter: u64,
    /// Current keystream block (16 words).
    block: [u32; 16],
    /// Next unread word index; `16` means the block is exhausted.
    idx: usize,
}

/// `ChaCha` quarter-round on four state indices (standard crypto naming).
///
/// Call sites pass only compile-time constants in `0..16`.
#[expect(
    clippy::many_single_char_names,
    reason = "standard ChaCha quarter-round parameter names a/b/c/d"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "indices are fixed constants 0..16 from regen callers"
)]
fn quarter_round(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] ^= s[a];
    s[d] = s[d].rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] ^= s[c];
    s[b] = s[b].rotate_left(7);
}

impl ChaCha8Rng {
    /// Build an RNG from a 32-byte seed (little-endian key words).
    #[must_use]
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self {
            key: key_words(&seed),
            counter: 0,
            block: [0; 16],
            idx: 16,
        }
    }

    fn regen(&mut self) {
        let mut s = [0u32; 16];
        s[0..4].copy_from_slice(&SIGMA);
        s[4..12].copy_from_slice(&self.key);
        // Low/high 32 bits of the 64-bit counter (mask/shift make truncation safe).
        s[12] = (self.counter & 0xFFFF_FFFF) as u32;
        s[13] = (self.counter >> 32) as u32;
        s[14] = 0; // stream lo
        s[15] = 0; // stream hi
        let start = s;
        for _ in 0..4 {
            quarter_round(&mut s, 0, 4, 8, 12);
            quarter_round(&mut s, 1, 5, 9, 13);
            quarter_round(&mut s, 2, 6, 10, 14);
            quarter_round(&mut s, 3, 7, 11, 15);
            quarter_round(&mut s, 0, 5, 10, 15);
            quarter_round(&mut s, 1, 6, 11, 12);
            quarter_round(&mut s, 2, 7, 8, 13);
            quarter_round(&mut s, 3, 4, 9, 14);
        }
        for i in 0..16 {
            #[expect(
                clippy::indexing_slicing,
                reason = "i iterates 0..16 over fixed-size [u32; 16] arrays"
            )]
            {
                self.block[i] = s[i].wrapping_add(start[i]);
            }
        }
        self.counter += 1;
        self.idx = 0;
    }

    /// Next little-endian keystream word.
    #[must_use]
    pub fn next_u32(&mut self) -> u32 {
        if self.idx >= 16 {
            self.regen();
        }
        #[expect(
            clippy::indexing_slicing,
            reason = "idx is reset to 0 by regen when >= 16, so always in 0..16"
        )]
        let w = self.block[self.idx];
        self.idx += 1;
        w
    }
}

/// Unbiased draw in `0..n` by rejection. Reads at least one word, also for `n = 1`.
#[must_use]
pub fn uniform(rng: &mut ChaCha8Rng, n: NonZeroU32) -> u32 {
    let n = u64::from(n.get());
    let limit = (1u64 << 32) - ((1u64 << 32) % n);
    loop {
        let x = u64::from(rng.next_u32());
        if x < limit {
            #[expect(clippy::cast_possible_truncation, reason = "x % n < n <= u32::MAX")]
            return (x % n) as u32;
        }
    }
}

/// The longest substream label in bytes.
pub const MAX_LABEL_LEN: usize = 32;

/// A substream label outside 1 to [`MAX_LABEL_LEN`] bytes, or with a byte
/// above `0x7F`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidLabel;

impl fmt::Display for InvalidLabel {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("substream label must be 1 to 32 ASCII bytes")
    }
}

impl core::error::Error for InvalidLabel {}

/// The seed of one substream (FAM-22): BLAKE3 in key-derivation mode with the
/// context `quip family stream v1` over the nonce, the descriptor digest, the
/// topology order digest, `u8 len(label)`, and the label.
///
/// # Errors
///
/// Returns [`InvalidLabel`] when the label is shorter than 1 byte, longer than
/// [`MAX_LABEL_LEN`] bytes, or has a byte above `0x7F`.
pub fn stream_seed(
    nonce: &[u8; 32],
    descriptor_digest: &[u8; 32],
    topology_order_digest: &[u8; 32],
    label: &[u8],
) -> Result<[u8; 32], InvalidLabel> {
    if label.is_empty() || label.len() > MAX_LABEL_LEN || !label.is_ascii() {
        return Err(InvalidLabel);
    }
    let label_len = u8::try_from(label.len()).map_err(|_| InvalidLabel)?;
    let mut hasher = BufferedHasher::new("quip family stream v1");
    hasher.put(nonce);
    hasher.put(descriptor_digest);
    hasher.put(topology_order_digest);
    hasher.put(&[label_len]);
    hasher.put(label);
    Ok(hasher.finish())
}
