"""Independent oracle for the ice tile structure: Zephyr lattice, candidate tiles,
spines, sides, and the order-driven matching.

Nothing here reads Rust output. Every value is computed from the pinned rules:
the Zephyr coupler formulas of `dwave_networkx.zephyr_graph`, the check order,
the two enumeration modes, the spine rule, the two-colouring of the spine graph,
and the recursive augmenting-path `select`. Dense positions are indexes into the
node list in registration order.
"""

import sys
from collections.abc import Sequence
from itertools import combinations

from .chacha8 import ChaCha8, shuffle

MAX_DEGREE = 32
MAX_NODES = 131_072
MAX_EDGES = 2_097_152
CODE_BOUNDS_EXCEEDED = 13

Edge = tuple[int, int]
Spine = tuple[Edge, Edge]


class StructureError(Exception):
    """A topology that cannot carry ice tiles.

    `name` is the kebab-case variant. `detail` holds the native ids of the
    payload: `[id]`, `[a, b]` with `a < b`, or empty.
    """

    def __init__(self, name: str, detail: Sequence[int] = ()) -> None:
        super().__init__(name)
        self.name = name
        self.detail = list(detail)


# ---- Zephyr lattice ----


def node_count(m: int, t: int) -> int:
    return 4 * t * m * (2 * m + 1)


def linear_id(m: int, t: int, u: int, w: int, k: int, j: int, z: int) -> int:
    return (((u * (2 * m + 1) + w) * t + k) * 2 + j) * m + z


def zephyr_coordinates(m: int, t: int, q: int) -> tuple[int, int, int, int, int] | None:
    """The index (u, w, k, j, z) of a linear id, or None when out of range."""
    if m < 1 or t < 1 or not 0 <= q < node_count(m, t):
        return None
    rest, z = divmod(q, m)
    rest, j = divmod(rest, 2)
    rest, k = divmod(rest, t)
    u, w = divmod(rest, 2 * m + 1)
    return (u, w, k, j, z)


def lattice_edges(m: int, t: int) -> list[Edge]:
    """Every z(m, t) coupler as (a, b) with a < b, ascending."""
    found: set[Edge] = set()

    def add(a: int, b: int) -> None:
        found.add((min(a, b), max(a, b)))

    for u in range(2):
        for w in range(2 * m + 1):
            for k in range(t):
                for j in range(2):
                    for z in range(m - 1):  # external
                        add(
                            linear_id(m, t, u, w, k, j, z),
                            linear_id(m, t, u, w, k, j, z + 1),
                        )
                for z in range(m):  # odd
                    for a in (0, 1):
                        if a <= z:
                            add(
                                linear_id(m, t, u, w, k, 0, z),
                                linear_id(m, t, u, w, k, 1, z - a),
                            )
    for w in range(m):  # internal
        for z in range(m):
            for h in range(t):
                for k in range(t):
                    for i in (0, 1):
                        for j in (0, 1):
                            for a in (0, 1):
                                for b in (0, 1):
                                    add(
                                        linear_id(
                                            m,
                                            t,
                                            0,
                                            2 * w + 1 + a * (2 * i - 1),
                                            k,
                                            j,
                                            z,
                                        ),
                                        linear_id(
                                            m,
                                            t,
                                            1,
                                            2 * z + 1 + b * (2 * j - 1),
                                            h,
                                            i,
                                            w,
                                        ),
                                    )
    return sorted(found)


def lattice(m: int, t: int) -> tuple[list[int], list[Edge]]:
    return list(range(node_count(m, t))), lattice_edges(m, t)


def live_topology(
    removed_nodes: Sequence[int], removed_edges: Sequence[Sequence[int]]
) -> tuple[list[int], list[Edge]]:
    """The z(12, 4) lattice minus the removed nodes and removed edges."""
    nodes, edges = lattice(12, 4)
    gone = set(removed_nodes)
    cut = {(a, b) for a, b in removed_edges}
    return (
        [n for n in nodes if n not in gone],
        [e for e in edges if e[0] not in gone and e[1] not in gone and e not in cut],
    )


# ---- candidates ----


def _max_degree(count: int, dense: Sequence[Edge]) -> int:
    degree = [0] * count
    for a, b in dense:
        degree[a] += 1
        degree[b] += 1
    return max(degree, default=0)


def has_self_loop(edges: Sequence[Edge]) -> bool:
    return any(a == b for a, b in edges)


def size_code(nodes: Sequence[int], native_edges: Sequence[Edge]) -> int | None:
    """Code 13 when the topology breaks a size limit, else None.

    The limits are the node count, the edge count, and the maximum degree of the
    ice families. Every ice family shares them.
    """
    position = {node: i for i, node in enumerate(nodes)}
    dense = [(position[a], position[b]) for a, b in native_edges]
    if (
        len(nodes) > MAX_NODES
        or len(native_edges) > MAX_EDGES
        or _max_degree(len(nodes), dense) > MAX_DEGREE
    ):
        return CODE_BOUNDS_EXCEEDED
    return None


