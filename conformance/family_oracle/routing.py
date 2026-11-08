"""Independent oracle for `quip/two-commodity-routing` revision 1.

Written from the pinned construction, not from the Rust code. The template (layout,
arcs, conflict pruning, strongly connected core) depends on the topology and the
parameters. The terminals draw from the `terminal` stream with at most four tries,
each a FAM-24 shuffle of the template node indices, and the costs draw from the
`cost` stream. The energy is built in `x = (1 - s) / 2` form in quarter-milli
integers and then divided by four.
"""

import functools
import struct
from itertools import combinations, permutations

from .chacha8 import ChaCha8, shuffle, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .ice import lattice_edges, linear_id, zephyr_coordinates
from .instance import canonical_bytes, energy_plain
from .params import TWO_COMMODITY, decode_params

FAMILY_ID = "quip/two-commodity-routing"
LABELS = (b"terminal", b"cost")
CODE_TOPOLOGY_INCOMPATIBLE = 12
CODE_BOUNDS_EXCEEDED = 13
CODE_NO_INSTANCE = 14
COEFFICIENT_BOUND = 1000
CONSERVE = 248
CAPACITY = 496
COSTS = (62, 124)
TERMINAL_TRIES = 4
MAX_STRENGTH = 1000
KIND_ROUTING = 7
_MAX_NODES = 131_072
_MAX_EDGES = 2_097_152
_MAX_OFFSET = 1_099_511_627_776
_MAX_METADATA = 1_048_576

Cell = tuple[int, int]


