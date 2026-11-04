"""Independent oracle for `quip/charged-ice` revision 1.

The draw order is the one the contract pins: the `left` shuffle, the `right`
shuffle, the tile selection, two `charge` draws per tile (`uniform(256)` then
`uniform(2)`, both always drawn), four `gauge` draws per tile (nodes of the tile
in ascending order), and one `background` draw per non-tile edge in ascending
edge order. Streams, the shuffle, `select`, and the canonical bytes come from the
other oracle modules.

The penalty of a tile t is `(A / 2) * (sum_i g_i s_i - q_t) ** 2`. With
s * s = g * g = 1 it expands to `A * sum_{i<j} g_i g_j s_i s_j
- A * q_t * sum_i g_i s_i + A * (4 + q_t ** 2) / 2`. `direct_energy` evaluates
the penalty without the expansion, so it checks `h`, `J`, and the offset.
"""

import struct
from itertools import combinations

import blake3

from .chacha8 import ChaCha8, shuffle, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .ice import (
    StructureError,
    candidates,
    has_self_loop,
    select,
    sides,
    size_code,
    zephyr_coordinates,
)
from .instance import canonical_bytes, energy_plain
from .params import CHARGED_ICE, decode_params

LABELS = (b"left", b"right", b"charge", b"gauge", b"background")
ENERGY_STATE_LABEL = b"energy-state"
CHARGE_UNIT = 2
COEFFICIENT_BOUND = 1000
CODE_TOPOLOGY_INCOMPATIBLE = 12
_KIND_TILES = 1
_KIND_GAUGES = 2
_KIND_CHARGES = 6


def _section(kind: int, payload: bytes) -> bytes:
    return struct.pack("<HI", kind, len(payload)) + payload


