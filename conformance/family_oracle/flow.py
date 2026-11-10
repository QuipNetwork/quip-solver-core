"""Independent oracle for `quip/flow-static` revision 1.

Written from the pinned construction, not from the Rust code. The native layout of
`quip/polariton-static` carries an open square-link lattice of side `L`. Every link
carries one unit of flow, and every vertex has a label. A vertex of label `+1` is a
sink and a vertex of label `-1` is a source. Each link has a requested direction
`t_e` and an even cost `c_e`. The objective is the cost of the links that point
against their request. A charge penalty with the weight `U` makes every state that
breaks a label cost more than every state that keeps all labels.
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
from .ice import zephyr_coordinates
from .instance import canonical_bytes, energy_plain
from .params import FLOW, decode_params
from .polariton import Lattice, _link_qubits, check_request, place
from .routing import _lattice_edge_set

FAMILY_ID = "quip/flow-static"
LABELS = (b"flow",)
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_BOUNDS_EXCEEDED = 13
CODE_NO_INSTANCE = 14
DRAW_TRIES = 4
KIND_CHARGES = 4
KIND_INCIDENCE = 5
KIND_FLOW = 9

# The limits are the smallest that hold for side 64 with cost_half_max 500. They
# come from the arithmetic of limits(), which the oracle tests check.
MAX_NODES = 8_320
MAX_EDGES = 24_574
MAX_COEFFICIENT = 8_320_504
MAX_OFFSET = 35_139_536_892
MAX_METADATA = 79_139


def limits(side: int, half: int) -> tuple[int, int, int, int, int]:
    """`(nodes, edges, coefficient, offset, metadata)` bounds at `side` and `half`.

    Every link carries `half` at most, so `sum u_e <= links * half`. The weight is that
    sum plus 1, rounded up to even. A link has two ends with a label of 0 or one of
    magnitude 1, so the penalty gives `|h| <= 2 U`. The objective adds `half`.
    """
    links = 2 * side * (side + 1)
    weight = links * half + 1
    weight += weight % 2
    edge_vertices = 4 * (side - 1)
    interior = (side - 1) ** 2
    degrees = 2 * links
    return (
        links,
        6 * interior + 3 * edge_vertices + 4,
        2 * weight + half,
        (weight // 2) * (degrees + edge_vertices) + links * half,
        2 + 3 * 6 + (2 + (side + 1) ** 2) + (4 + 4 * links) + (8 + 5 * links),
    )


# ---- the lattice ----


def _on_border(lattice: Lattice, v: int) -> bool:
    x, y = lattice.coordinates(v)
    return x in (0, lattice.side) or y in (0, lattice.side)


def _is_corner(lattice: Lattice, v: int) -> bool:
    x, y = lattice.coordinates(v)
    return x in (0, lattice.side) and y in (0, lattice.side)


def edge_vertices(lattice: Lattice) -> list[int]:
    """The boundary vertices of degree 3, in vertex order."""
    return [
        v
        for v in range(len(lattice.incident))
        if _on_border(lattice, v) and not _is_corner(lattice, v)
    ]


def labels_of(lattice: Lattice, order: list[int]) -> list[int]:
    """The first half of `order` are sinks (`+1`), the rest are sources (`-1`)."""
    labels = [0] * len(lattice.incident)
    for rank, v in enumerate(order):
        labels[v] = 1 if rank < len(order) // 2 else -1
    return labels


# ---- the penalty and the energy ----


def penalty_weight(costs: list[int]) -> int:
    """`U = R / 2 + 1` for `R = sum c_e`, raised by 1 when that is odd."""
    weight = sum(c // 2 for c in costs) + 1
    return weight + weight % 2


def energy(
    lattice: Lattice,
    labels: list[int],
    targets: list[int],
    costs: list[int],
    positions: list[int],
    n: int,
    weight: int,
) -> tuple[list[int], list[tuple[int, int]], list[int], int]:
    """`(h, edges, j, offset)`: `sum c_e (1 - t_e s_e) / 2` plus `(U / 2) sum (Q_v - q_v)^2`."""
    h = [0] * n
    pairs: dict[tuple[int, int], int] = {}
    offset = 0
    for e, (target, cost) in enumerate(zip(targets, costs, strict=True)):
        h[positions[e]] -= (cost // 2) * target
        offset += cost // 2
    for v, q in enumerate(labels):
        at = lattice.incident[v]
        offset += (weight // 2) * (len(at) + q * q)
        for (e, ie), (f, jf) in combinations(at, 2):
            key = (min(positions[e], positions[f]), max(positions[e], positions[f]))
            pairs[key] = pairs.get(key, 0) + weight * ie * jf
        for e, ie in at:
            h[positions[e]] -= weight * q * ie
    edges = sorted(key for key, value in pairs.items() if value)
    return h, edges, [pairs[key] for key in edges], offset


def _section(kind: int, payload: bytes) -> bytes:
    return struct.pack("<HI", kind, len(payload)) + payload


def metadata(
    side: int,
    labels: list[int],
    window: tuple[int, int],
    positions: list[int],
    weight: int,
    targets: list[int],
    costs: list[int],
) -> bytes:
    """The kinds 4, 5, and 9, as a complete region."""
    charges = struct.pack("<H", side) + bytes(q & 0xFF for q in labels)
    incidence = struct.pack("<HH", *window) + b"".join(
        struct.pack("<I", p) for p in positions
    )
    flow = struct.pack("<Q", weight) + b"".join(
        struct.pack("<bI", t, c) for t, c in zip(targets, costs, strict=True)
    )
    return struct.pack("<H", 3) + b"".join(
        _section(kind, payload)
        for kind, payload in (
            (KIND_CHARGES, charges),
            (KIND_INCIDENCE, incidence),
            (KIND_FLOW, flow),
        )
    )


# ---- admission and generation ----


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted."""
    code, values = decode_params(FLOW, params)
    if values is None:
        return code
    m, t, side = values["zephyr_m"], values["zephyr_t"], values["side"]
    if any(zephyr_coordinates(m, t, n) is None for n in nodes):
        return CODE_TOPOLOGY_INCOMPATIBLE
    lattice = _lattice_edge_set(m, t)
    if any((min(a, b), max(a, b)) not in lattice for a, b in native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    if side + 1 > 2 * m - 1:
        return CODE_TOPOLOGY_INCOMPATIBLE
    if place(m, t, side, nodes, native_edges) is None:
        return CODE_TOPOLOGY_INCOMPATIBLE
    return None


def size_bounds(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> tuple[int, int, int]:
    """`(N, E, D)`: the links, six couplers per vertex, and degree six."""
    _, values = decode_params(FLOW, params)
    if values is None:
        raise ValueError("params rejected")
    side = values["side"]
    degree = [0] * len(nodes)
    position = {node: i for i, node in enumerate(nodes)}
    for a, b in native_edges:
        degree[position[a]] += 1
        degree[position[b]] += 1
    n = min(len(nodes), 2 * side * (side + 1))
    return (
        n,
        min(len(native_edges), 6 * (side + 1) ** 2),
        min(max(degree, default=0), 6, n - 1),
    )


def draw_labels(
    stream: ChaCha8, lattice: Lattice
) -> tuple[int, list[int], list[int]] | None:
    """`(try, labels, orientation)` of the first draw that passes, else None.

    Each try shuffles a fresh copy of the edge vertices in vertex order and continues
    the one stream.
    """
    for attempt in range(1, DRAW_TRIES + 1):
        order = edge_vertices(lattice)
        shuffle(stream, order)
        labels = labels_of(lattice, order)
        spins = check_request(lattice, list(labels))
        if spins is not None:
            return attempt, labels, spins
    return None


def draw_links(stream: ChaCha8, links: int, half: int) -> tuple[list[int], list[int]]:
    """The targets and the costs: for each link, `uniform(2)` and then `uniform(half + 1)`."""
    targets, costs = [], []
    for _ in range(links):
        targets.append(1 if uniform(stream, 2) == 1 else -1)
        costs.append(2 * uniform(stream, half + 1))
    return targets, costs


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int, dict | None]:
    """`(0, instance)`, `(14, None)` when four draws fail, or `(13, None)`."""
    code, values = decode_params(FLOW, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, side = values["zephyr_m"], values["zephyr_t"], values["side"]
    half = values["cost_half_max"]
    placed = place(m, t, side, nodes, native_edges)
    if placed is None:
        raise ValueError("no window is alive")
    lattice = Lattice(side)
    qubits = _link_qubits(lattice, placed)
    if any(q is None for q in qubits) or len(set(qubits)) != len(qubits):
        raise ValueError("the link qubits are missing or shared")
    topology_position = {node: i for i, node in enumerate(nodes)}
    node_ids = sorted(qubits, key=topology_position.__getitem__)  # type: ignore[arg-type]
    dense = {node: i for i, node in enumerate(node_ids)}
    positions = [dense[q] for q in qubits]  # type: ignore[index]
    descriptor = descriptor_digest(family_id, revision, params_digest(params))
    order = topology_order_digest(nodes, native_edges)
    stream = ChaCha8(stream_seed(nonce, descriptor, order, LABELS[0]))
    drawn = draw_labels(stream, lattice)
    if drawn is None:
        return CODE_NO_INSTANCE, None
    attempt, labels, orientation = drawn
    targets, costs = draw_links(stream, len(lattice.links), half)
    weight = penalty_weight(costs)
    h, edges, j, offset = energy(
        lattice, labels, targets, costs, positions, len(node_ids), weight
    )
    grid = placed["grid"]
    for a, b in edges:
        if not grid.linked(node_ids[a], node_ids[b]):
            raise ValueError("a coupling is not a topology edge")
    window = (placed["c0"], placed["r0"])
    meta = metadata(side, labels, window, positions, weight, targets, costs)
    if (
        len(node_ids) > MAX_NODES
        or len(edges) > MAX_EDGES
        or max((abs(x) for x in (*h, *j)), default=0) > MAX_COEFFICIENT
        or abs(offset) > MAX_OFFSET
        or len(meta) > MAX_METADATA
    ):
        return CODE_BOUNDS_EXCEEDED, None
    canonical = canonical_bytes(
        descriptor, order, nonce, node_ids, edges, h, j, offset, meta
    )
    return 0, {
        "node_ids": node_ids,
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": offset,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": energy_plain(h, j, edges, [1] * len(node_ids)) + offset,
        "try": attempt,
        "window": window,
        "side": side,
        "labels": labels,
        "positions": positions,
        "orientation": orientation,
        "targets": targets,
        "costs": costs,
        "weight": weight,
        "params": values,
    }


# ---- the metadata check ----


class _Cursor:
    """Reads little-endian integers from a byte string and fails on a short read."""

    def __init__(self, data: bytes):
        self.data = data
        self.at = 0

    def take(self, size: int) -> bytes:
        if size < 0 or self.at + size > len(self.data):
            raise ValueError("short read")
        chunk = self.data[self.at : self.at + size]
        self.at += size
        return chunk

    def uint(self, size: int) -> int:
        return int.from_bytes(self.take(size), "little")

    def sint(self, size: int) -> int:
        return int.from_bytes(self.take(size), "little", signed=True)

    def done(self) -> bool:
        return self.at == len(self.data)


def _labels_follow_the_request(lattice: Lattice, labels: list[int]) -> bool:
    """Every vertex is labeled: interior and corners 0, edge vertices `+-1` in equal counts."""
    plus = minus = 0
    for v, q in enumerate(labels):
        if _on_border(lattice, v) and not _is_corner(lattice, v):
            if q not in (-1, 1):
                return False
            plus += q == 1
            minus += q == -1
        elif q != 0:
            return False
    return plus == minus


def parse_metadata(inst: dict) -> dict | None:
    """The labels, positions, targets, and costs that the metadata states, or None.

    None unless every rule of the contract holds: the three sections parse exactly,
    the labels follow the request, the window and positions are valid, every target
    is `+-1`, every cost is even and at most `2 * cost_half_max`, `U` equals the
    weight of the costs, and the energy rebuilt from the metadata equals the instance.
    """
    params = inst["params"]
    side, m, half = params["side"], params["zephyr_m"], params["cost_half_max"]
    lattice = Lattice(side)
    vertices, links = len(lattice.incident), len(lattice.links)
    try:
        outer = _Cursor(inst["metadata"])
        if outer.uint(2) != 3:
            return None
        payloads = []
        for kind in (KIND_CHARGES, KIND_INCIDENCE, KIND_FLOW):
            if outer.uint(2) != kind:
                return None
            payloads.append(_Cursor(outer.take(outer.uint(4))))
        if not outer.done():
            return None
        charges, incidence, flow = payloads

        if charges.uint(2) != side:
            return None
        labels = [charges.sint(1) for _ in range(vertices)]
        if not charges.done() or not _labels_follow_the_request(lattice, labels):
            return None

        window = (incidence.uint(2), incidence.uint(2))
        if min(window) < 1 or max(window) + side > 2 * m - 1:
            return None
        positions = [incidence.uint(4) for _ in range(links)]
        if not incidence.done() or sorted(positions) != list(range(links)):
            return None
        if len(inst["node_ids"]) != links:
            return None

        stored = flow.uint(8)
        targets, costs = [], []
        for _ in range(links):
            target, cost = flow.sint(1), flow.uint(4)
            if target not in (-1, 1) or cost % 2 or cost > 2 * half:
                return None
            targets.append(target)
            costs.append(cost)
        if not flow.done() or stored != penalty_weight(costs):
            return None
    except ValueError:
        return None
    weight = penalty_weight(costs)
    rebuilt = energy(lattice, labels, targets, costs, positions, links, weight)
    if rebuilt != (inst["h"], inst["edges"], inst["j"], inst["offset_milli"]):
        return None
    return {
        "labels": labels,
        "positions": positions,
        "targets": targets,
        "costs": costs,
        "weight": weight,
        "side": side,
    }


def valid_spins(inst: dict, spins: list[int]) -> bool:
    """One +1 or -1 entry for each instance node."""
    return len(spins) == len(inst["node_ids"]) and all(s in (-1, 1) for s in spins)


# ---- states ----


def node_spins(inst: dict, link_spins: list[int]) -> list[int]:
    """Instance-node spins from the spins of the links."""
    spins = [0] * len(inst["node_ids"])
    for e, position in enumerate(inst["positions"]):
        spins[position] = link_spins[e]
    return spins


def encoded_energy(inst: dict, spins: list[int]) -> int:
    return (
        energy_plain(inst["h"], inst["j"], inst["edges"], spins) + inst["offset_milli"]
    )


def observables(inst: dict, spins: list[int]) -> dict | None:
    """The objective, the direct charges, the violations, and the reversed links.

    None when the metadata is invalid or `spins` is not one +1 or -1 per node.
    """
    view = parse_metadata(inst)
    if view is None or not valid_spins(inst, spins):
        return None
    lattice = Lattice(view["side"])
    links = [spins[p] for p in view["positions"]]
    got = lattice.charges(links)
    against = [e for e, t in enumerate(view["targets"]) if links[e] != t]
    return {
        "objective": sum(view["costs"][e] for e in against),
        "charges": got,
        "violations": sum(
            1 for g, q in zip(got, view["labels"], strict=True) if g != q
        ),
        "reversed": len(against),
    }


def feasible(inst: dict, spins: list[int]) -> bool:
    """Valid metadata, one +-1 spin per node, and every label holds."""
    seen = observables(inst, spins)
    return seen is not None and seen["violations"] == 0


def network(inst: dict) -> dict | None:
    """The supplies and the arcs of the flow network, or None when the metadata is invalid.

    The supply of a vertex is its negated label. An arc is a link from its tail to its
    head with the target and the cost.
    """
    view = parse_metadata(inst)
    if view is None:
        return None
    lattice = Lattice(view["side"])
    return {
        "supplies": [-q for q in view["labels"]],
        "tails": [tail for tail, _, _ in lattice.links],
        "heads": [head for _, head, _ in lattice.links],
        "targets": list(view["targets"]),
        "costs": list(view["costs"]),
    }


def _all_states(links: int) -> list[list[int]]:
    return [
        [1 if bits >> e & 1 else -1 for e in range(links)] for bits in range(1 << links)
    ]


def witnesses(inst: dict, enumerate_all: bool) -> dict[str, list[int]]:
    """The feasible and the infeasible states, as instance-node spins.

    With `enumerate_all` true, the feasible state is the one of lowest objective over
    every state (the first in bit order on a tie), and the infeasible state is the one
    of lowest objective that breaks a label. Otherwise the feasible state is the
    orientation that the draw found, and the infeasible state is the requested
    direction of every link. The infeasible state must have a lower objective than
    the feasible one.
    """
    view = parse_metadata(inst)
    if view is None:
        raise ValueError("the metadata is invalid")
    lattice = Lattice(view["side"])
    links = len(lattice.links)

    def objective(state: list[int]) -> int:
        return sum(
            c
            for s, t, c in zip(state, view["targets"], view["costs"], strict=True)
            if s != t
        )

    def holds(state: list[int]) -> bool:
        return lattice.charges(state) == view["labels"]

    if enumerate_all:
        states = _all_states(links)
        good = min((s for s in states if holds(s)), key=objective)
        bad = min((s for s in states if not holds(s)), key=objective)
    else:
        good = list(inst["orientation"])
        bad = list(view["targets"])
    if not holds(good) or holds(bad) or objective(bad) >= objective(good):
        raise ValueError(
            "the witnesses must be feasible and infeasible with a lower objective"
        )
    return {
        "feasible": node_spins(inst, good),
        "infeasible": node_spins(inst, bad),
    }
