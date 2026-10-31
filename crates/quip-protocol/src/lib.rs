//! `QuIP` consensus SDK: wire codec, `ChaCha8` draw, energy scoring, diversity,
//! and proof target checks.
//!
//! These primitives are golden-pinned for cross-language consensus. The Rust
//! implementations in this crate are the source of truth mirrored by the `PyO3`
//! bindings. The scalar `ChaCha8` stream and the nonce derivation live in the
//! `ising-generators` crate.

pub mod chacha8;
pub mod diversity;
pub mod lease;
pub mod scoring;
pub mod target;
pub mod wire;

/// Handshake negotiation over the generated protobuf types.
///
/// Behind the default `session` feature because it is the only module that
/// reaches for `quip-proto`, and so the only one that cannot build for
/// `wasm32-unknown-unknown`. The consensus primitives above need nothing but
/// `ising-generators`, so `--no-default-features` yields a WASM-ready crate with the
/// golden-pinned math intact.
#[cfg(feature = "session")]
pub mod session;

/// FAM-42 mapping from family codes to wire reject reasons.
///
/// Behind `session` for the same reason as [`session`]: it names the generated
/// `RejectReason`.
#[cfg(feature = "session")]
pub mod family;
