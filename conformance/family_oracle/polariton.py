"""Independent oracle for `quip/polariton-static` revision 1.

Written from the pinned construction, not from the Rust code. An open square-link
lattice of side `L` sits on Zephyr cells: each lattice vertex is the K4 of one cell
and each link is one segment qubit shared by two cells. The interior vertices get
balanced charge labels `+2`, `-2`, and `0` from the `charges` stream with at most four
tries, and the energy is the charge penalty `(U / 2) * sum (Q_v - q_v)^2`. The open
lattice, the direct charges, and the feasibility check follow `docs/square-links.md`.
"""

import struct
from itertools import combinations

from .chacha8 import ChaCha8, shuffle
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .ice import zephyr_coordinates
from .instance import canonical_bytes, energy_plain
from .params import POLARITON, decode_params
from .routing import _Grid, _lattice_edge_set

FAMILY_ID = "quip/polariton-static"
LABELS = (b"charges",)
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_BOUNDS_EXCEEDED = 13
CODE_NO_INSTANCE = 14
COEFFICIENT_BOUND = 1000
PENALTY = 250
CHARGE_TRIES = 4
FREE = 127
KIND_CHARGES = 4
KIND_INCIDENCE = 5
_MAX_NODES = 131_072
_MAX_EDGES = 2_097_152
_MAX_OFFSET = 1_099_511_627_776
_MAX_METADATA = 1_048_576


class Lattice:
    """The open lattice of docs/square-links.md: `(L + 1)^2` vertices, `2 L (L + 1)` links."""

    def __init__(self, side: int):
        self.side = side
        self.width = side + 1
        self.links: list[tuple[int, int, str]] = []
        self.incident: list[list[tuple[int, int]]] = [
            [] for _ in range(self.width * self.width)
        ]
        for y in range(self.width):
            for x in range(self.width):
                tail = y * self.width + x
                heads = []
                if x < side:
                    heads.append((tail + 1, "x"))
                if y < side:
                    heads.append((tail + self.width, "y"))
                for head, axis in heads:
                    index = len(self.links)
                    self.links.append((tail, head, axis))
                    self.incident[tail].append((index, -1))
                    self.incident[head].append((index, 1))

    def coordinates(self, vertex: int) -> tuple[int, int]:
        return vertex % self.width, vertex // self.width

    def interior(self) -> list[int]:
        """The interior vertices in vertex order."""
        return [
            y * self.width + x for y in range(1, self.side) for x in range(1, self.side)
        ]

    def charges(self, spins: list[int]) -> list[int]:
        """The charge of every vertex: the sum of incidence times spin over its links."""
        return [sum(inc * spins[e] for e, inc in at) for at in self.incident]


class _Flow:
    def __init__(self, size: int):
        self.cap: list[dict[int, int]] = [{} for _ in range(size)]

    def add(self, a: int, b: int, capacity: int) -> None:
        self.cap[a][b] = self.cap[a].get(b, 0) + capacity
        self.cap[b].setdefault(a, 0)

    def augment(self, source: int, sink: int) -> int:
        """Push augmenting paths found breadth first until none is left."""
        pushed = 0
        while True:
            parent = {source: source}
            queue = [source]
            for v in queue:
                for w in sorted(self.cap[v]):
                    if w not in parent and self.cap[v][w] > 0:
                        parent[w] = v
                        queue.append(w)
            if sink not in parent:
                return pushed
            w = sink
            while w != source:
                v = parent[w]
                self.cap[v][w] -= 1
                self.cap[w][v] += 1
                w = v
            pushed += 1


def orientation(lattice: Lattice, labels: list[int | None]) -> list[int] | None:
    """Spins, one per link, that give every labeled vertex its charge. None if none exist.

    Every link sends one unit to the endpoint it points into. A labeled vertex takes
    exactly `(degree + q) / 2` units. The labeled demands are saturated first and then
    the free vertices are added.
    """
    links = len(lattice.links)
    vertices = len(lattice.incident)
    source, sink = 0, 1 + links + vertices

    def link_node(e: int) -> int:
        return 1 + e

    def vertex_node(v: int) -> int:
        return 1 + links + v

    flow = _Flow(sink + 1)
    for e, (tail, head, _) in enumerate(lattice.links):
        flow.add(source, link_node(e), 1)
        flow.add(link_node(e), vertex_node(tail), 1)
        flow.add(link_node(e), vertex_node(head), 1)
    demand = 0
    for v, label in enumerate(labels):
        if label is not None:
            take = (len(lattice.incident[v]) + label) // 2
            demand += take
            flow.add(vertex_node(v), sink, take)
    if flow.augment(source, sink) != demand:
        return None
    for v, label in enumerate(labels):
        if label is None:
            flow.add(vertex_node(v), sink, len(lattice.incident[v]))
    if demand + flow.augment(source, sink) != links:
        return None
    # A link that gave its unit to its head points tail to head, spin +1.
    return [
        1 if flow.cap[vertex_node(head)][link_node(e)] == 1 else -1
        for e, (_, head, _) in enumerate(lattice.links)
    ]


