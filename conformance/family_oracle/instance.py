"""Canonical instance bytes, metadata, bounds, energy, spins, and size bounds."""

import struct

_U64_MAX = 2**64 - 1
_MAX_NODES = 2**24
_MAX_EDGES = 2**26
_MAX_METADATA = 2**26
_COEFFICIENT_BOUND = 2**30
_OFFSET_BOUND = 2**56
_MAX_KIND = 5


def canonical_bytes(
    descriptor_digest: bytes,
    topology_order_digest: bytes,
    nonce: bytes,
    node_ids: list[int],
    edges: list[tuple[int, int]],
    h: list[int],
    j: list[int],
    offset_milli: int,
    metadata: bytes,
) -> bytes:
    """Build the FAM-30 canonical instance bytes. An empty nonce is zero-filled."""
    if nonce == b"":
        nonce = bytes(32)
    if len(nonce) != 32:
        raise ValueError("nonce must be empty or 32 bytes")
    out = bytearray([1])
    out += descriptor_digest + topology_order_digest + nonce
    out += struct.pack("<I", len(node_ids))
    for node in node_ids:
        out += struct.pack("<I", node)
    out += struct.pack("<I", len(edges))
    for u, v in edges:
        out += struct.pack("<II", u, v)
    for value in h:
        out += struct.pack("<i", value)
    for value in j:
        out += struct.pack("<i", value)
    out += struct.pack("<q", offset_milli)
    out += metadata
    return bytes(out)


def edge_fault(edges: list[tuple[int, int]], n_nodes: int) -> str | None:
    """Return the first broken rule: FAM-30 positions, then FAM-32 order."""
    if any(u >= n_nodes or v >= n_nodes for u, v in edges):
        return "FAM-30"
    previous = None
    for edge in edges:
        u, v = edge
        if u >= v:
            return "FAM-32"
        if previous is not None and edge <= previous:
            return "FAM-32"
        previous = edge
    return None


def metadata_code(region: bytes, contract_kinds: list[int]) -> int | None:
    """Return code 16 for a FAM-34 layout fault, code 13 above the size bound."""
    if len(region) > _MAX_METADATA:
        return 13
    if len(region) < 2:
        return 16
    count = struct.unpack_from("<H", region, 0)[0]
    pos = 2
    last_kind = 0
    for _ in range(count):
        if pos + 6 > len(region):
            return 16
        kind, length = struct.unpack_from("<HI", region, pos)
        pos += 6
        if kind == 0 or kind > _MAX_KIND or kind not in contract_kinds:
            return 16
        if kind <= last_kind:
            return 16
        last_kind = kind
        if pos + length > len(region):
            return 16
        pos += length
    if pos != len(region):
        return 16
    return None


def bound_code(quantity: str, value: int) -> int | None:
    """Return code 13 when a FAM-37 value bound is exceeded."""
    limits = {"h": _COEFFICIENT_BOUND, "j": _COEFFICIENT_BOUND, "offset": _OFFSET_BOUND}
    return 13 if abs(value) > limits[quantity] else None


def energy_plain(
    h: list[int], j: list[int], edges: list[tuple[int, int]], spins: list[int]
) -> int:
    """Energy without the offset (FAM-64); add `offset` for the FAM-33 energy."""
    total = sum(hi * si for hi, si in zip(h, spins, strict=True))
    for (u, v), jv in zip(edges, j, strict=True):
        total += jv * spins[u] * spins[v]
    return total


def pack_spins(spins: list[int]) -> bytes:
    out = bytearray((len(spins) + 7) // 8)
    for i, spin in enumerate(spins):
        if spin == 1:
            out[i // 8] |= 1 << (i % 8)
        elif spin != -1:
            raise ValueError("spin must be +1 or -1")
    return bytes(out)


def unpack_spins(data: bytes, n_nodes: int) -> tuple[list[int] | None, str | None]:
    if len(data) != (n_nodes + 7) // 8:
        return None, "wrong-length"
    bits = int.from_bytes(data, "little")
    if bits >> n_nodes:
        return None, "padding-bit-set"
    return [1 if (bits >> i) & 1 else -1 for i in range(n_nodes)], None


def default_node_order(topology_nodes: list[int], used: set[int]) -> list[int]:
    return [node for node in topology_nodes if node in used]


def example_size_bounds(
    n_layers: int, n_nodes: int, n_edges: int, max_degree: int
) -> tuple[int, int, int]:
    """Spec example formulas on exact integers; overflow is checked later."""
    nodes = n_layers * n_nodes
    edges = n_layers * n_edges + (n_layers - 1) * n_nodes
    degree = max_degree + min(n_layers - 1, 2)
    return nodes, edges, degree


def size_bound_code(
    bounds: tuple[int, int, int],
    limit_max_nodes: int | None,
    limit_max_edges: int | None,
    limit_max_degree: int | None,
) -> int | None:
    """Return code 13 on a u64 overflow, a FAM-37 bound, or a claimed limit.

    A limit of None or 0 is no claim.
    """
    nodes, edges, degree = bounds
    if max(nodes, edges, degree) > _U64_MAX:
        return 13
    if nodes > _MAX_NODES or edges > _MAX_EDGES:
        return 13
    if limit_max_nodes and nodes > limit_max_nodes:
        return 13
    if limit_max_edges and edges > limit_max_edges:
        return 13
    if limit_max_degree and degree > limit_max_degree:
        return 13
    return None