def _tile_edges(tile: Sequence[int]) -> list[Edge]:
    return list(combinations(tile, 2))  # type: ignore[arg-type]


def _matchings(tile: Sequence[int]) -> list[Spine]:
    a, b, c, d = tile
    return [((a, b), (c, d)), ((a, c), (b, d)), ((a, d), (b, c))]


def _adjacency_spines(tiles: list[tuple[int, ...]]) -> list[Spine | None]:
    count: dict[Edge, int] = {}
    for tile in tiles:
        for edge in _tile_edges(tile):
            count[edge] = count.get(edge, 0) + 1
    spines: list[Spine | None] = []
    for tile in tiles:
        held = [e for e in _tile_edges(tile) if count[e] >= 2]
        if not held:
            spines.append(None)
            continue
        for matching in _matchings(tile):
            if all(e in matching for e in held):
                spines.append(matching)
                break
        else:
            raise StructureError("spine-conflict")
    return spines


def _zephyr_tiles(
    coords: list[tuple[int, int, int, int, int]],
    edge_set: set[Edge],
    neighbours: list[set[int]],
) -> dict[tuple[int, ...], Spine]:
    def odd(a: int, b: int, u: int) -> bool:
        ca, cb = coords[a], coords[b]
        return (
            ca[0] == u and cb[0] == u and ca[1] == cb[1] and ca[2] == cb[2]
            and ca[3] != cb[3]
        )  # fmt: skip

    found: dict[tuple[int, ...], Spine] = {}
    for a, b in sorted(edge_set):
        if not odd(a, b, 0):
            continue
        common = sorted(neighbours[a] & neighbours[b])
        for c, d in combinations(common, 2):
            if (c, d) in edge_set and odd(c, d, 1):
                spine = tuple(sorted(((a, b), (c, d))))
                found[tuple(sorted((a, b, c, d)))] = spine  # type: ignore[assignment]
    return found


def _adjacency_tiles(
    neighbours: list[set[int]], edge_count: int
) -> list[tuple[int, ...]]:
    """Every 4-clique. More than `edge_count // 4` of them is a spine conflict.

    Without a spine conflict each tile shares at most its 2 spine edges, so it
    owns at least 4 of its 6 edges alone and `4 * tiles <= edges`.
    """
    cap = edge_count // 4
    tiles = []
    for a, na in enumerate(neighbours):
        for b in sorted(x for x in na if x > a):
            above = sorted(c for c in na & neighbours[b] if c > b)
            for c, d in combinations(above, 2):
                if d in neighbours[c]:
                    if len(tiles) >= cap:
                        raise StructureError("spine-conflict")
                    tiles.append((a, b, c, d))
    return tiles


def candidates(
    m: int,
    t: int,
    nodes: Sequence[int],
    edges: Sequence[Edge],
) -> tuple[list[tuple[int, ...]], list[Spine | None]]:
    """Tiles ascending and a parallel spine list, or raise StructureError.

    `(m, t)` is `(0, 0)` for adjacency mode. `edges` are native ids in
    registration order.
    """
    if (m == 0) != (t == 0):
        raise StructureError("mixed-shape")
    position = {n: i for i, n in enumerate(nodes)}
    dense = [(position[a], position[b]) for a, b in edges]
    if _max_degree(len(nodes), dense) > MAX_DEGREE:
        raise StructureError("degree-above-limit")
    edge_set: set[Edge] = set()
    for a, b in dense:
        pair = (min(a, b), max(a, b))
        if a == b or pair in edge_set:
            raise StructureError("duplicate-edge")
        edge_set.add(pair)
    zephyr = m != 0
    coords: list[tuple[int, int, int, int, int]] = []
    if zephyr:
        for n in nodes:
            c = zephyr_coordinates(m, t, n)
            if c is None:
                raise StructureError("not-zephyr-node", [n])
            coords.append(c)
        coupler = set(lattice_edges(m, t))
        for a, b in edges:
            if (min(a, b), max(a, b)) not in coupler:
                raise StructureError("not-zephyr-coupler", [min(a, b), max(a, b)])
    neighbours: list[set[int]] = [set() for _ in nodes]
    for a, b in edge_set:
        neighbours[a].add(b)
        neighbours[b].add(a)
    spines: list[Spine | None]
    if zephyr:
        by_tile = _zephyr_tiles(coords, edge_set, neighbours)
        tiles = sorted(by_tile)
        spines = [by_tile[tile] for tile in tiles]
    else:
        tiles = sorted(set(_adjacency_tiles(neighbours, len(edge_set))))
        spines = _adjacency_spines(tiles)
    if not tiles:
        raise StructureError("no-candidates")
    _check_slots(nodes, tiles, spines)
    sides(m, coords, spines)
    return tiles, spines