def check_request(lattice: Lattice, labels: list[int | None]) -> list[int] | None:
    """The orientation when the request is feasible, None when `check_request` fails."""
    for v, label in enumerate(labels):
        if label is None:
            continue
        degree = len(lattice.incident[v])
        if abs(label) > degree or (label - degree) % 2:
            return None
    if all(label is not None for label in labels) and sum(labels) != 0:  # type: ignore[arg-type]
        return None
    return orientation(lattice, labels)


# ---- placement ----


def _layout(grid: _Grid) -> tuple[list[int], list[int]]:
    """Step 1. One line per row and one per column, improved until a sweep changes nothing."""
    side = grid.side
    rows = [0] * (side + 1)
    cols = [0] * (side + 1)
    changed = True
    while changed:
        changed = False
        for r in range(1, side + 1):
            scores = [
                sum(grid.has_k4(c, r, cols[c], k2) for c in range(1, side + 1))
                for k2 in range(grid.t)
            ]
            if max(scores) > scores[rows[r]]:
                rows[r] = scores.index(max(scores))
                changed = True
        for c in range(1, side + 1):
            scores = [
                sum(grid.has_k4(c, r, k, rows[r]) for r in range(1, side + 1))
                for k in range(grid.t)
            ]
            if max(scores) > scores[cols[c]]:
                cols[c] = scores.index(max(scores))
                changed = True
    return rows, cols


def place(
    m: int, t: int, side: int, nodes: list[int], native_edges: list[tuple[int, int]]
) -> dict | None:
    """Steps 1 and 2: the lines and the first fully alive window, or None."""
    grid = _Grid(m, t, nodes, native_edges)
    rows, cols = _layout(grid)
    cells = grid.side
    for c0 in range(1, cells - side + 1):
        for r0 in range(1, cells - side + 1):
            if all(
                grid.has_k4(c0 + x, r0 + y, cols[c0 + x], rows[r0 + y])
                for x in range(side + 1)
                for y in range(side + 1)
            ):
                return {"grid": grid, "rows": rows, "cols": cols, "c0": c0, "r0": r0}
    return None


def _link_qubits(lattice: Lattice, placed: dict) -> list[int | None]:
    """Step 3. The `+x` link is the right horizontal corner of its tail cell and the
    `+y` link is the high vertical corner."""
    grid, rows, cols = placed["grid"], placed["rows"], placed["cols"]
    out = []
    for tail, _, axis in lattice.links:
        x, y = lattice.coordinates(tail)
        c, r = placed["c0"] + x, placed["r0"] + y
        if axis == "x":
            out.append(grid.seg(1, r, rows[r], c))
        else:
            out.append(grid.seg(0, c, cols[c], r))
    return out


# ---- energy ----


