//! A nonce of one repeated byte, for tests that declare it with
//! `#[path = "common/nonce.rs"] mod nonce;`.

/// A nonce whose 32 bytes all equal `i`.
pub(crate) fn nonce_of(i: u8) -> [u8; 32] {
    [i; 32]
}
