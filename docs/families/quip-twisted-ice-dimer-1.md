# quip/twisted-ice-dimer revision 1

This contract describes a twisted ice with local tile gauges, a weak signed layer, and disjoint ferromagnetic pairs. The construction picks edge-disjoint ice tiles in the base topology. Every tile has its own gauge, so a node in two tiles has two gauges. The gauge of its own tile twists the strong coupling of a tile edge, and every other edge has a weak random coupling. A chosen number of disjoint non-tile edges then become ferromagnetic pairs.
Its Rust contract is `ising_generators::ice::TWISTED_DIMER_V1`.
Its vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/twisted_dimer.json`. The same file holds the recipe, obstruction, and admission sections.
The construction uses the tile structure of `ising_generators::ice`, as `quip/ice-glass` revision 1 does.

## ID

`quip/twisted-ice-dimer`

## Revision

`1`

## Status

`research`

The registry holds the status (FAM-09). This line must equal the registry status.

## Parameter schema

| Tag | Name | Type | Scale | Min | Max |
|---|---|---|---|---|---|
| 1 | zephyr_m | u16 | 1 | 0 | 32 |
| 2 | zephyr_t | u8 | 1 | 0 | 8 |
| 3 | weak_milli | u16 | 1 | 1 | 999 |
| 4 | pairs | u32 | 1 | 0 | 65536 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` is the lattice shape. The shape `(0, 0)` is the adjacency mode. The shape with both values greater than 0 is the Zephyr mode on the lattice `z(zephyr_m, zephyr_t)`.
`weak_milli` is the size of every weak coupling, in milli units. Any value from 1 to 999 is valid.
`pairs` is the exact number of ferromagnetic pairs. The value 0 is valid. Admission rejects a value of more than half the node count.

## Substream labels

| Label | Draws |
|---|---|
| left | The shuffle of the left side order. One `uniform(i + 1)` value for each `i` from `L - 1` down to 1, where `L` is the number of left spine edges. |
| right | The shuffle of the right side order. One `uniform(i + 1)` value for each `i` from `R - 1` down to 1, where `R` is the number of right spine edges. |
| gauge | Four `uniform(2)` values for each selected tile, in tile order. The four values of a tile go to its four nodes in ascending order. |
| weak | One `uniform(2)` value for each edge that is not a tile edge, in ascending edge order. |
| dimer | The shuffle of the non-tile edge list. One `uniform(i + 1)` value for each `i` from `K - 1` down to 1, where `K` is the number of non-tile edges. |

## Construction steps

Let `n` be the node count of the base topology. Let `w` be `weak_milli` and let `P` be `pairs`.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A dense position is the index of a node in the registration order of the base topology.

1. Find the tile candidates of the base topology and its sides. The shape `(zephyr_m, zephyr_t)` selects the mode. A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.
2. Set `left_order` to the identity order of the left spine edges. Shuffle it with the Fisher-Yates shuffle (FAM-24) on the stream `left`.
3. Set `right_order` to the identity order of the right spine edges. Shuffle it the same way on the stream `right`. The shuffle of the left order draws first. The two streams are independent.
4. Select the tiles as `quip/ice-glass` revision 1 does. The result is the sorted list of tiles. Each tile is four dense positions in ascending order. Tile edges are the 6 edges of each selected tile.
5. Draw the local gauges on the stream `gauge`. For each tile in ascending order, and for each of its 4 nodes in ascending order, draw `d = uniform(gauge, 2)`. The value 0 gives the gauge `+1` and the value 1 gives `-1`. The gauge belongs to the pair of the tile and the node.
6. List the nodes in registration order. List every edge of the base topology as `(min, max)` of its dense positions, in ascending order.
7. Give each tile edge `(a, b)` of the tile `k` the coupling `1000 * g_k(a) * g_k(b)`. Here `g_k` is the gauge of the tile `k`. Each tile edge uses only the gauges of its own tile.
8. Visit every other edge in ascending order. Draw `d = uniform(weak, 2)`. If `d = 0`, set the coupling to `+w`. If `d = 1`, set the coupling to `-w`.
9. List the edges that are not tile edges, in ascending order. Shuffle that list with the Fisher-Yates shuffle on the stream `dimer`. Run the shuffle also when `P` is 0. Walk the shuffled list. Take an edge when neither of its nodes is in a pair already, and set its coupling to `-1000`. Stop after the walk takes `P` edges. If the walk ends with fewer than `P` edges, the construction returns code 14, `NoInstance`, and the miner skips the salt.
10. Set `h = 0` for every node and the offset to 0. This revision has no global gauge.
11. Write the metadata region with the kinds 1, 2, and 3, in that order. All integers are little-endian. The region is the section count 3 as `u16`, then the sections. Each section starts with its kind as `u16` and its payload length as `u32`. The payload follows.

