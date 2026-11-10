"""Independent oracle for `quip/defects-static` revision 1.

Written from the pinned construction, not from the Rust code. The native layout of
`quip/polariton-static` carries an open square-link lattice of side `L`. Link `e` is
a dipolar island at its midpoint, and the physical energy takes its couplings from a
table of the pairs with `K <= 4`. Only the pairs of links that share a vertex are
couplers of the layout, and the exact sum of `|q|` over the other pairs is `dropped`.
A charge penalty and a pin penalty with the weight `U` make every infeasible state
cost more than every feasible state. A route is combinatorial evidence about a state
and never changes `feasible`.
"""

import struct
from collections.abc import Callable
from itertools import combinations, pairwise
from math import isqrt

import blake3

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
from .params import DEFECTS, decode_params
from .polariton import Lattice, _Flow, _link_qubits, place
from .routing import _lattice_edge_set

FAMILY_ID = "quip/defects-static"
LABELS = (b"charges",)
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_BOUNDS_EXCEEDED = 13
CODE_NO_INSTANCE = 14
DRAW_TRIES = 4
FREE = 127
KIND_CHARGES = 4
KIND_INCIDENCE = 5
KIND_DEFECTS = 8
PHYSICAL_STATUS = "InsufficientPhysicalEvidence"

# The dipolar constants of the contract: `D`, the scale `S`, and the radius `R2`.
DIPOLAR_D = 1
DIPOLAR_SCALE = 1000
DIPOLAR_R2 = 4
_GEOMETRY_CONTEXT = "quip dipolar geometry v1"
_TABLE_CONTEXT = "quip dipolar table v1"

# The limits of the contract, for side 3 with both fields at 1000 milli. See the
# Bounds section of the contract for the arithmetic.
MAX_NODES = 24
MAX_EDGES = 96
MAX_COEFFICIENT = 189_328
MAX_OFFSET = 1_318_296
MAX_METADATA = 254


# ---- the dipolar table ----


def _islands(lattice: Lattice) -> list[tuple[int, int, tuple[int, int]]]:
    """Link `e` as `(x, y, axis)`: `+x` from `(x, y)` is at `(2x + 1, 2y)`, `+y` at `(2x, 2y + 1)`."""
    out = []
    for tail, _, axis in lattice.links:
        x, y = lattice.coordinates(tail)
        out.append(
            (2 * x + 1, 2 * y, (1, 0)) if axis == "x" else (2 * x, 2 * y + 1, (0, 1))
        )
    return out


def _pair(a: tuple, b: tuple) -> tuple[int, int]:
    """`(x, K)` for two islands: `x = S * D * N` with every moment 1."""
    dx, dy = b[0] - a[0], b[1] - a[1]
    k = dx * dx + dy * dy
    (ax, ay), (bx, by) = a[2], b[2]
    n = k * (ax * bx + ay * by) - 3 * (ax * dx + ay * dy) * (bx * dx + by * dy)
    return DIPOLAR_SCALE * DIPOLAR_D * n, k