def energy(
    lattice: Lattice, labels: list[int | None], positions: list[int], n: int
) -> tuple[list[int], list[tuple[int, int]], list[int], int]:
    """Step 5: `(h, edges, j, offset)` of `(U / 2) * sum (Q_v - q_v)^2` in milli."""
    h = [0] * n
    pairs: dict[tuple[int, int], int] = {}
    offset = 0
    for v, q in enumerate(labels):
        if q is None:
            continue
        at = lattice.incident[v]
        offset += (PENALTY // 2) * (len(at) + q * q)
        for (e, ie), (f, jf) in combinations(at, 2):
            key = (min(positions[e], positions[f]), max(positions[e], positions[f]))
            pairs[key] = pairs.get(key, 0) + PENALTY * ie * jf
        for e, ie in at:
            h[positions[e]] -= PENALTY * q * ie
    edges = sorted(key for key, value in pairs.items() if value)
    return h, edges, [pairs[key] for key in edges], offset


def metadata(
    side: int, labels: list[int | None], c0: int, r0: int, positions: list[int]
) -> bytes:
    """The kind 4 and kind 5 sections, as a complete region."""
    charges = struct.pack("<H", side) + bytes(
        (FREE if q is None else q) & 0xFF for q in labels
    )
    incidence = struct.pack("<HH", c0, r0) + b"".join(
        struct.pack("<I", p) for p in positions
    )
    region = struct.pack("<H", 2)
    for kind, payload in ((KIND_CHARGES, charges), (KIND_INCIDENCE, incidence)):
        region += struct.pack("<HI", kind, len(payload)) + payload
    return region


# ---- admission and generation ----


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted.

    Params faults (codes 7 and 9) come first. A node or coupler outside z(m, t), a
    charge count that does not fit the interior, a lattice that does not fit the cell
    grid, or no fully alive window gives code 12.
    """
    code, values = decode_params(POLARITON, params)
    if values is None:
        return code
    m, t, side, pairs = (
        values["zephyr_m"],
        values["zephyr_t"],
        values["side"],
        values["pairs"],
    )
    if any(zephyr_coordinates(m, t, n) is None for n in nodes):
        return CODE_TOPOLOGY_INCOMPATIBLE
    lattice = _lattice_edge_set(m, t)
    if any((min(a, b), max(a, b)) not in lattice for a, b in native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    if 2 * pairs > (side - 1) ** 2 or side + 1 > 2 * m - 1:
        return CODE_TOPOLOGY_INCOMPATIBLE
    if place(m, t, side, nodes, native_edges) is None:
        return CODE_TOPOLOGY_INCOMPATIBLE
    return None


def size_bounds(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> tuple[int, int, int]:
    """`(N, E, D)`: the links, six couplers per interior vertex, and degree six."""
    _, values = decode_params(POLARITON, params)
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
        min(len(native_edges), 6 * (side - 1) ** 2),
        min(max(degree, default=0), 6, n - 1),
    )


def _labels_of(lattice: Lattice, order: list[int], pairs: int) -> list[int | None]:
    labels: list[int | None] = [None] * len(lattice.incident)
    for rank, v in enumerate(order):
        labels[v] = 2 if rank < pairs else -2 if rank < 2 * pairs else 0
    return labels


def draw_labels(
    stream: ChaCha8, lattice: Lattice, pairs: int
) -> tuple[int, list[int | None], list[int]] | None:
    """Step 4: `(try, labels, orientation)` of the first draw that passes, else None.

    Each try shuffles a fresh copy of the interior list and continues the one stream.
    """
    for attempt in range(1, CHARGE_TRIES + 1):
        order = lattice.interior()
        shuffle(stream, order)
        labels = _labels_of(lattice, order, pairs)
        spins = check_request(lattice, labels)
        if spins is not None:
            return attempt, labels, spins
    return None


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int, dict | None]:
    """`(0, instance)`, or `(14, None)` when four charge draws fail, or `(13, None)`."""
    code, values = decode_params(POLARITON, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, side, pairs = (
        values["zephyr_m"],
        values["zephyr_t"],
        values["side"],
        values["pairs"],
    )
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
    drawn = draw_labels(stream, lattice, pairs)
    if drawn is None:
        return CODE_NO_INSTANCE, None
    attempt, labels, link_spins = drawn
    h, edges, j, offset = energy(lattice, labels, positions, len(node_ids))
    grid = placed["grid"]
    for a, b in edges:
        if not grid.linked(node_ids[a], node_ids[b]):
            raise ValueError("a coupling is not a topology edge")
    meta = metadata(side, labels, placed["c0"], placed["r0"], positions)
    if (
        len(node_ids) > _MAX_NODES
        or len(edges) > _MAX_EDGES
        or max((abs(x) for x in (*h, *j)), default=0) > COEFFICIENT_BOUND
        or abs(offset) > _MAX_OFFSET
        or len(meta) > _MAX_METADATA
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
        "window": (placed["c0"], placed["r0"]),
        "side": side,
        "labels": labels,
        "positions": positions,
        "orientation": link_spins,
    }


# ---- witnesses ----


def _node_spins(inst: dict, link_spins: list[int]) -> list[int]:
    spins = [0] * len(inst["node_ids"])
    for e, position in enumerate(inst["positions"]):
        spins[position] = link_spins[e]
    return spins


def observables(inst: dict, spins: list[int]) -> dict:
    """The direct charges, the violated labels, and the charge loss in milli."""
    lattice = Lattice(inst["side"])
    link_spins = [spins[p] for p in inst["positions"]]
    got = lattice.charges(link_spins)
    gaps = [
        got[v] - q
        for v, q in enumerate(inst["labels"])
        if q is not None and got[v] != q
    ]
    return {
        "charges": got,
        "violations": len(gaps),
        "loss_milli": sum((PENALTY // 2) * gap * gap for gap in gaps),
    }


def encoded_energy(inst: dict, spins: list[int]) -> int:
    return (
        energy_plain(inst["h"], inst["j"], inst["edges"], spins) + inst["offset_milli"]
    )


def witnesses(inst: dict) -> tuple[list[int], list[int]]:
    """`(feasible, infeasible)` instance-node spins.

    The feasible one is the orientation the draw found. The infeasible one flips the
    first link whose flip gives the smallest nonzero loss.
    """
    lattice = Lattice(inst["side"])
    good = list(inst["orientation"])
    best: tuple[int, int] | None = None
    for e in range(len(lattice.links)):
        flipped = list(good)
        flipped[e] = -flipped[e]
        got = lattice.charges(flipped)
        loss = sum(
            (PENALTY // 2) * (got[v] - q) ** 2
            for v, q in enumerate(inst["labels"])
            if q is not None
        )
        if loss and (best is None or loss < best[0]):
            best = (loss, e)
    if best is None:
        raise ValueError("no flip breaks a label")
    bad = list(good)
    bad[best[1]] = -bad[best[1]]
    return _node_spins(inst, good), _node_spins(inst, bad)


__all__ = ["admission_code", "generate", "place", "witnesses"]