A pair may use nodes of a tile but is never a tile edge.

This revision differs from the research generator in these ways:

- The streams are ChaCha8 streams from the stream seed of the family (FAM-22) and the shuffles are FAM-24 shuffles. The research generator used NumPy PCG64 generators seeded from the SHA-256 of the nonce and of `"dimer"` followed by the nonce.
- The matching of spine tiles is the Kuhn matching in the order of `left_order` and `right_order`. The research generator used SciPy matching.
- The weak layer draws `+w` or `-w` directly. The research generator drew a value of size 125 and rescaled it.
- A shortfall of pairs gives code 14. The research generator kept fewer pairs.
- Any topology that passes admission is valid, not only the fixed z(12, 4) hardware graph.

The research cohort is not regenerated by this revision.
This revision gives no nonce-for-nonce parity with the archived research instances.

### Recipes

This contract registers three parameter sets on Zephyr z(12, 4) as recipes. The Rust constants are `ising_generators::ice::RECIPES`. The vectors in the `recipes` section hold two salts for each recipe on the live topology.

| Alias | Cell | `weak_milli` | `pairs` | Params digest |
|---|---|---|---|---|
| `x-w62-p1200` | `ice-twisted-w62-dimer1200` | 62 | 1200 | `cdec3f240f27cda31af63003523952e09d7503f3fea82ee2a4e0f2577c2a78d5` |
| `x-w62-p1350` | `ice-twisted-w62-dimer1350` | 62 | 1350 | `21851aacb0d940d9166d1a2dcd4f1aef1039706fafc0b417701e6bb75523a6a8` |
| `x-w45-p1200` | `ice-twisted-w45-dimer1200` | 45 | 1200 | `1dbb6ae3383df6728658923d9ad88396a7b704d66ce1cd21c7403bb2c501178d` |

Each recipe has `zephyr_m` 12 and `zephyr_t` 4.

### Gauge obstruction diagnostic

The function `ising_generators::ice::gauge_obstructions` tests whether the local gauges admit one common node gauge.
The result is information only. It never decides acceptance, and no check in this contract uses it.
A node that two tiles share links them. The relation of the link is the product of the two gauges at that node. A breadth-first walk gives each tile a potential of `+1` or `-1`. The walk starts at each unvisited tile in ascending order. It visits the neighbours of a tile in ascending order of tile index and then of relation. A link counts as frustrated when the product of the potentials of its two tiles differs from its relation.
The result holds the number of components, the cycle rank (links minus tiles plus components), the number of frustrated links, and a flag that is true when every link agrees with the potentials.
The function expects the tiles of a valid instance. It does not check for a repeated position inside a tile or for duplicate tiles. A mismatched length, a position outside the node range, or a gauge that is not `1` or `-1` gives an error. A node in more than two tiles gives the error with the lowest such position.

### Tile structure

The tile structure is the one of `quip/ice-glass` revision 1.

- Tile: a 4-clique of the base topology, with 6 edges. Candidate tiles may share edges.
- Spine edge: an edge that two or more candidate tiles share.
- Spine of a tile: a perfect matching of the tile that holds every spine edge of the tile.
- Free tile: a tile with no spine edge.
- Tile slots: each node has at most 2 slots.
- Sides: the two vertex groups of the spine graph. The graph must be bipartite.

The Zephyr mode needs a base topology that is a subgraph of the lattice `z(zephyr_m, zephyr_t)`. The adjacency mode reads the adjacency alone and its tiles are the 4-cliques.

## Output order

`default`

This is the default order (FAM-31). The nodes are in registration order of the base topology.

## Offset

`0`

The offset is in milli units.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 1 | tiles | The tile count as `u32`. Then, for each tile, 4 dense positions as `u32`, ascending, with the tiles in ascending order. |
| 2 | gauges | The tile count as `u32`. Then 4 bytes for each tile, in tile order and node order, `0x00` for the gauge `+1` and `0x01` for `-1`. |
| 3 | pairs | The pair count as `u32`. Then, for each pair, the two dense positions `a` and `b` as `u32` with `a < b`, with the pairs in ascending order. |

The region has three sections, kind 1, kind 2, then kind 3, and nothing follows them.
The metadata check `check_metadata` is true only when every one of these rules holds:

