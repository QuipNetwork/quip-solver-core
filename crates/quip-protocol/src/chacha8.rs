//! `ChaCha8Rng` + `PoW` draw-order reference, ported from `shared/chacha8.py`.
//!
//! Produces byte-identical output to the Python reference for cross-language
//! deterministic Ising model generation. Not intended for cryptographic use.

const CONSTANTS: [u32; 4] = [0x6170_7865, 0x3320_646e, 0x7962_2d32, 0x6b20_6574]; // "expand 32-byte k"

/// Deterministic `ChaCha8` stream matching the Python golden reference.
///
/// Not a general-purpose CSPRNG — only the `PoW` Ising draw path.
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
/// # Panics
/// Panics if any of `a`, `b`, `c`, `d` is out of range for a 16-word state.
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
    ///
    /// # Panics
    /// Does not panic in practice: `chunks_exact(4)` always yields 4-byte
    /// chunks, and the `expect` only guards that invariant.
    #[must_use]
    pub fn from_seed(key_bytes: [u8; 32]) -> Self {
        let mut key = [0u32; 8];
        for (word, chunk) in key.iter_mut().zip(key_bytes.chunks_exact(4)) {
            *word = u32::from_le_bytes(
                #[expect(
                    clippy::expect_used,
                    reason = "chunks_exact(4) always yields 4-byte chunks"
                )]
                {
                    chunk
                        .try_into()
                        .expect("chunks_exact(4) always yields 4-byte chunks")
                },
            );
        }
        Self {
            key,
            counter: 0,
            block: [0; 16],
            idx: 16,
        }
    }

    fn regen(&mut self) {
        let mut s = [0u32; 16];
        s[0..4].copy_from_slice(&CONSTANTS);
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

/// Error drawing an Ising model from a nonce.
#[derive(Debug, PartialEq, Eq)]
pub enum DrawError {
    /// An allowed-value set was empty while values were required to be drawn
    /// from it (`n_nodes > 0` with empty `allowed_h`, or `n_edges > 0` with
    /// empty `allowed_j`). A degenerate or non-`Set` chain snapshot reaches
    /// here; drawing would be a modulo-by-zero, so it is rejected instead.
    EmptyAllowedValues,
}

impl std::fmt::Display for DrawError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::EmptyAllowedValues => {
                write!(f, "allowed-value set is empty; cannot draw an Ising model")
            }
        }
    }
}

impl std::error::Error for DrawError {}

/// One state word across `L` consecutive blocks.
type Lanes<const L: usize> = [u32; L];

/// The quarter-round of [`quarter_round`], applied lane by lane.
///
/// The lane loop has no cross-lane dependency, so LLVM lowers it to vector
/// adds, xors, and rotates when the target has a vector rotate or shift.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
)]
fn quarter_round_lanes<const L: usize>(
    a: &mut Lanes<L>,
    b: &mut Lanes<L>,
    c: &mut Lanes<L>,
    d: &mut Lanes<L>,
) {
    for (((a, b), c), d) in a
        .iter_mut()
        .zip(b.iter_mut())
        .zip(c.iter_mut())
        .zip(d.iter_mut())
    {
        *a = a.wrapping_add(*b);
        *d = (*d ^ *a).rotate_left(16);
        *c = c.wrapping_add(*d);
        *b = (*b ^ *c).rotate_left(12);
        *a = a.wrapping_add(*b);
        *d = (*d ^ *a).rotate_left(8);
        *c = c.wrapping_add(*d);
        *b = (*b ^ *c).rotate_left(7);
    }
}

