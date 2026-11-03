"""Independent oracle for `quip/ice-glass` revision 1.

The draw order is the one the contract pins: the `left` shuffle, the `right`
shuffle, the tile selection, one `glass` draw per non-tile edge in ascending
edge order, then one `gauge` draw per node in registration order. Streams, the
shuffle, `select`, and the canonical bytes come from the other oracle modules.
"""

import struct
from itertools import combinations

from .chacha8 import ChaCha8, shuffle, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .ice import candidates, select, sides, zephyr_coordinates
from .instance import canonical_bytes
from .params import ICE_GLASS, decode_params

LABELS = (b"left", b"right", b"glass", b"gauge")
STRONG_MILLI = 1000
COEFFICIENT_BOUND = STRONG_MILLI
_KIND_TILES = 1
_KIND_GAUGE = 2


def _section(kind: int, payload: bytes) -> bytes:
    return struct.pack("<HI", kind, len(payload)) + payload


def metadata(tiles: list[tuple[int, ...]], gauge: list[int]) -> bytes:
    """Kind 1 holds the tiles, kind 2 the gauge, as `u32` little endian."""
    tile_payload = struct.pack("<I", len(tiles))
    for tile in tiles:
        tile_payload += struct.pack("<4I", *tile)
    gauge_payload = struct.pack("<I", len(gauge)) + bytes(
        0 if g == 1 else 1 for g in gauge
    )
    return (
        struct.pack("<H", 2)
        + _section(_KIND_TILES, tile_payload)
        + _section(_KIND_GAUGE, gauge_payload)
    )


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> dict:
    """The instance of one nonce. The topology must carry tiles."""
    code, values = decode_params(ICE_GLASS, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, glass = values["zephyr_m"], values["zephyr_t"], values["glass_milli"]
    position = {node: i for i, node in enumerate(nodes)}
    tiles, spines = candidates(m, t, nodes, native_edges)
    coords = [zephyr_coordinates(m, t, n) for n in nodes] if m else []
    left, right = sides(m, coords, spines)  # type: ignore[arg-type]

    descriptor = descriptor_digest(family_id, revision, params_digest(params))
    order = topology_order_digest(nodes, native_edges)

    def stream(label: bytes) -> ChaCha8:
        return ChaCha8(stream_seed(nonce, descriptor, order, label))

    left_order = list(range(len(left)))
    shuffle(stream(LABELS[0]), left_order)
    right_order = list(range(len(right)))
    shuffle(stream(LABELS[1]), right_order)
    chosen = select(tiles, spines, left, right, left_order, right_order)

    edges = sorted(
        (min(position[a], position[b]), max(position[a], position[b]))
        for a, b in native_edges
    )
    strong = {pair for tile in chosen for pair in combinations(tile, 2)}
    glass_rng = stream(LABELS[2])
    j = []
    for edge in edges:
        if edge in strong:
            j.append(STRONG_MILLI)
        else:
            j.append(glass if uniform(glass_rng, 2) == 0 else -glass)
    gauge_rng = stream(LABELS[3])
    gauge = [1 if uniform(gauge_rng, 2) == 0 else -1 for _ in nodes]
    j = [value * gauge[u] * gauge[v] for (u, v), value in zip(edges, j, strict=True)]

    h = [0] * len(nodes)
    meta = metadata(chosen, gauge)
    canonical = canonical_bytes(descriptor, order, nonce, nodes, edges, h, j, 0, meta)
    return {
        "node_ids": list(nodes),
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": 0,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": sum(j),
        "tile_count": len(chosen),
    }