def _quantize(x: int, k: int) -> int:
    """`round(x / K^(5/2))`, a tie away from zero, in integers only.

    `|q|` is the largest `n` with `(2n - 1) * K^(5/2) <= 2 |x|`. Squaring gives
    `2n - 1 <= isqrt(4 x^2 // K^5)`.
    """
    m = isqrt(4 * x * x // k**5)
    n = (m + 1) // 2
    return n if x >= 0 else -n


class Table:
    """The table of `Calibration::Oracle` over the link midpoints of one lattice."""

    def __init__(self, lattice: Lattice):
        islands = _islands(lattice)
        self.islands = islands
        self.entries: list[tuple[int, int, int]] = []
        self.truncation_bound = 0
        for i, j in combinations(range(len(islands)), 2):
            x, k = _pair(islands[i], islands[j])
            if k <= DIPOLAR_R2:
                self.entries.append((i, j, _quantize(x, k)))
            else:
                # The per-pair bound |x| / (K^2 isqrt(K)), rounded up.
                self.truncation_bound += -(-abs(x) // (k * k * isqrt(k)))
        shares = [{tail, head} for tail, head, _ in lattice.links]
        self.kept = [e for e in self.entries if shares[e[0]] & shares[e[1]]]
        self.dropped = sum(
            abs(q) for i, j, q in self.entries if not shares[i] & shares[j]
        )
        self.kept_magnitude = sum(abs(q) for _, _, q in self.kept)

    def geometry_digest(self) -> bytes:
        data = struct.pack("<I", len(self.islands))
        for x, y, axis in self.islands:
            data += struct.pack("<HHBb", x, y, 0 if axis == (1, 0) else 2, 1)
        data += struct.pack("<iII", DIPOLAR_D, DIPOLAR_SCALE, DIPOLAR_R2)
        return blake3.blake3(data, derive_key_context=_GEOMETRY_CONTEXT).digest()

    def commitment(self) -> bytes:
        data = self.geometry_digest() + struct.pack("<Q", len(self.entries))
        for i, j, q in self.entries:
            data += struct.pack("<IIi", i, j, q)
        data += b"\x00"  # the oracle calibration
        return blake3.blake3(data, derive_key_context=_TABLE_CONTEXT).digest()


_TABLES: dict[int, Table] = {}


def table_of(side: int) -> Table:
    if side not in _TABLES:
        _TABLES[side] = Table(Lattice(side))
    return _TABLES[side]


def penalty_weight(table: Table, lattice: Lattice, field_x: int, field_y: int) -> int:
    """D4: `R = 2 * (sum |J| + sum |h|)`, `U = R / 2 + 1`, raised to the next even number."""
    field = sum(
        abs(field_x if axis == "x" else field_y) for _, _, axis in lattice.links
    )
    weight = (2 * (table.kept_magnitude + field)) // 2 + 1
    return weight + weight % 2


# ---- the request ----


def _on_border(lattice: Lattice, v: int) -> bool:
    x, y = lattice.coordinates(v)
    return x in (0, lattice.side) or y in (0, lattice.side)


def boundary_links(lattice: Lattice) -> list[int]:
    """The links with both ends on the outer border, in link order."""
    return [
        e
        for e, (tail, head, _) in enumerate(lattice.links)
        if _on_border(lattice, tail) and _on_border(lattice, head)
    ]


def orientation(
    lattice: Lattice, labels: list[int | None], pins: list[int]
) -> list[int] | None:
    """Spins, one per link, that give every label its charge and every pin spin +1.

    Every link sends one unit to the endpoint it points into. A labeled vertex takes
    exactly `(degree + q) / 2` units. A pinned link points tail to head, so it can only
    give its unit to its head. None when no such state exists.
    """
    links = len(lattice.links)
    source, sink = 0, 1 + links + len(lattice.incident)
    pinned = set(pins)

    flow = _Flow(sink + 1)
    for e, (tail, head, _) in enumerate(lattice.links):
        flow.add(source, 1 + e, 1)
        if e not in pinned:
            flow.add(1 + e, 1 + links + tail, 1)
        flow.add(1 + e, 1 + links + head, 1)
    demand = 0
    for v, label in enumerate(labels):
        if label is not None:
            take = (len(lattice.incident[v]) + label) // 2
            demand += take
            flow.add(1 + links + v, sink, take)
    if flow.augment(source, sink) != demand:
        return None
    for v, label in enumerate(labels):
        if label is None:
            flow.add(1 + links + v, sink, len(lattice.incident[v]))
    if demand + flow.augment(source, sink) != links:
        return None
    return [
        1 if flow.cap[1 + links + head][1 + e] == 1 else -1
        for e, (_, head, _) in enumerate(lattice.links)
    ]


def check_request(
    lattice: Lattice, labels: list[int | None], pins: list[int]
) -> list[int] | None:
    """The orientation when the request is feasible, None when `check_request` fails."""
    for v, label in enumerate(labels):
        if label is None:
            continue
        degree = len(lattice.incident[v])
        if abs(label) > degree or (label - degree) % 2:
            return None
    if all(label is not None for label in labels) and sum(labels) != 0:  # type: ignore[arg-type]
        return None
    return orientation(lattice, labels, pins)


def draw_request(
    stream: ChaCha8, lattice: Lattice, pairs: int, obstacles: int, pins: int
) -> tuple[int, list[int | None], list[int], list[int], list[int]] | None:
    """Step 2: `(try, labels, obstacles, pins, orientation)` of the first draw that passes."""
    border = boundary_links(lattice)
    for attempt in range(1, DRAW_TRIES + 1):
        order = [
            y * lattice.width + x
            for y in range(1, lattice.side)
            for x in range(1, lattice.side)
        ]
        shuffle(stream, order)
        labels: list[int | None] = [None] * len(lattice.incident)
        for rank, v in enumerate(order):
            labels[v] = 2 if rank < pairs else -2 if rank < 2 * pairs else 0
        zeros = [v for v, label in enumerate(labels) if label == 0]
        shuffle(stream, zeros)
        chosen_obstacles = sorted(zeros[:obstacles])
        links = list(border)
        shuffle(stream, links)
        chosen_pins = sorted(links[:pins])
        spins = check_request(lattice, labels, chosen_pins)
        if spins is not None:
            return attempt, labels, chosen_obstacles, chosen_pins, spins
    return None


# ---- energy ----


def energy(
    lattice: Lattice,
    labels: list[int | None],
    pins: list[int],
    table: Table,
    positions: list[int],
    n: int,
    fields: tuple[int, int],
    weight: int,
) -> tuple[list[int], list[tuple[int, int]], list[int], int]:
    """Step 4: `(h, edges, j, offset)` in milli."""
    h = [0] * n
    pairs: dict[tuple[int, int], int] = {}

    def add_pair(e: int, f: int, value: int) -> None:
        key = (min(positions[e], positions[f]), max(positions[e], positions[f]))
        pairs[key] = pairs.get(key, 0) + value

    for e, (_, _, axis) in enumerate(lattice.links):
        h[positions[e]] -= fields[0] if axis == "x" else fields[1]
    for i, j, q in table.kept:
        add_pair(i, j, q)
    offset = 0
    for v, q in enumerate(labels):
        if q is None:
            continue
        at = lattice.incident[v]
        offset += (weight // 2) * (len(at) + q * q)
        for (e, ie), (f, jf) in combinations(at, 2):
            add_pair(e, f, weight * ie * jf)
        for e, ie in at:
            h[positions[e]] -= weight * q * ie
    for e in pins:
        h[positions[e]] -= weight
        offset += weight
    edges = sorted(key for key, value in pairs.items() if value)
    return h, edges, [pairs[key] for key in edges], offset


def _section(kind: int, payload: bytes) -> bytes:
    return struct.pack("<HI", kind, len(payload)) + payload


def metadata(
    side: int,
    labels: list[int | None],
    window: tuple[int, int],
    positions: list[int],
    weight: int,
    table: Table,
    obstacles: list[int],
    pins: list[int],
) -> bytes:
    """The kinds 4, 5, and 8, as a complete region."""
    charges = struct.pack("<H", side) + bytes(
        (FREE if q is None else q) & 0xFF for q in labels
    )
    incidence = struct.pack("<HH", *window) + b"".join(
        struct.pack("<I", p) for p in positions
    )
    defects = struct.pack("<QQ", weight, table.dropped) + table.commitment()
    for ids in (obstacles, pins):
        defects += struct.pack("<H", len(ids)) + b"".join(
            struct.pack("<I", v) for v in ids
        )
    return struct.pack("<H", 3) + b"".join(
        _section(kind, payload)
        for kind, payload in (
            (KIND_CHARGES, charges),
            (KIND_INCIDENCE, incidence),
            (KIND_DEFECTS, defects),
        )
    )


# ---- admission and generation ----


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted."""
    code, values = decode_params(DEFECTS, params)
    if values is None:
        return code
    m, t, side = values["zephyr_m"], values["zephyr_t"], values["side"]
    if any(zephyr_coordinates(m, t, n) is None for n in nodes):
        return CODE_TOPOLOGY_INCOMPATIBLE
    lattice = _lattice_edge_set(m, t)
    if any((min(a, b), max(a, b)) not in lattice for a, b in native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    if (
        2 * values["pairs"] + values["obstacles"] > (side - 1) ** 2
        or values["pins"] > 4 * side
        or side + 1 > 2 * m - 1
    ):
        return CODE_TOPOLOGY_INCOMPATIBLE
    if place(m, t, side, nodes, native_edges) is None:
        return CODE_TOPOLOGY_INCOMPATIBLE
    return None


def size_bounds(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> tuple[int, int, int]:
    """`(N, E, D)`: the links, six couplers per vertex, and degree six."""
    _, values = decode_params(DEFECTS, params)
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


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int, dict | None]:
    """`(0, instance)`, `(14, None)` when four draws fail, or `(13, None)`."""
    code, values = decode_params(DEFECTS, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, side = values["zephyr_m"], values["zephyr_t"], values["side"]
    fields = (values["field_x"], values["field_y"])
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
    drawn = draw_request(
        stream, lattice, values["pairs"], values["obstacles"], values["pins"]
    )
    if drawn is None:
        return CODE_NO_INSTANCE, None
    attempt, labels, obstacles, pins, link_spins = drawn
    table = table_of(side)
    weight = penalty_weight(table, lattice, *fields)
    h, edges, j, offset = energy(
        lattice, labels, pins, table, positions, len(node_ids), fields, weight
    )
    grid = placed["grid"]
    for a, b in edges:
        if not grid.linked(node_ids[a], node_ids[b]):
            raise ValueError("a coupling is not a topology edge")
    window = (placed["c0"], placed["r0"])
    meta = metadata(side, labels, window, positions, weight, table, obstacles, pins)
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
        "orientation": link_spins,
        "obstacles": obstacles,
        "pins": pins,
        "weight": weight,
        "fields": fields,
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

    def done(self) -> bool:
        return self.at == len(self.data)


def _ascending_ids(
    cursor: _Cursor, expected: int, valid: Callable[[int], bool]
) -> list[int]:
    count = cursor.uint(2)
    if count != expected or count * 4 > len(cursor.data) - cursor.at:
        raise ValueError("bad id count")
    ids = [cursor.uint(4) for _ in range(count)]
    if any(a >= b for a, b in pairwise(ids)) or not all(valid(v) for v in ids):
        raise ValueError("bad ids")
    return ids


def parse_metadata(inst: dict) -> dict | None:
    """The labels, positions, obstacles, and pins that the metadata states, or None.

    None unless every rule of the contract holds: the three sections parse exactly,
    the labels, window, and positions are valid, the obstacles and pins are valid,
    the weight, the dropped sum, and the commitment equal the rebuilt values, and the
    energy rebuilt from the metadata equals the instance.
    """
    params = inst["params"]
    side, m = params["side"], params["zephyr_m"]
    lattice = Lattice(side)
    vertices, links = len(lattice.incident), len(lattice.links)
    try:
        outer = _Cursor(inst["metadata"])
        if outer.uint(2) != 3:
            return None
        payloads = []
        for kind in (KIND_CHARGES, KIND_INCIDENCE, KIND_DEFECTS):
            if outer.uint(2) != kind:
                return None
            payloads.append(_Cursor(outer.take(outer.uint(4))))
        if not outer.done():
            return None
        charges, incidence, defect = payloads

        if charges.uint(2) != side:
            return None
        labels: list[int | None] = []
        for v in range(vertices):
            raw = charges.uint(1)
            raw = raw - 256 if raw > 127 else raw
            if _on_border(lattice, v):
                if raw != FREE:
                    return None
                labels.append(None)
            elif raw in (-2, 0, 2):
                labels.append(raw)
            else:
                return None
        if not charges.done():
            return None
        if labels.count(2) != params["pairs"] or labels.count(-2) != params["pairs"]:
            return None

        window = (incidence.uint(2), incidence.uint(2))
        if min(window) < 1 or max(window) + side > 2 * m - 1:
            return None
        positions = [incidence.uint(4) for _ in range(links)]
        if not incidence.done() or sorted(positions) != list(range(links)):
            return None
        if len(inst["node_ids"]) != links:
            return None

        table = table_of(side)
        fields = (params["field_x"], params["field_y"])
        weight = penalty_weight(table, lattice, *fields)
        if (
            defect.uint(8) != weight
            or defect.uint(8) != table.dropped
            or defect.take(32) != table.commitment()
        ):
            return None
        obstacles = _ascending_ids(
            defect,
            params["obstacles"],
            lambda v: v < vertices and not _on_border(lattice, v) and labels[v] == 0,
        )
        border = set(boundary_links(lattice))
        pins = _ascending_ids(defect, params["pins"], lambda e: e in border)
        if not defect.done():
            return None
    except ValueError:
        return None
    rebuilt = energy(lattice, labels, pins, table, positions, links, fields, weight)
    if rebuilt != (inst["h"], inst["edges"], inst["j"], inst["offset_milli"]):
        return None
    return {
        "labels": labels,
        "positions": positions,
        "obstacles": obstacles,
        "pins": pins,
        "fields": fields,
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


def link_spins_of(inst: dict, spins: list[int]) -> list[int]:
    return [spins[p] for p in inst["positions"]]


def encoded_energy(inst: dict, spins: list[int]) -> int:
    return (
        energy_plain(inst["h"], inst["j"], inst["edges"], spins) + inst["offset_milli"]
    )


def observables(inst: dict, spins: list[int]) -> dict | None:
    """The direct charges, violations, and physical energy of a state.

    None when the metadata is invalid or `spins` is not one +1 or -1 per node.
    """
    view = parse_metadata(inst)
    if view is None or not valid_spins(inst, spins):
        return None
    lattice = Lattice(view["side"])
    links = [spins[p] for p in view["positions"]]
    got = lattice.charges(links)
    table = table_of(view["side"])
    physical = sum(q * links[i] * links[j] for i, j, q in table.kept)
    for e, (_, _, axis) in enumerate(lattice.links):
        physical -= view["fields"][0 if axis == "x" else 1] * links[e]
    return {
        "physical_energy": physical,
        "charges": got,
        "violated_labels": sum(
            1 for v, q in enumerate(view["labels"]) if q is not None and got[v] != q
        ),
        "violated_pins": sum(1 for e in view["pins"] if links[e] != 1),
        "truncation_bound": table.truncation_bound,
        "dropped": table.dropped,
        "physical_status": PHYSICAL_STATUS,
    }


def feasible(inst: dict, spins: list[int]) -> bool:
    """Valid metadata, one +-1 spin per node, every label holds, every pin has spin +1."""
    seen = observables(inst, spins)
    return (
        seen is not None and seen["violated_labels"] == 0 and seen["violated_pins"] == 0
    )


def witnesses(inst: dict) -> dict[str, list[int]]:
    """The feasible, infeasible, and pin-violating states, as instance-node spins.

    The feasible one is the orientation of the draw. The infeasible one flips the
    link whose flip breaks a label and leaves the lowest physical energy. The
    pin-violating one flips the first pinned link.
    """
    lattice = Lattice(inst["side"])
    good = list(inst["orientation"])
    best: tuple[int, int] | None = None
    for e in range(len(lattice.links)):
        flipped = list(good)
        flipped[e] = -flipped[e]
        seen = observables(inst, node_spins(inst, flipped))
        if seen["violated_labels"] and (
            best is None or seen["physical_energy"] < best[0]
        ):
            best = (seen["physical_energy"], e)
    if best is None:
        raise ValueError("no flip breaks a label")
    bad = list(good)
    bad[best[1]] = -bad[best[1]]
    out = {
        "feasible": node_spins(inst, good),
        "infeasible": node_spins(inst, bad),
    }
    if inst["pins"]:
        pinned = list(good)
        pinned[inst["pins"][0]] = -1
        out["pin-violating"] = node_spins(inst, pinned)
    return out


# ---- routes ----


def link_between(lattice: Lattice, u: int, v: int) -> int | None:
    """The link that joins distinct vertices `u` and `v`, if there is one."""
    if u == v:
        return None
    for e, _ in lattice.incident[u]:
        tail, head, _ = lattice.links[e]
        if v in (tail, head):
            return e
    return None


def check_route(inst: dict, spins: list[int], route: list[list[int]]) -> str | None:
    """None when the route holds, else the name of the first failed check.

    The checks run in this order over the whole route: `Metadata`, `Spins`, `Shape`,
    `NotAdjacent`, `RepeatedVertex`, `RepeatedLink`, `Obstacle`, `Endpoints`,
    `Difference`.
    """
    view = parse_metadata(inst)
    if view is None:
        return "Metadata"
    if not valid_spins(inst, spins):
        return "Spins"
    lattice = Lattice(view["side"])
    vertices = len(lattice.incident)
    if any(len(path) < 2 or any(not 0 <= v < vertices for v in path) for path in route):
        return "Shape"
    steps = []
    for path in route:
        found = [link_between(lattice, u, v) for u, v in pairwise(path)]
        if None in found:
            return "NotAdjacent"
        steps.append(found)
    if any(len(set(path)) != len(path) for path in route):
        return "RepeatedVertex"
    used = [e for found in steps for e in found]
    if len(set(used)) != len(used):
        return "RepeatedLink"
    if any(v in view["obstacles"] for path in route for v in path):
        return "Obstacle"
    labels = view["labels"]
    ends = [v for path in route for v in (path[0], path[-1])]
    if any({labels[path[0]], labels[path[-1]]} != {2, -2} for path in route) or len(
        set(ends)
    ) != len(ends):
        return "Endpoints"
    if sum(1 for q in labels if q in (2, -2)) != 2 * len(route):
        return "Endpoints"
    flipped = {e for e, p in enumerate(view["positions"]) if spins[p] == -1}
    if flipped != set(used):
        return "Difference"
    return None


def route_links(inst: dict, route: list[list[int]]) -> list[int]:
    """The links of the paths of a route that all steps join."""
    lattice = Lattice(inst["side"])
    out = []
    for path in route:
        for u, v in pairwise(path):
            e = link_between(lattice, u, v)
            if e is None:
                raise ValueError("a step joins no link")
            out.append(e)
    return out


def state_of_flips(inst: dict, links: list[int]) -> list[int]:
    """The reference state with every link in `links` at spin -1, as instance-node spins."""
    flat = [1] * len(inst["positions"])
    for e in links:
        flat[e] = -1
    return node_spins(inst, flat)


def neighbours(lattice: Lattice, v: int) -> list[int]:
    out = []
    for e, _ in lattice.incident[v]:
        tail, head, _ = lattice.links[e]
        out.append(head if tail == v else tail)
    return sorted(out)


def simple_paths(
    lattice: Lattice, start: int, goal: int, forbidden: set[int]
) -> list[list[int]]:
    """Every simple path from `start` to `goal` avoiding `forbidden`, shortest first."""
    found: list[list[int]] = []

    def walk(path: list[int]) -> None:
        here = path[-1]
        if here == goal:
            found.append(list(path))
            return
        for w in neighbours(lattice, here):
            if w not in path and w not in forbidden:
                path.append(w)
                walk(path)
                path.pop()

    walk([start])
    return sorted(found, key=len)


__all__ = ["admission_code", "check_route", "generate", "witnesses"]