/// Keystream blocks `counter .. counter + L`, block by block in stream order.
///
/// Byte-identical to `L` calls of [`ChaCha8Rng`]'s block function. The state
/// is held word-major, one [`Lanes`] array per state word, so every round step
/// is one operation across the `L` blocks. The 16 state words stay live
/// through every round, so `L` lanes of one word should fill one vector
/// register, or the rounds spill to the stack.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
)]
fn wide_blocks<const L: usize>(key: &[u32; 8], counter: u64, out: &mut [[u32; 16]; L]) {
    let mut state = [[0u32; L]; 16];
    for (word, constant) in state.iter_mut().zip(CONSTANTS) {
        *word = [constant; L];
    }
    for (word, key_word) in state.iter_mut().skip(4).zip(key) {
        *word = [*key_word; L];
    }
    let [.., c_lo, c_hi, _, _] = &mut state;
    for ((lo, hi), lane) in c_lo.iter_mut().zip(c_hi.iter_mut()).zip(0u64..) {
        let block = counter.wrapping_add(lane);
        // Low/high 32 bits of the 64-bit counter (mask/shift make truncation safe).
        *lo = (block & 0xFFFF_FFFF) as u32;
        *hi = (block >> 32) as u32;
    }
    let start = state;
    let [x0, x1, x2, x3, x4, x5, x6, x7, x8, x9, x10, x11, x12, x13, x14, x15] = &mut state;
    for _ in 0..4 {
        quarter_round_lanes(x0, x4, x8, x12);
        quarter_round_lanes(x1, x5, x9, x13);
        quarter_round_lanes(x2, x6, x10, x14);
        quarter_round_lanes(x3, x7, x11, x15);
        quarter_round_lanes(x0, x5, x10, x15);
        quarter_round_lanes(x1, x6, x11, x12);
        quarter_round_lanes(x2, x7, x8, x13);
        quarter_round_lanes(x3, x4, x9, x14);
    }
    for (word_index, (word, initial)) in state.iter().zip(&start).enumerate() {
        for (lane, (value, initial)) in word.iter().zip(initial).enumerate() {
            if let Some(slot) = out
                .get_mut(lane)
                .and_then(|block| block.get_mut(word_index))
            {
                *slot = value.wrapping_add(*initial);
            }
        }
    }
}

/// Exact `word % divisor` by one multiply-high (Lemire, Kaser, Kurz 2019).
#[derive(Clone, Copy)]
struct FastMod {
    /// `ceil(2^64 / divisor)`. Wraps to zero for a divisor of one, which
    /// yields the correct remainder of zero.
    magic: u64,
    /// The divisor.
    divisor: u64,
}

impl FastMod {
    const fn new(divisor: u32) -> Self {
        Self {
            magic: (u64::MAX / divisor as u64).wrapping_add(1),
            divisor: divisor as u64,
        }
    }

    #[inline(always)]
    #[expect(
        clippy::inline_always,
        reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
    )]
    #[expect(
        clippy::cast_possible_truncation,
        reason = "the high half of a 64x64 product by a u32 divisor is below the divisor"
    )]
    const fn rem(self, word: u32) -> usize {
        let low = self.magic.wrapping_mul(word as u64);
        ((low as u128 * self.divisor as u128) >> 64) as usize
    }
}

/// `out[i] = allowed[words[i] % D]`. A constant divisor lets LLVM lower the
/// remainder to a vectorizable multiply instead of a division, and a
/// fixed-size table lets it drop the bounds check on every lookup.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
)]
fn select_const<T: Copy, const D: usize>(words: &[u32], allowed: &[T], out: &mut [T]) {
    let Ok(table) = <&[T; D]>::try_from(allowed) else {
        return;
    };
    // `select` passes D of at most 8, so the conversion never saturates.
    let divisor = u32::try_from(D).unwrap_or(u32::MAX);
    for (slot, &word) in out.iter_mut().zip(words) {
        if let Some(value) = table.get((word % divisor) as usize) {
            *slot = *value;
        }
    }
}

/// `out[i] = allowed[words[i] % allowed.len()]` for any set size.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
)]
fn select<T: Copy>(words: &[u32], allowed: &[T], out: &mut [T]) {
    match allowed.len() {
        1 => select_const::<T, 1>(words, allowed, out),
        2 => select_const::<T, 2>(words, allowed, out),
        3 => select_const::<T, 3>(words, allowed, out),
        4 => select_const::<T, 4>(words, allowed, out),
        5 => select_const::<T, 5>(words, allowed, out),
        6 => select_const::<T, 6>(words, allowed, out),
        7 => select_const::<T, 7>(words, allowed, out),
        8 => select_const::<T, 8>(words, allowed, out),
        len => {
            // A set larger than any u32 word makes `word % len` the word itself.
            let modulus = u32::try_from(len).ok().map(FastMod::new);
            for (slot, &word) in out.iter_mut().zip(words) {
                let index = modulus.map_or(word as usize, |m| m.rem(word));
                if let Some(value) = allowed.get(index) {
                    *slot = *value;
                }
            }
        }
    }
}

/// [`draw_into`] for a non-empty `allowed`, `L` blocks per keystream pass.
#[inline(always)]
#[expect(
    clippy::inline_always,
    reason = "must inline into draw_avx2 to be compiled with AVX2 enabled"
)]
fn draw_lanes<T: Copy, const L: usize>(
    key: &[u32; 8],
    first_word: u64,
    allowed: &[T],
    out: &mut [T],
) {
    let mut keystream = [[0u32; 16]; L];
    let mut counter = first_word / 16;
    // `first_word % 16` is below 16, so the cast is lossless.
    let mut skip = (first_word % 16) as usize;
    let mut rest = out;
    while !rest.is_empty() {
        wide_blocks(key, counter, &mut keystream);
        counter = counter.wrapping_add(L as u64);
        let words = keystream.as_flattened().get(skip..).unwrap_or_default();
        skip = 0;
        let (chunk, tail) = rest.split_at_mut(rest.len().min(words.len()));
        select(words, allowed, chunk);
        rest = tail;
    }
}