class _Grid:
    """The topology as native ids and native couplers, seen through Zephyr coordinates."""

    def __init__(
        self, m: int, t: int, nodes: list[int], native_edges: list[tuple[int, int]]
    ):
        self.m = m
        self.t = t
        self.side = 2 * m - 1
        self.nodes = set(nodes)
        self.edges = {(min(a, b), max(a, b)) for a, b in native_edges}
        self._k4 = functools.cache(self._k4_uncached)

    def seg(self, u: int, w: int, k: int, p: int) -> int | None:
        if not 0 <= p < 2 * self.m:
            return None
        native = linear_id(self.m, self.t, u, w, k, p % 2, p // 2)
        return native if native in self.nodes else None

    def linked(self, a: int, b: int) -> bool:
        return (min(a, b), max(a, b)) in self.edges

    def corners(self, c: int, r: int, k: int, k2: int) -> tuple[int | None, ...]:
        """(VLO, VHI, HL, HR), each may be None. No clique check."""
        return (
            self.seg(0, c, k, r - 1),
            self.seg(0, c, k, r),
            self.seg(1, r, k2, c - 1),
            self.seg(1, r, k2, c),
        )

    def _k4_uncached(self, c: int, r: int, k: int, k2: int) -> bool:
        quad = self.corners(c, r, k, k2)
        if any(s is None for s in quad):
            return False
        return all(self.linked(a, b) for a, b in combinations(quad, 2))  # type: ignore[arg-type]

    def has_k4(self, c: int, r: int, k: int, k2: int) -> bool:
        return self._k4(c, r, k, k2)

    def alive(self, c: int, r: int, row: tuple[int, ...], col: tuple[int, ...]) -> bool:
        return self.has_k4(c, r, col[0], row[0]) and self.has_k4(
            c, r, col[1 + r % 2], row[1 + c % 2]
        )


def _layout(
    grid: _Grid,
) -> tuple[dict[int, tuple[int, ...]], dict[int, tuple[int, ...]]]:
    """Step 1. Ordered triples of distinct lines, searched in lexicographic order."""
    triples = list(permutations(range(grid.t), 3))
    side = grid.side
    rows = {r: (0, 1, 2) for r in range(1, side + 1)}
    cols = {c: (0, 1, 2) for c in range(1, side + 1)}
    changed = True
    while changed:
        changed = False
        for r in range(1, side + 1):
            scores = [
                sum(grid.alive(c, r, tr, cols[c]) for c in range(1, side + 1))
                for tr in triples
            ]
            current = sum(
                grid.alive(c, r, rows[r], cols[c]) for c in range(1, side + 1)
            )
            if max(scores) > current:
                rows[r] = triples[scores.index(max(scores))]
                changed = True
        for c in range(1, side + 1):
            scores = [
                sum(grid.alive(c, r, rows[r], tc) for r in range(1, side + 1))
                for tc in triples
            ]
            current = sum(
                grid.alive(c, r, rows[r], cols[c]) for r in range(1, side + 1)
            )
            if max(scores) > current:
                cols[c] = triples[scores.index(max(scores))]
                changed = True
    return rows, cols


def _arcs(grid: _Grid, rows: dict, cols: dict) -> list[dict]:
    """Steps 2 and 3."""
    side = grid.side

    def corners1(c: int, r: int) -> tuple[int | None, ...]:
        return grid.corners(c, r, cols[c][0], rows[r][0])

    def corners2(c: int, r: int) -> tuple[int | None, ...]:
        return grid.corners(c, r, cols[c][1 + r % 2], rows[r][1 + c % 2])

    arcs = []
    for c in range(1, side + 1):
        for r in range(1, side + 1):
            candidates = []
            if c + 1 <= side:
                x1 = corners1(c, r)[3]
                chain = (corners2(c, r)[1], corners2(c + 1, r)[2])
                ends = ((c, r), (c + 1, r)) if r % 2 == 0 else ((c + 1, r), (c, r))
                candidates.append((x1, chain, ends))
            if r + 1 <= side:
                x1 = corners1(c, r)[1]
                chain = (corners2(c, r)[3], corners2(c, r + 1)[0])
                ends = ((c, r), (c, r + 1)) if c % 2 == 0 else ((c, r + 1), (c, r))
                candidates.append((x1, chain, ends))
            for x1, chain, (tail, head) in candidates:
                if x1 is None or chain[0] is None or chain[1] is None:
                    continue
                if grid.linked(chain[0], chain[1]) and grid.linked(x1, chain[0]):
                    arcs.append({"tail": tail, "head": head, "x1": x1, "chain": chain})
    return arcs


def _spin(arc: dict, v: Cell | int, k: int) -> int:
    """The spin of commodity `k` in the K4 of the endpoint `v` of `arc`.

    `v` and the `tail` and `head` of the arc must use the same representation,
    either cells or template node indices. Node indices follow cell order, so the
    smaller endpoint is the same in both."""
    if k == 0:
        return arc["x1"]
    return arc["chain"][0] if v == min(arc["tail"], arc["head"]) else arc["chain"][1]


def _prune(grid: _Grid, arcs: list[dict]) -> list[dict]:
    """Step 4."""
    while True:
        incident: dict[Cell, list[int]] = {}
        for index, arc in enumerate(arcs):
            incident.setdefault(arc["tail"], []).append(index)
            incident.setdefault(arc["head"], []).append(index)
        dropped: set[int] = set()
        for v in sorted(incident):
            for k in (0, 1):
                for i, j in combinations(incident[v], 2):
                    if i in dropped or j in dropped:
                        continue
                    if not grid.linked(_spin(arcs[i], v, k), _spin(arcs[j], v, k)):
                        dropped.add(max(i, j))
        if not dropped:
            return arcs
        arcs = [arc for index, arc in enumerate(arcs) if index not in dropped]


def _core(arcs: list[dict]) -> tuple[list[Cell], list[dict]]:
    """Step 5. The largest strongly connected component, ties by smallest cell."""
    ends = sorted({cell for arc in arcs for cell in (arc["tail"], arc["head"])})
    out: dict[Cell, list[Cell]] = {cell: [] for cell in ends}
    back: dict[Cell, list[Cell]] = {cell: [] for cell in ends}
    for arc in arcs:
        out[arc["tail"]].append(arc["head"])
        back[arc["head"]].append(arc["tail"])

    def reach(start: Cell, graph: dict[Cell, list[Cell]]) -> set[Cell]:
        seen = {start}
        stack = [start]
        while stack:
            for nxt in graph[stack.pop()]:
                if nxt not in seen:
                    seen.add(nxt)
                    stack.append(nxt)
        return seen

    done: set[Cell] = set()
    best: list[Cell] = []
    for cell in ends:
        if cell in done:
            continue
        component = sorted(reach(cell, out) & reach(cell, back))
        done.update(component)
        if len(component) > len(best):
            best = component
    keep = set(best)
    return best, [a for a in arcs if a["tail"] in keep and a["head"] in keep]


def template(
    m: int, t: int, nodes: list[int], native_edges: list[tuple[int, int]]
) -> dict | None:
    """The cells and arcs, with native ids for the spins. None when no arc is left."""
    grid = _Grid(m, t, nodes, native_edges)
    rows, cols = _layout(grid)
    arcs = _prune(grid, _arcs(grid, rows, cols))
    if not arcs:
        return None
    cells, kept = _core(arcs)
    index = {cell: i for i, cell in enumerate(cells)}
    if not kept:
        return None
    return {
        "cells": cells,
        "arcs": [
            {
                "tail": index[a["tail"]],
                "head": index[a["head"]],
                "x1": a["x1"],
                "chain": a["chain"],
            }
            for a in kept
        ],
        "grid": grid,
    }


@functools.cache
def _lattice_edge_set(m: int, t: int) -> frozenset[tuple[int, int]]:
    return frozenset(lattice_edges(m, t))


def admission_code(
    params: bytes, nodes: list[int], native_edges: list[tuple[int, int]]
) -> int | None:
    """The admission code of a request, or None when it is admitted.

    Params faults (codes 7 and 9) come first. A node or coupler outside z(m, t),
    or a template without an arc, gives code 12.
    """
    code, values = decode_params(TWO_COMMODITY, params)
    if values is None:
        return code
    m, t = values["zephyr_m"], values["zephyr_t"]
    if any(zephyr_coordinates(m, t, n) is None for n in nodes):
        return CODE_TOPOLOGY_INCOMPATIBLE
    lattice = _lattice_edge_set(m, t)
    if any((min(a, b), max(a, b)) not in lattice for a, b in native_edges):
        return CODE_TOPOLOGY_INCOMPATIBLE
    if template(m, t, nodes, native_edges) is None:
        return CODE_TOPOLOGY_INCOMPATIBLE
    return None


def _routes(n: int, arcs: list[dict], sources: list[int], sinks: list[int]) -> bool:
    """Unit-capacity maximum flow from a super source to a super sink equals the pairs."""
    source, sink = n, n + 1
    capacity: dict[tuple[int, int], int] = {}
    adjacent: dict[int, set[int]] = {v: set() for v in range(n + 2)}

    def add(a: int, b: int) -> None:
        capacity[(a, b)] = capacity.get((a, b), 0) + 1
        capacity.setdefault((b, a), 0)
        adjacent[a].add(b)
        adjacent[b].add(a)

    for arc in arcs:
        add(arc["tail"], arc["head"])
    for v in sources:
        add(source, v)
    for v in sinks:
        add(v, sink)
    flow = 0
    while True:
        parent = {source: source}
        queue = [source]
        for v in queue:
            for w in sorted(adjacent[v]):
                if w not in parent and capacity[(v, w)] > 0:
                    parent[w] = v
                    queue.append(w)
        if sink not in parent:
            return flow == len(sources)
        w = sink
        while w != source:
            v = parent[w]
            capacity[(v, w)] -= 1
            capacity[(w, v)] += 1
            w = v
        flow += 1


def _draw_terminals(
    stream: ChaCha8, count: int, arcs: list[dict], pairs: int
) -> tuple[int, list[list[int]]] | None:
    """`(try, supply)` of the first try whose commodities both route alone."""
    for attempt in range(1, TERMINAL_TRIES + 1):
        order = list(range(count))
        shuffle(stream, order)
        picked = []
        for start in (0, 2 * pairs):
            sources = [order[start + 2 * i] for i in range(pairs)]
            sinks = [order[start + 2 * i + 1] for i in range(pairs)]
            picked.append((sources, sinks))
        if all(_routes(count, arcs, s, d) for s, d in picked):
            supply = [[0, 0] for _ in range(count)]
            for k, (sources, sinks) in enumerate(picked):
                for v in sources:
                    supply[v][k] = 1
                for v in sinks:
                    supply[v][k] = -1
            return attempt, supply
    return None


class _Quarter:
    """The energy in quarter-milli integers, in x form."""

    def __init__(self, n: int):
        self.h4 = [0] * n
        self.j4: dict[tuple[int, int], int] = {}
        self.c4 = 0

    def lin(self, i: int, c: int) -> None:
        self.h4[i] -= 2 * c
        self.c4 += 2 * c

    def pair(self, i: int, j: int, c: int) -> None:
        key = (min(i, j), max(i, j))
        self.j4[key] = self.j4.get(key, 0) + c
        self.h4[i] -= c
        self.h4[j] -= c
        self.c4 += c

    def square(self, w: int, coeffs: list[tuple[int, int]], b0: int) -> None:
        for i, ci in coeffs:
            self.lin(i, w * (ci * ci + 2 * b0 * ci))
        for (i, ci), (j, cj) in combinations(coeffs, 2):
            self.pair(i, j, 2 * w * ci * cj)
        self.c4 += 4 * w * b0 * b0


def _exact_quarter(value: int) -> int:
    if value % 4:
        raise ValueError(f"quarter-milli value {value} does not divide by four")
    return value // 4


def energy(
    n: int, supply: list[list[int]], arcs: list[dict]
) -> tuple[list[int], dict[tuple[int, int], int], int, int]:
    """Step 8. `(h, j, offset, S)` where `arcs` hold node indices and instance positions."""
    q = _Quarter(n)
    for v, supplies in enumerate(supply):
        for k in (0, 1):
            coeffs = [(_spin(a, v, k), 1) for a in arcs if a["tail"] == v]
            coeffs += [(_spin(a, v, k), -1) for a in arcs if a["head"] == v]
            if coeffs:
                q.square(CONSERVE, coeffs, -supplies[k])
    for a in arcs:
        q.pair(a["x1"], a["chain"][0], CAPACITY)
        q.lin(a["x1"], a["cost"][0])
        q.lin(a["chain"][0], a["cost"][1])
    h = [_exact_quarter(x) for x in q.h4]
    j = {key: _exact_quarter(x) for key, x in q.j4.items()}
    offset = _exact_quarter(q.c4)
    load = [abs(x) for x in h]
    for (a, b), value in j.items():
        load[a] += abs(value)
        load[b] += abs(value)
    strength = 1 + max(load[s] for a in arcs for s in a["chain"])
    for a in arcs:
        key = (min(a["chain"]), max(a["chain"]))
        j[key] = j.get(key, 0) - strength
    return h, j, offset, strength


def metadata(
    cells: list[Cell], supply: list[list[int]], arcs: list[dict], strength: int
) -> bytes:
    """The kind 7 section, as a complete region."""
    payload = struct.pack("<I", len(cells))
    for (c, r), (s1, s2) in zip(cells, supply, strict=True):
        payload += struct.pack("<HHbb", c, r, s1, s2)
    payload += struct.pack("<I", len(arcs))
    for a in arcs:
        payload += struct.pack(
            "<5I2H",
            a["tail"],
            a["head"],
            a["x1"],
            a["chain"][0],
            a["chain"][1],
            a["cost"][0],
            a["cost"][1],
        )
    payload += struct.pack("<H", strength)
    return (
        struct.pack("<H", 1) + struct.pack("<HI", KIND_ROUTING, len(payload)) + payload
    )


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int, dict | None]:
    """`(0, instance)`, or `(14, None)` when four terminal tries fail, or `(13, None)`."""
    code, values = decode_params(TWO_COMMODITY, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    m, t, divisor = values["zephyr_m"], values["zephyr_t"], values["pairs_divisor"]
    tpl = template(m, t, nodes, native_edges)
    if tpl is None:
        raise ValueError("the template has no arc")
    descriptor = descriptor_digest(family_id, revision, params_digest(params))
    order = topology_order_digest(nodes, native_edges)

    def stream(label: bytes) -> ChaCha8:
        return ChaCha8(stream_seed(nonce, descriptor, order, label))

    cells, tarcs = tpl["cells"], tpl["arcs"]
    pairs = len(cells) // divisor
    drawn = _draw_terminals(stream(LABELS[0]), len(cells), tarcs, pairs)
    if drawn is None:
        return CODE_NO_INSTANCE, None
    attempt, supply = drawn
    cost_rng = stream(LABELS[1])
    for arc in tarcs:
        arc["cost"] = [COSTS[uniform(cost_rng, 2)], COSTS[uniform(cost_rng, 2)]]

    position = {node: i for i, node in enumerate(nodes)}
    spins = sorted(
        {s for a in tarcs for s in (a["x1"], *a["chain"])}, key=position.__getitem__
    )
    if len(spins) != 3 * len(tarcs):
        raise ValueError("a spin is used twice")
    dense = {s: i for i, s in enumerate(spins)}
    arcs = [
        {
            "tail": a["tail"],
            "head": a["head"],
            "x1": dense[a["x1"]],
            "chain": [dense[a["chain"][0]], dense[a["chain"][1]]],
            "cost": a["cost"],
        }
        for a in tarcs
    ]
    h, jmap, offset, strength = energy(len(spins), supply, arcs)
    if strength > MAX_STRENGTH:
        return CODE_BOUNDS_EXCEEDED, None
    grid = tpl["grid"]
    edges = sorted(key for key, value in jmap.items() if value)
    for a, b in edges:
        if not grid.linked(spins[a], spins[b]):
            raise ValueError("a coupling is not a topology edge")
    j = [jmap[e] for e in edges]
    meta = metadata(cells, supply, arcs, strength)
    if (
        len(spins) > _MAX_NODES
        or len(edges) > _MAX_EDGES
        or max((abs(x) for x in (*h, *j)), default=0) > COEFFICIENT_BOUND
        or abs(offset) > _MAX_OFFSET
        or len(meta) > _MAX_METADATA
    ):
        return CODE_BOUNDS_EXCEEDED, None
    canonical = canonical_bytes(
        descriptor, order, nonce, spins, edges, h, j, offset, meta
    )
    return 0, {
        "node_ids": spins,
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": offset,
        "metadata": meta,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": energy_plain(h, j, edges, [1] * len(spins)) + offset,
        "try": attempt,
        "supply": supply,
        "strength": strength,
        "cells": cells,
        "arcs": arcs,
    }


__all__ = ["admission_code", "generate", "template"]
