//! Digest helpers for family identity (FAM-18): BLAKE3 in key-derivation mode.

/// Context for `params_digest`.
pub(crate) const PARAMS_CONTEXT: &str = "quip family params v1";
/// Context for `descriptor_digest`.
pub(crate) const DESCRIPTOR_CONTEXT: &str = "quip family descriptor v1";
/// Context for `instance_digest`.
pub(crate) const INSTANCE_CONTEXT: &str = "quip family instance v1";

/// Digest of the canonical parameter bytes.
pub(crate) fn params_digest(params: &[u8]) -> [u8; 32] {
    blake3::derive_key(PARAMS_CONTEXT, params)
}

/// `u8 len(family_id)`, `family_id`, `u32 LE revision`, `params_digest`.
pub(crate) fn descriptor_material(
    family_id: &[u8],
    revision: u32,
    params_digest: &[u8; 32],
) -> Vec<u8> {
    let mut out = Vec::with_capacity(family_id.len() + 37);
    // A family_id past 255 bytes is invalid anyway; saturating keeps the helper total.
    out.push(u8::try_from(family_id.len()).unwrap_or(u8::MAX));
    out.extend_from_slice(family_id);
    out.extend_from_slice(&revision.to_le_bytes());
    out.extend_from_slice(params_digest);
    out
}

/// Digest of the descriptor material.
pub(crate) fn descriptor_digest(
    family_id: &[u8],
    revision: u32,
    params_digest: &[u8; 32],
) -> [u8; 32] {
    blake3::derive_key(
        DESCRIPTOR_CONTEXT,
        &descriptor_material(family_id, revision, params_digest),
    )
}

/// Digest of the canonical instance bytes.
pub(crate) fn instance_digest(canonical: &[u8]) -> [u8; 32] {
    blake3::derive_key(INSTANCE_CONTEXT, canonical)
}
