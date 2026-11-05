"""Independent oracle for `quip/ice-fields` revision 1.

The draw order is the one the contract pins: the `left` shuffle, the `right`
shuffle, the tile selection, one `glass` draw per non-tile edge in ascending edge
order, one `gauge` draw per node in registration order, and then the field. The
field lists the nodes in exactly two selected tiles in ascending order, shuffles
the list with the `field` stream, keeps the first `n // field_divisor` nodes,
sorts them, and draws one `uniform(2)` sign per kept node on the `field_sign`
stream. A shortfall of eligible nodes gives code 14. The metadata is the
ice-glass metadata, so this module reuses `glass.metadata`.
"""

from itertools import combinations

from .chacha8 import ChaCha8, shuffle, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .glass import STRONG_MILLI, metadata
from .ice import (
    StructureError,
    candidates,
    has_self_loop,
    select,
    sides,
    size_code,
    slot_counts,
    zephyr_coordinates,
)
from .instance import canonical_bytes
from .params import ICE_FIELDS, decode_params

LABELS = (b"left", b"right", b"glass", b"gauge", b"field", b"field_sign")
COEFFICIENT_BOUND = STRONG_MILLI
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_NO_INSTANCE = 14


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted.

    Params faults (codes 7 and 9) come first. A self-loop, a lattice error, or a
    structure error other than a degree above the limit gives code 12. When the
    structure is sound, a field count above the number of nodes with two tile
    slots also gives code 12. The size bounds then give code 13.
    """
    code, values = decode_params(ICE_FIELDS, params)
    if values is None:
        return code
    if has_self_loop(native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    try:
        tiles, spines = candidates(
            values["zephyr_m"], values["zephyr_t"], nodes, native_edges
        )
    except StructureError as err:
        if err.name != "degree-above-limit":
            return CODE_TOPOLOGY_INCOMPATIBLE
    else:
        count = len(nodes) // values["field_divisor"]
        two_slots = sum(1 for s in slot_counts(len(nodes), tiles, spines) if s == 2)
        if count > two_slots:
            return CODE_TOPOLOGY_INCOMPATIBLE
    return size_code(nodes, native_edges)


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int, dict | None]:
    """`(0, instance)` for one nonce, or `(14, None)` on a shortfall of eligible nodes."""
    code, values = decode_params(ICE_FIELDS, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, glass = values["zephyr_m"], values["zephyr_t"], values["glass_milli"]
    magnitude, divisor = values["field_milli"], values["field_divisor"]
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
    missing = strong.difference(edges)
    if missing:
        raise ValueError(f"tile pairs that are not topology edges: {sorted(missing)}")
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

    seen = [0] * len(nodes)
    for tile in chosen:
        for node in tile:
            seen[node] += 1
    eligible = [i for i, count in enumerate(seen) if count == 2]
    count = len(nodes) // divisor
    if len(eligible) < count:
        return CODE_NO_INSTANCE, None
    shuffle(stream(LABELS[4]), eligible)
    selected = sorted(eligible[:count])
    sign_rng = stream(LABELS[5])
    h = [0] * len(nodes)
    for node in selected:
        sign = 1 if uniform(sign_rng, 2) == 0 else -1
        h[node] = sign * magnitude * gauge[node]

    meta = metadata(chosen, gauge)
    canonical = canonical_bytes(descriptor, order, nonce, nodes, edges, h, j, 0, meta)
    return 0, {
        "node_ids": list(nodes),
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": 0,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": sum(j) + sum(h),
        "tiles": chosen,
        "gauge": gauge,
        "selected": selected,
    }
