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

/// Context for `topology_order_digest`.
pub(crate) const TOPOLOGY_CONTEXT: &str = "quip family topology order v1";
/// Context for stream seeds.
pub(crate) const STREAM_CONTEXT: &str = "quip family stream v1";

fn push_count(out: &mut Vec<u8>, len: usize) {
    // Counts above u32::MAX cannot occur in a fixture; saturating keeps the helper total.
    out.extend_from_slice(&u32::try_from(len).unwrap_or(u32::MAX).to_le_bytes());
}

/// `u32 n_nodes`, node ids in registration order, `u32 n_edges`, each edge
/// `u32 u`, `u32 v` as registered (native ids).
pub(crate) fn topology_order_material(nodes: &[u32], edges: &[(u32, u32)]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + 4 * nodes.len() + 8 * edges.len());
    push_count(&mut out, nodes.len());
    for node in nodes {
        out.extend_from_slice(&node.to_le_bytes());
    }
    push_count(&mut out, edges.len());
    for (u, v) in edges {
        out.extend_from_slice(&u.to_le_bytes());
        out.extend_from_slice(&v.to_le_bytes());
    }
    out
}

/// Digest of the topology order material.
pub(crate) fn topology_order_digest(nodes: &[u32], edges: &[(u32, u32)]) -> [u8; 32] {
    blake3::derive_key(TOPOLOGY_CONTEXT, &topology_order_material(nodes, edges))
}

/// Seed of a substream. The label must be 1 to 32 bytes.
pub(crate) fn stream_seed(
    nonce: &[u8; 32],
    descriptor_digest: &[u8; 32],
    topology_order_digest: &[u8; 32],
    label: &[u8],
) -> Result<[u8; 32], String> {
    if !(1..=32).contains(&label.len()) {
        return Err(format!("label length {} is outside 1..=32", label.len()));
    }
    let mut material = Vec::with_capacity(97 + label.len());
    material.extend_from_slice(nonce);
    material.extend_from_slice(descriptor_digest);
    material.extend_from_slice(topology_order_digest);
    material.push(u8::try_from(label.len()).map_err(|e| e.to_string())?);
    material.extend_from_slice(label);
    Ok(blake3::derive_key(STREAM_CONTEXT, &material))
}

/// `ChaCha8` stream that counts every word drawn.
pub(crate) struct CountingRng {
    rng: quip_protocol::chacha8::ChaCha8Rng,
    pub(crate) used: u64,
}

impl CountingRng {
    pub(crate) fn new(seed: [u8; 32]) -> Self {
        Self {
            rng: quip_protocol::chacha8::ChaCha8Rng::from_seed(seed),
            used: 0,
        }
    }

    pub(crate) fn next_u32(&mut self) -> u32 {
        self.used += 1;
        self.rng.next_u32()
    }
}

/// Unbiased draw in `0..n` (FAM-23): redraw while `x >= 2^32 - (2^32 mod n)`.
pub(crate) fn uniform(rng: &mut CountingRng, n: u32) -> Result<u32, String> {
    if n == 0 {
        return Err("uniform bound is zero".to_owned());
    }
    let n = u64::from(n);
    let limit = (1u64 << 32) - ((1u64 << 32) % n);
    loop {
        let x = u64::from(rng.next_u32());
        if x < limit {
            return u32::try_from(x % n).map_err(|e| e.to_string());
        }
    }
}

/// Fisher-Yates from the back (FAM-24).
pub(crate) fn shuffle(rng: &mut CountingRng, items: &mut [u32]) -> Result<(), String> {
    for i in (1..items.len()).rev() {
        let bound = u32::try_from(i + 1).map_err(|e| e.to_string())?;
        let j = usize::try_from(uniform(rng, bound)?).map_err(|e| e.to_string())?;
        items.swap(i, j);
    }
    Ok(())
}
