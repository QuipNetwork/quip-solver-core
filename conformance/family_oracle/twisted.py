"""Independent oracle for `quip/twisted-ice-dimer` revision 1.

The draw order is the one the contract pins: the `left` shuffle, the `right`
shuffle, the tile selection, four `gauge` draws per tile (nodes of the tile in
ascending order), one `weak` draw per non-tile edge in ascending edge order, and
the `dimer` shuffle of the non-tile edges with a greedy walk over the result.
Streams, the shuffle, `select`, and the canonical bytes come from the other
oracle modules. The gauge obstruction diagnostic is a separate port that only
describes an instance.
"""

import struct
from collections import deque
from itertools import combinations

from .chacha8 import ChaCha8, shuffle, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .ice import (
    MAX_DEGREE,
    StructureError,
    candidates,
    select,
    sides,
    zephyr_coordinates,
)
from .instance import canonical_bytes
from .params import TWISTED_DIMER, decode_params

LABELS = (b"left", b"right", b"gauge", b"weak", b"dimer")
STRONG_MILLI = 1000
PAIR_MILLI = -1000
COEFFICIENT_BOUND = STRONG_MILLI
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_BOUNDS_EXCEEDED = 13
CODE_NO_INSTANCE = 14
_KIND_TILES = 1
_KIND_GAUGES = 2
_KIND_PAIRS = 3


def _section(kind: int, payload: bytes) -> bytes:
    return struct.pack("<HI", kind, len(payload)) + payload