- The region has exactly the kinds 1, 2, and 3 in that order, and each payload is well formed. The gauge count equals the tile count. The pair count equals `pairs`.
- The tiles are ascending and distinct. Each position is below the node count. Each tile pair is an edge of the instance. The tiles are edge-disjoint, and each node is in at most 2 tiles.
- The pairs are ascending and distinct. Each pair has `a < b` below the node count. Each pair is an edge of the instance and never a tile edge. The pairs are node-disjoint.
- Each tile edge has `J = 1000 * g_k(a) * g_k(b)` for the gauges of its own tile. Each pair has `J = -1000`. Every other edge has `J = +w` or `-w`.
- Every `h` and the offset are 0.

A count that does not fit its payload makes the check false. The check allocates nothing sized by a count before it compares the count with the payload length.

## Constraint class

`soft`

This family has no feasibility constraints. Every spin assignment is a valid answer.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 1000 milli |
| each `\|J\|` | 1000 milli |
| nodes | 131072 |
| edges | 2097152 |
| metadata bytes | 8388608 |
| max degree of the base topology | 32 |

The limits allow an `h` of up to 1000 milli. The metadata check requires every `h` to be 0, so no valid instance has a nonzero `h`.
The strong coupling and the pair coupling have a size of 1000 and every weak coupling has a size of at most 999, so no product overflows `i32`.
A base topology with a degree of more than 32 returns code 13 at size bounds. This keeps the tile search bounded (FAM-38).
Every other bound is the FAM-37 bound.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Two such values are `weak_milli` 1000 and `pairs` 65537.
Topology admission then applies these checks in this order. The first failure decides the code.

1. A self-loop gives code 12.
2. A mixed shape, which has exactly one of `zephyr_m` and `zephyr_t` equal to 0, gives code 12.
3. A degree of more than 32 does not fail here. No other tile check runs after it. Admission continues with item 11, and the size bounds give code 13 afterward.
4. A repeated unordered edge gives code 12 (`DuplicateEdge`).
5. In the Zephyr mode, a node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12 (`NotZephyrNode`).
6. In the Zephyr mode, an edge that is not a coupler of the lattice gives code 12 (`NotZephyrCoupler`).
7. A topology with no 4-clique gives code 12 (`NoCandidates`).
8. In the adjacency mode, a tile whose shared edges are not inside one perfect matching gives code 12 (`SpineConflict`).
9. A node in more than 2 tile slots gives code 12 (`MembershipAboveTwo`).
10. A spine graph with an odd cycle gives code 12 (`NotBipartite`).
11. A value of `pairs` over `floor(n / 2)` gives code 12. Admission accepts the value `floor(n / 2)`.

After these checks, the size bounds give code 13 for a degree of more than 32 and for any other bound that the topology breaks.
An admitted request with `pairs` equal to `floor(n / 2)` can still give code 14 for a salt. Code 14 is a generation result and not an admission result.

### Response to a changed topology

The generator takes the registered topology as its input. That input is the node ids and the edges of the base topology in registration order, the same input that the legacy draw reads. The construction finds the tiles and the pair matching on that topology, and the remaining edges get the weak couplings. No step assumes a fixed chip. When a chip loses qubits or couplers, refresh the registered topology as [Topology file format](../topology-format.md#refresh-after-a-chip-change) describes. The construction then builds from the new topology without a change to this contract.

- A tile needs its four nodes and its six edges on the topology. A tile with a lost node or a lost edge is not a tile. The tile count scales with the topology: the free tiles plus the maximum matching over the candidate tiles that exist.
- A pair needs both nodes and their edge on the topology. The pair count stays at `pairs` and does not scale with the topology, because the pair count sets the difficulty of a cell. Admission accepts `pairs` up to `floor(n / 2)`. A salt whose walk finds fewer than `pairs` disjoint non-tile edges gives code 14, and the miner skips the salt.
- The weak-coupler count scales with the topology: the edge count minus 6 times the tile count minus `pairs`.

An instance records the topology order digest of its base topology (FAM-19), and a lease carries the same digest (FAM-49). The solver refuses a lease whose digest differs from the digest of its cached topology (FAM-50). A solver with a native working graph also refuses a cached topology that names a node or an edge the graph lacks (FAM-47, step 4). A solver that reports no native working graph skips that check and may clamp a missing part itself.

## Size bounds

The size bounds are the base topology itself:

```text
N(p, T) = n_nodes(T)
E(p, T) = n_edges(T)
D(p, T) = max_degree(T)
```

The parameters do not change the sizes. If `max_degree(T)` is more than 32, the size bounds return code 13.

## Max retries

`none`

No step retries. The pair walk is a single bounded try, and its exhaustion is the retry exhaustion of FAM-26. A salt whose greedy pair walk ends with fewer than `pairs` pairs gives code 14, and the miner skips it.
Every other salt gives an instance when the topology passes admission.
A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
