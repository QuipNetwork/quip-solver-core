//! A buffered BLAKE3 key-derivation hasher shared by the digest functions.

/// Bytes held before a flush to the hasher.
const BUF_LEN: usize = 1024;

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
