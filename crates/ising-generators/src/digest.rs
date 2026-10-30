//! A buffered BLAKE3 key-derivation hasher shared by the digest functions.

/// Bytes held before a flush to the hasher.
const BUF_LEN: usize = 1024;

/// A destination for the canonical encoder: a byte vector or a hasher.
pub(crate) trait ByteSink {
    /// Append bytes.
    fn put(&mut self, bytes: &[u8]);
}

impl ByteSink for alloc::vec::Vec<u8> {
    fn put(&mut self, bytes: &[u8]) {
        self.extend_from_slice(bytes);
    }
}

impl ByteSink for BufferedHasher {
    fn put(&mut self, bytes: &[u8]) {
        Self::put(self, bytes);
    }
}

/// BLAKE3 in key-derivation mode behind a stack buffer, so many small writes
/// reach the hasher as few large updates. It never allocates.
pub(crate) struct BufferedHasher {
    hasher: blake3::Hasher,
    buf: [u8; BUF_LEN],
    len: usize,
}

impl BufferedHasher {
    /// Start a hasher with the given derive-key context.
    pub(crate) fn new(context: &str) -> Self {
        Self {
            hasher: blake3::Hasher::new_derive_key(context),
            buf: [0; BUF_LEN],
            len: 0,
        }
    }

    /// Append bytes to the material.
    pub(crate) fn put(&mut self, mut bytes: &[u8]) {
        while !bytes.is_empty() {
            if self.len == BUF_LEN {
                self.flush();
            }
            let room = BUF_LEN.saturating_sub(self.len);
            let take = room.min(bytes.len());
            let (head, rest) = bytes.split_at(take);
            if let Some(dst) = self.buf.get_mut(self.len..self.len.saturating_add(take)) {
                dst.copy_from_slice(head);
            }
            self.len = self.len.saturating_add(take);
            bytes = rest;
        }
    }

    fn flush(&mut self) {
        if let Some(pending) = self.buf.get(..self.len) {
            let _updated = self.hasher.update(pending);
        }
        self.len = 0;
    }

    /// Flush and return the 32-byte digest.
    pub(crate) fn finish(mut self) -> [u8; 32] {
        self.flush();
        *self.hasher.finalize().as_bytes()
    }
}

#[cfg(test)]
mod tests {
    use super::{BufferedHasher, BUF_LEN};

    const CONTEXT: &str = "quip family test v1";

    fn pattern(len: usize) -> alloc::vec::Vec<u8> {
        (0..len)
            .map(|i| u8::try_from(i % 251).unwrap_or_default())
            .collect()
    }

    fn reference(all: &[u8]) -> [u8; 32] {
        let mut h = blake3::Hasher::new_derive_key(CONTEXT);
        let _updated = h.update(all);
        *h.finalize().as_bytes()
    }

    fn buffered(all: &[u8], step: usize) -> [u8; 32] {
        let mut h = BufferedHasher::new(CONTEXT);
        if step == 0 {
            h.put(all);
        } else {
            for chunk in all.chunks(step) {
                h.put(chunk);
            }
        }
        h.finish()
    }

    #[test]
    fn matches_blake3_for_every_split() {
        let lengths = [
            0,
            1,
            4,
            63,
            64,
            65,
            BUF_LEN - 1,
            BUF_LEN,
            BUF_LEN + 1,
            2 * BUF_LEN - 1,
            2 * BUF_LEN,
            2 * BUF_LEN + 1,
            3 * BUF_LEN + 17,
            10_000,
        ];
        for len in lengths {
            let all = pattern(len);
            let want = reference(&all);
            for step in [
                0,
                1,
                3,
                4,
                7,
                8,
                64,
                1000,
                BUF_LEN - 1,
                BUF_LEN,
                BUF_LEN + 1,
                5000,
            ] {
                assert_eq!(buffered(&all, step), want, "len {len} step {step}");
            }
        }
    }

    #[test]
    fn uneven_puts_cross_the_boundary() {
        let all = pattern(5 * BUF_LEN + 3);
        let mut h = BufferedHasher::new(CONTEXT);
        let mut rest = all.as_slice();
        let mut size = 1;
        while !rest.is_empty() {
            let take = size.min(rest.len());
            h.put(rest.get(..take).unwrap_or_default());
            rest = rest.get(take..).unwrap_or_default();
            size = size * 3 % 1500 + 1;
        }
        assert_eq!(h.finish(), reference(&all));
    }
}
