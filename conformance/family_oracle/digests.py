"""Domain-separated digests, stream seeds, and legacy lease nonces."""

import struct

import blake3

_U64_MAX = 2**64 - 1


def derive_key(context: str, material: bytes) -> bytes:
    return blake3.blake3(material, derive_key_context=context).digest()


def params_digest(params: bytes) -> bytes:
    return derive_key("quip family params v1", params)


def descriptor_material(family_id: bytes, revision: int, params_digest: bytes) -> bytes:
    return (
        bytes([len(family_id)])
        + family_id
        + struct.pack("<I", revision)
        + params_digest
    )


def descriptor_digest(family_id: bytes, revision: int, params_digest: bytes) -> bytes:
    return derive_key(
        "quip family descriptor v1",
        descriptor_material(family_id, revision, params_digest),
    )


def topology_order_material(nodes: list[int], edges: list[tuple[int, int]]) -> bytes:
    out = struct.pack("<I", len(nodes))
    out += b"".join(struct.pack("<I", node) for node in nodes)
    out += struct.pack("<I", len(edges))
    out += b"".join(struct.pack("<II", u, v) for u, v in edges)
    return out


def topology_order_digest(nodes: list[int], edges: list[tuple[int, int]]) -> bytes:
    return derive_key(
        "quip family topology order v1", topology_order_material(nodes, edges)
    )


def stream_seed(
    nonce: bytes,
    descriptor_digest: bytes,
    topology_order_digest: bytes,
    label: bytes,
) -> bytes:
    material = (
        nonce + descriptor_digest + topology_order_digest + bytes([len(label)]) + label
    )
    return derive_key("quip family stream v1", material)


def instance_digest(instance: bytes) -> bytes:
    return derive_key("quip family instance v1", instance)


def salt_with_counter(base: bytes, counter: int) -> bytes:
    return struct.pack("<Q", counter) + base[8:]


def derive_nonce(last_proof: bytes, miner: bytes, salt: bytes) -> bytes:
    return blake3.blake3(last_proof + miner + salt).digest()


def lease_salts(
    last_proof: bytes,
    miner: bytes,
    base_salt: bytes,
    salt_start: int,
    salt_count: int,
) -> list[tuple[int, bytes, bytes]] | None:
    """Return (counter, salt, nonce) per lease slot, or None if the lease is invalid."""
    if salt_count == 0 or salt_start + salt_count > _U64_MAX:
        return None
    result = []
    for counter in range(salt_start, salt_start + salt_count):
        salt = salt_with_counter(base_salt, counter)
        result.append((counter, salt, derive_nonce(last_proof, miner, salt)))
    return result