/// [`draw_lanes`] compiled for AVX2: eight lanes fill one 256-bit register.
#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2")]
fn draw_avx2<T: Copy>(key: &[u32; 8], first_word: u64, allowed: &[T], out: &mut [T]) {
    draw_lanes::<T, 8>(key, first_word, allowed, out);
}

/// Pick the widest keystream generator this CPU runs.
///
/// A build that already targets AVX2 uses it directly. A baseline `x86_64`
/// build detects AVX2 at run time. Other targets use four lanes, the width of
/// a NEON or `simd128` register.
fn draw_dispatch<T: Copy>(key: &[u32; 8], first_word: u64, allowed: &[T], out: &mut [T]) {
    #[cfg(target_arch = "x86_64")]
    if cfg!(target_feature = "avx2") || std::arch::is_x86_feature_detected!("avx2") {
        #[expect(
            unsafe_code,
            reason = "calling a target_feature function; AVX2 support is checked on the line above"
        )]
        // SAFETY: `draw_avx2` needs only AVX2, which the condition above
        // confirmed at compile time or on this CPU.
        unsafe {
            draw_avx2(key, first_word, allowed, out);
        }
        return;
    }
    draw_lanes::<T, 4>(key, first_word, allowed, out);
}

/// Fill `out` from the draw stream of `nonce`, starting at keystream word
/// `first_word`, selecting each value from `allowed`.
///
/// `out[i]` is `allowed[w % allowed.len()]` for keystream word
/// `w = first_word + i`. This is random access into the same stream
/// [`draw_ising_milli`] reads in order: fields take words `0..n_nodes` and
/// couplings take words `n_nodes..n_nodes + n_edges`. Disjoint ranges can be
/// drawn on separate threads and concatenated, with output identical to a
/// single sequential draw.
///
/// `allowed` may hold any `Copy` value, such as milli integers or the
/// solver's own coefficient type converted once from the milli set.
///
/// # Errors
/// Returns [`DrawError::EmptyAllowedValues`] if `out` is not empty and
/// `allowed` is.
pub fn draw_into<T: Copy>(
    nonce: [u8; 32],
    first_word: u64,
    allowed: &[T],
    out: &mut [T],
) -> Result<(), DrawError> {
    if out.is_empty() {
        return Ok(());
    }
    if allowed.is_empty() {
        return Err(DrawError::EmptyAllowedValues);
    }
    draw_dispatch(&ChaCha8Rng::from_seed(nonce).key, first_word, allowed, out);
    Ok(())
}

/// Draw an Ising model deterministically from a nonce, selecting each field
/// and coupling from `allowed_h` and `allowed_j` via the golden `ChaCha8`
/// draw order.
///
/// [`draw_ising_milli`] is this function over the milli sets. Passing tables
/// of another `Copy` type, entry for entry parallel to the milli sets, draws
/// straight into that type with no per-value conversion.
///
/// # Errors
/// Returns [`DrawError::EmptyAllowedValues`] if a required allowed-value set is
/// empty, rather than panicking on the modulo-by-zero.
pub fn draw_ising<T: Copy>(
    nonce: [u8; 32],
    n_nodes: usize,
    n_edges: usize,
    allowed_h: &[T],
    allowed_j: &[T],
) -> Result<(Vec<T>, Vec<T>), DrawError> {
    if (n_nodes > 0 && allowed_h.is_empty()) || (n_edges > 0 && allowed_j.is_empty()) {
        return Err(DrawError::EmptyAllowedValues);
    }
    // The fills below overwrite every slot, so the seed value never survives.
    let mut h = allowed_h
        .first()
        .map_or_else(Vec::new, |&v| vec![v; n_nodes]);
    let mut j = allowed_j
        .first()
        .map_or_else(Vec::new, |&v| vec![v; n_edges]);
    draw_into(nonce, 0, allowed_h, &mut h)?;
    draw_into(nonce, n_nodes as u64, allowed_j, &mut j)?;
    Ok((h, j))
}