def metadata(
    tiles: list[tuple[int, ...]],
    gauges: list[list[int]],
    pairs: list[tuple[int, int]],
) -> bytes:
    """Kinds 1 (tiles), 2 (local gauges), 3 (pairs), as little endian integers."""
    tile_payload = struct.pack("<I", len(tiles))
    for tile in tiles:
        tile_payload += struct.pack("<4I", *tile)
    gauge_payload = struct.pack("<I", len(gauges))
    for gauge in gauges:
        gauge_payload += bytes(0 if g == 1 else 1 for g in gauge)
    pair_payload = struct.pack("<I", len(pairs))
    for a, b in pairs:
        pair_payload += struct.pack("<II", a, b)
    return (
        struct.pack("<H", 3)
        + _section(_KIND_TILES, tile_payload)
        + _section(_KIND_GAUGES, gauge_payload)
        + _section(_KIND_PAIRS, pair_payload)
    )


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted.

    Params faults (codes 7 and 9) come first. Then the topology checks give
    code 12 in the pinned order, and the size bounds give code 13.
    """
    code, values = decode_params(TWISTED_DIMER, params)
    if values is None:
        return code
    m, t, pairs = values["zephyr_m"], values["zephyr_t"], values["pairs"]
    try:
        candidates(m, t, nodes, native_edges)
    except StructureError as err:
        if err.name != "degree-above-limit":
            return CODE_TOPOLOGY_INCOMPATIBLE
    if pairs > len(nodes) // 2:
        return CODE_TOPOLOGY_INCOMPATIBLE
    degree = [0] * len(nodes)
    position = {node: i for i, node in enumerate(nodes)}
    for a, b in native_edges:
        degree[position[a]] += 1
        degree[position[b]] += 1
    if max(degree, default=0) > MAX_DEGREE:
        return CODE_BOUNDS_EXCEEDED
    return None


def gauge_obstructions(
    tiles: list[list[int]], gauges: list[list[int]], n: int
) -> dict[str, int | bool]:
    """Whether the local gauges admit one common node gauge (information only).

    Two tiles that share a node are linked by the product of their signs at
    that node. Potentials come from a breadth-first walk that starts at each
    unvisited tile in ascending order and visits the neighbours of a tile in
    ascending order of (tile, relation). A link is frustrated when the
    potentials of its tiles disagree with its relation. Raises StructureError
    `overflow` for a malformed input, before any membership check, and
    `membership-above-two` with the lowest position in more than two tiles.
    """
    if len(tiles) != len(gauges):
        raise StructureError("overflow")
    membership: dict[int, list[tuple[int, int]]] = {}
    for index, (positions, signs) in enumerate(zip(tiles, gauges, strict=True)):
        for position, sign in zip(positions, signs, strict=False):
            if sign not in (1, -1) or not 0 <= position < n:
                raise StructureError("overflow")
            membership.setdefault(position, []).append((index, sign))
    links: list[tuple[int, int, int]] = []
    neighbours: list[list[tuple[int, int]]] = [[] for _ in tiles]
    for position in sorted(membership):
        group = membership[position]
        if len(group) > 2:
            raise StructureError("membership-above-two", [position])
        if len(group) == 2:
            (a, sign_a), (b, sign_b) = group
            relation = sign_a * sign_b
            links.append((a, b, relation))
            neighbours[a].append((b, relation))
            neighbours[b].append((a, relation))
    potential = [0] * len(tiles)
    components = 0
    for start in range(len(tiles)):
        if potential[start] != 0:
            continue
        components += 1
        potential[start] = 1
        pending = deque([start])
        while pending:
            current = pending.popleft()
            for neighbour, relation in sorted(neighbours[current]):
                if potential[neighbour] == 0:
                    potential[neighbour] = potential[current] * relation
                    pending.append(neighbour)
    frustrated = sum(1 for a, b, r in links if potential[a] * potential[b] != r)
    return {
        "components": components,
        "cycle_rank": len(links) - len(tiles) + components,
        "frustrated_cycles": frustrated,
        "gauge_equivalent": frustrated == 0,
    }


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int | None, dict | None]:
    """`(None, instance)` for one nonce, or `(14, None)` for a pair shortfall.

    The topology must carry tiles and the params must admit.
    """
    code, values = decode_params(TWISTED_DIMER, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t = values["zephyr_m"], values["zephyr_t"]
    weak, want_pairs = values["weak_milli"], values["pairs"]
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

    gauge_rng = stream(LABELS[2])
    gauges = [
        [1 if uniform(gauge_rng, 2) == 0 else -1 for _ in tile] for tile in chosen
    ]

    edges = sorted(
        (min(position[a], position[b]), max(position[a], position[b]))
        for a, b in native_edges
    )
    edge_set = set(edges)
    strong: dict[tuple[int, int], int] = {}
    for tile, gauge in zip(chosen, gauges, strict=True):
        local = dict(zip(tile, gauge, strict=True))
        for a, b in combinations(tile, 2):
            if (a, b) not in edge_set:
                raise ValueError(f"tile pair {(a, b)} is not a topology edge")
            strong[(a, b)] = STRONG_MILLI * local[a] * local[b]

    weak_rng = stream(LABELS[3])
    j = []
    for edge in edges:
        if edge in strong:
            j.append(strong[edge])
        else:
            j.append(weak if uniform(weak_rng, 2) == 0 else -weak)

    free = [at for at, edge in enumerate(edges) if edge not in strong]
    shuffle(stream(LABELS[4]), free)
    used: set[int] = set()
    taken: list[int] = []
    for at in free:
        if len(taken) >= want_pairs:
            break
        u, v = edges[at]
        if u in used or v in used:
            continue
        used.update((u, v))
        taken.append(at)
    if len(taken) < want_pairs:
        return CODE_NO_INSTANCE, None
    for at in taken:
        j[at] = PAIR_MILLI
    pairs = sorted(edges[at] for at in taken)

    h = [0] * len(nodes)
    meta = metadata(chosen, gauges, pairs)
    canonical = canonical_bytes(descriptor, order, nonce, nodes, edges, h, j, 0, meta)
    return None, {
        "node_ids": list(nodes),
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": 0,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": sum(j),
        "tiles": chosen,
        "gauges": gauges,
        "pairs": pairs,
    }