def metadata(
    tiles: list[tuple[int, ...]], gauges: list[list[int]], charges: list[int]
) -> bytes:
    """Kinds 1 (tiles), 2 (local gauges), 6 (tile charges), little endian."""
    tile_payload = struct.pack("<I", len(tiles))
    for tile in tiles:
        tile_payload += struct.pack("<4I", *tile)
    gauge_payload = struct.pack("<I", len(gauges))
    for gauge in gauges:
        gauge_payload += bytes(0 if g == 1 else 1 for g in gauge)
    charge_payload = struct.pack("<I", len(charges)) + b"".join(
        struct.pack("<b", q) for q in charges
    )
    return (
        struct.pack("<H", 3)
        + _section(_KIND_TILES, tile_payload)
        + _section(_KIND_GAUGES, gauge_payload)
        + _section(_KIND_CHARGES, charge_payload)
    )


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted.

    Params faults (codes 7 and 9) come first. A self-loop, a lattice error, or a
    structure error other than a degree above the limit gives code 12. The size
    bounds then give code 13 for a degree above 32 and for a node or edge count
    above the limit.
    """
    code, values = decode_params(CHARGED_ICE, params)
    if values is None:
        return code
    if has_self_loop(native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    try:
        candidates(values["zephyr_m"], values["zephyr_t"], nodes, native_edges)
    except StructureError as err:
        if err.name != "degree-above-limit":
            return CODE_TOPOLOGY_INCOMPATIBLE
    return size_code(nodes, native_edges)


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> dict:
    """The instance of one nonce. The topology must carry tiles."""
    code, values = decode_params(CHARGED_ICE, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t = values["zephyr_m"], values["zephyr_t"]
    scale, background = values["scale_milli"], values["background_milli"]
    charged = values["charged_per_256"]
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

    charge_rng = stream(LABELS[2])
    charges = []
    for _ in chosen:
        chance = uniform(charge_rng, 256)
        sign = uniform(charge_rng, 2)
        if chance >= charged:
            charges.append(0)
        else:
            charges.append(CHARGE_UNIT if sign == 0 else -CHARGE_UNIT)

    gauge_rng = stream(LABELS[3])
    gauges = [
        [1 if uniform(gauge_rng, 2) == 0 else -1 for _ in tile] for tile in chosen
    ]

    edges = sorted(
        (min(position[a], position[b]), max(position[a], position[b]))
        for a, b in native_edges
    )
    edge_set = set(edges)
    strong: dict[tuple[int, int], int] = {}
    h = [0] * len(nodes)
    offset = 0
    for tile, gauge, q in zip(chosen, gauges, charges, strict=True):
        local = dict(zip(tile, gauge, strict=True))
        for a, b in combinations(tile, 2):
            if (a, b) not in edge_set:
                raise ValueError(f"tile pair {(a, b)} is not a topology edge")
            strong[(a, b)] = scale * local[a] * local[b]
        for node in tile:
            h[node] += -scale * q * local[node]
        offset += scale * (4 + q * q) // 2

    background_rng = stream(LABELS[4])
    j = []
    background_signs: dict[tuple[int, int], int] = {}
    for edge in edges:
        if edge in strong:
            j.append(strong[edge])
        else:
            sign = 1 if uniform(background_rng, 2) == 0 else -1
            background_signs[edge] = sign
            j.append(sign * background)

    meta = metadata(chosen, gauges, charges)
    canonical = canonical_bytes(
        descriptor, order, nonce, nodes, edges, h, j, offset, meta
    )
    return {
        "node_ids": list(nodes),
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": offset,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": energy_plain(h, j, edges, [1] * len(nodes)) + offset,
        "tiles": chosen,
        "gauges": gauges,
        "charges": charges,
        "scale": scale,
        "background_milli": background,
        "background_signs": background_signs,
    }


def encoded_energy(inst: dict, spins: list[int]) -> int:
    """The FAM-33 energy of the instance: `h`, `J`, and the offset."""
    return (
        energy_plain(inst["h"], inst["j"], inst["edges"], spins) + inst["offset_milli"]
    )


def tile_sums(inst: dict, spins: list[int]) -> list[int]:
    """The charge `sum_i g_i s_i` of every tile for a spin state."""
    return [
        sum(g * spins[node] for node, g in zip(tile, gauge, strict=True))
        for tile, gauge in zip(inst["tiles"], inst["gauges"], strict=True)
    ]


def charge_defects(inst: dict, spins: list[int]) -> int:
    """Tiles whose charge differs from the prescribed one (information only)."""
    return sum(
        1
        for total, q in zip(tile_sums(inst, spins), inst["charges"], strict=True)
        if total != q
    )


def direct_energy(inst: dict, spins: list[int]) -> int:
    """The sum of the tile penalties and the background couplings.

    A tile contributes `(A / 2) * (charge - q) ** 2`. The result never reads `h`,
    the offset, or any `J`. The background term uses the signs drawn on the
    `background` stream, kept in `background_signs`, and `background_milli`.
    """
    scale = inst["scale"]
    total = 0
    for charge, q in zip(tile_sums(inst, spins), inst["charges"], strict=True):
        square = scale * (charge - q) ** 2
        if square % 2:
            raise ValueError("a tile penalty is not an integer in milli units")
        total += square // 2
    for (u, v), sign in inst["background_signs"].items():
        total += inst["background_milli"] * sign * spins[u] * spins[v]
    return total


def energy_states(nonce: bytes, n: int) -> list[tuple[str, list[int]]]:
    """The four spin states of an energy check, in dense position order.

    `all-up` is +1 everywhere, `all-down` is -1, and `alternating` is +1 at even
    positions. `seeded` draws `uniform(2)` per node from the ChaCha8 stream
    whose seed is the BLAKE3 of the label `energy-state` followed by the nonce.
    A draw of 0 gives +1 and a draw of 1 gives -1.
    """
    rng = ChaCha8(blake3.blake3(ENERGY_STATE_LABEL + nonce).digest())
    seeded = [1 if uniform(rng, 2) == 0 else -1 for _ in range(n)]
    return [
        ("all-up", [1] * n),
        ("all-down", [-1] * n),
        ("alternating", [1 if i % 2 == 0 else -1 for i in range(n)]),
        ("seeded", seeded),
    ]


def spins_hex(spins: list[int]) -> str:
    """One byte per node: 01 for +1 and ff for -1."""
    return bytes(1 if s == 1 else 0xFF for s in spins).hex()