/// Draw an Ising model (field/coupling milli-values) deterministically from a
/// nonce, selecting each value from the allowed sets via the golden `ChaCha8`
/// draw order.
///
/// # Errors
/// Returns [`DrawError::EmptyAllowedValues`] if a required allowed-value set is
/// empty, rather than panicking on the modulo-by-zero.
pub fn draw_ising_milli(
    nonce: [u8; 32],
    n_nodes: usize,
    n_edges: usize,
    allowed_h: &[i32],
    allowed_j: &[i32],
) -> Result<(Vec<i32>, Vec<i32>), DrawError> {
    draw_ising(nonce, n_nodes, n_edges, allowed_h, allowed_j)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_rejects_empty_allowed_h_when_nodes_present() {
        let err = draw_ising_milli([0u8; 32], 2, 0, &[], &[]);
        assert_eq!(err, Err(DrawError::EmptyAllowedValues));
    }

    #[test]
    fn draw_rejects_empty_allowed_j_when_edges_present() {
        let err = draw_ising_milli([0u8; 32], 1, 1, &[-1000, 1000], &[]);
        assert_eq!(err, Err(DrawError::EmptyAllowedValues));
    }

    /// Words `first..first + len` of the scalar reference stream.
    fn reference_words(seed: [u8; 32], first: usize, len: usize) -> Vec<u32> {
        let mut rng = ChaCha8Rng::from_seed(seed);
        for _ in 0..first {
            let _ = rng.next_u32();
        }
        (0..len).map(|_| rng.next_u32()).collect()
    }

    #[test]
    fn draw_into_matches_the_scalar_stream_at_every_alignment() {
        let seed = [0x5A; 32];
        // Identity table up to 2^16 so the drawn value exposes `word % len`.
        let table: Vec<u32> = (0..1u32 << 16).collect();
        for first in [0, 1, 15, 16, 17, 127, 128, 129, 300] {
            for len in [1, 15, 16, 17, 128, 129, 1000] {
                let expected: Vec<u32> = reference_words(seed, first, len)
                    .into_iter()
                    .map(|w| w % (1 << 16))
                    .collect();
                let mut out = vec![0u32; len];
                draw_into(seed, first as u64, &table, &mut out).unwrap();
                assert_eq!(out, expected, "dispatched, first {first}, len {len}");
                // The portable path, which an AVX2 machine never dispatches to.
                let key = ChaCha8Rng::from_seed(seed).key;
                let mut out = vec![0u32; len];
                draw_lanes::<u32, 4>(&key, first as u64, &table, &mut out);
                assert_eq!(out, expected, "four lanes, first {first}, len {len}");
            }
        }
    }

    #[test]
    fn fast_modulo_is_exact_for_every_set_size() {
        let seed = [0xA5; 32];
        let words = reference_words(seed, 0, 512);
        for divisor in [1u32, 2, 3, 5, 7, 255, 1000, 65_537, u32::MAX] {
            let m = FastMod::new(divisor);
            for &w in words.iter().chain(&[0, 1, u32::MAX, u32::MAX - 1]) {
                assert_eq!(m.rem(w), (w % divisor) as usize, "{w} % {divisor}");
            }
        }
    }

    #[test]
    fn split_draws_concatenate_to_the_sequential_draw() {
        let nonce = [0x3C; 32];
        let allowed = [-1000, 0, 1000];
        let mut whole = vec![0; 1000];
        draw_into(nonce, 0, &allowed, &mut whole).unwrap();
        let mut parts = vec![0; 1000];
        let (a, rest) = parts.split_at_mut(333);
        let (b, c) = rest.split_at_mut(400);
        draw_into(nonce, 0, &allowed, a).unwrap();
        draw_into(nonce, 333, &allowed, b).unwrap();
        draw_into(nonce, 733, &allowed, c).unwrap();
        assert_eq!(parts, whole);
    }

    #[test]
    fn typed_tables_draw_the_same_selection_as_milli() {
        let nonce = [0x77; 32];
        let (h, j) = draw_ising_milli(nonce, 37, 101, &[-1000, 0, 1000], &[-1000, 1000]).unwrap();
        let (th, tj) = draw_ising(nonce, 37, 101, &[-1i8, 0, 1], &[-1i8, 1]).unwrap();
        assert_eq!(
            th,
            h.iter()
                .map(|&m| i8::try_from(m / 1000).unwrap())
                .collect::<Vec<_>>()
        );
        assert_eq!(
            tj,
            j.iter()
                .map(|&m| i8::try_from(m / 1000).unwrap())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn draw_allows_empty_sets_when_no_draws_needed() {
        // Zero nodes and zero edges: nothing to draw, empty sets are fine.
        let (h, j) = draw_ising_milli([0u8; 32], 0, 0, &[], &[]).unwrap();
        assert!(h.is_empty());
        assert!(j.is_empty());
    }
}