def slot_counts(
    count: int, tiles: list[tuple[int, ...]], spines: list[Spine | None]
) -> list[int]:
    """The slots of every dense node: one per distinct spine edge it ends and one
    per free tile it belongs to."""
    slots = [0] * count
    spine_edges: set[Edge] = set()
    for tile, spine in zip(tiles, spines, strict=True):
        if spine is None:
            for node in tile:
                slots[node] += 1
        else:
            spine_edges.update(spine)
    for a, b in spine_edges:
        slots[a] += 1
        slots[b] += 1
    return slots


def _check_slots(
    nodes: Sequence[int], tiles: list[tuple[int, ...]], spines: list[Spine | None]
) -> None:
    slots = slot_counts(len(nodes), tiles, spines)
    for node, slot in zip(nodes, slots, strict=True):
        if slot > 2:
            raise StructureError("membership-above-two", [node])


# ---- sides ----


def sides(
    m: int,
    coords: list[tuple[int, int, int, int, int]],
    spines: Sequence[Spine | None],
) -> tuple[list[Edge], list[Edge]]:
    """Left and right spine edges, each ascending.

    `coords` holds the Zephyr index of each dense node and is unused in
    adjacency mode.
    """
    pairs = [s for s in spines if s is not None]
    edges = sorted({e for pair in pairs for e in pair})
    if m != 0:
        right = {e: coords[e[0]][0] != 0 for e in edges}
        if any(right[e] == right[f] for e, f in pairs):
            raise StructureError("not-bipartite")
    else:
        right = _two_colour(edges, pairs)
    return (
        [e for e in edges if not right[e]],
        [e for e in edges if right[e]],
    )


def _two_colour(edges: list[Edge], pairs: list[Spine]) -> dict[Edge, bool]:
    graph: dict[Edge, set[Edge]] = {e: set() for e in edges}
    for e, f in pairs:
        graph[e].add(f)
        graph[f].add(e)
    colour: dict[Edge, bool] = {}
    for root in edges:
        if root in colour:
            continue
        colour[root] = False
        queue = [root]
        head = 0
        while head < len(queue):
            node = queue[head]
            head += 1
            for nxt in sorted(graph[node]):
                if nxt not in colour:
                    colour[nxt] = not colour[node]
                    queue.append(nxt)
                elif colour[nxt] == colour[node]:
                    raise StructureError("not-bipartite")
    return colour


# ---- select ----


def select(
    tiles: list[tuple[int, ...]],
    spines: Sequence[Spine | None],
    left: Sequence[Edge],
    right: Sequence[Edge],
    left_order: Sequence[int],
    right_order: Sequence[int],
) -> list[tuple[int, ...]]:
    """Free tiles plus a maximum matching of spine tiles, by recursive augmentation."""
    rank = {r: position for position, r in enumerate(right_order)}
    left_at = {e: i for i, e in enumerate(left)}
    right_at = {e: i for i, e in enumerate(right)}
    tile_of: dict[tuple[int, int], tuple[int, ...]] = {}
    free = []
    for tile, spine in zip(tiles, spines, strict=True):
        if spine is None:
            free.append(tile)
            continue
        e, f = spine
        left_index, r = (
            (left_at[e], right_at[f]) if e in left_at else (left_at[f], right_at[e])
        )
        tile_of[(left_index, r)] = tile
    adj: list[list[int]] = [[] for _ in left]
    for left_index, r in tile_of:
        adj[left_index].append(r)
    for row in adj:
        row.sort(key=lambda r: rank[r])
    match: list[int | None] = [None] * len(right)

    def attempt(left_index: int, visited: list[bool]) -> bool:
        for r in adj[left_index]:
            if not visited[r]:
                visited[r] = True
                owner = match[r]
                if owner is None or attempt(owner, visited):
                    match[r] = left_index
                    return True
        return False

    # `attempt` recurses once per left vertex on an augmenting path, so the
    # depth is bounded only by the left side. Raise the limit for the matching
    # and restore it afterwards.
    saved = sys.getrecursionlimit()
    sys.setrecursionlimit(max(saved, len(left) + 1000))
    try:
        for left_index in left_order:
            attempt(left_index, [False] * len(right))
    finally:
        sys.setrecursionlimit(saved)
    chosen = free + [
        tile_of[(left_index, r)]
        for r, left_index in enumerate(match)
        if left_index is not None
    ]
    return sorted(chosen)


def seeded_orders(
    seed: bytes, left_len: int, right_len: int
) -> tuple[list[int], list[int]]:
    """Shuffle the identity of the left side, then of the right, from one stream."""
    rng = ChaCha8(seed)
    left_order = list(range(left_len))
    shuffle(rng, left_order)
    right_order = list(range(right_len))
    shuffle(rng, right_order)
    return left_order, right_order
