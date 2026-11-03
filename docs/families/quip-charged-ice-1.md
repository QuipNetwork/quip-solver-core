# quip/charged-ice revision 1

This contract describes a synthetic charged ice. The construction picks edge-disjoint ice tiles in the base topology. Each tile has its own gauge and a prescribed synthetic charge. The instance holds the squared charge penalty of each tile in its couplings, fields, and offset. Every other edge has a weak random coupling of fixed size.
Its Rust contract is `ising_generators::ice::CHARGED_ICE_V1`.
Its vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/ice_charge.json`. The same file holds the energy, recipe, and admission sections.
The construction uses the tile structure of `ising_generators::ice`, as `quip/ice-glass` revision 1 does.

The tile charges are synthetic. They do not encode physical vertex charges or link incidence. This family is not an adapted family and it does not belong to the P or D-static constraint families. The charges live in metadata kind 6 (`tile_charges`). Kind 4 (`charges`) stays reserved for physical vertex charges.

## ID

`quip/charged-ice`

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
| 3 | scale_milli | u16 | 1 | 1 | 250 |
| 4 | background_milli | u16 | 1 | 1 | 999 |
| 5 | charged_per_256 | u16 | 1 | 0 | 256 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` is the lattice shape. The shape `(0, 0)` is the adjacency mode. The shape with both values greater than 0 is the Zephyr mode on the lattice `z(zephyr_m, zephyr_t)`.
`scale_milli` is the penalty scale `A`, in milli units. Any value from 1 to 250 is valid. The limit of 250 keeps every `|h|` at or below 1000 milli.
`background_milli` is the size of every background coupling, in milli units. Any value from 1 to 999 is valid.
`charged_per_256` is the chance, as a count out of 256, that a tile carries a charge. The value 0 gives no charged tile and the value 256 gives only charged tiles. The charge values are `0`, `+2`, and `-2`. A charged tile gets `+2` or `-2` with equal chance.

## Substream labels

| Label | Draws |
|---|---|
| left | The shuffle of the left side order. One `uniform(i + 1)` value for each `i` from `L - 1` down to 1, where `L` is the number of left spine edges. |
| right | The shuffle of the right side order. One `uniform(i + 1)` value for each `i` from `R - 1` down to 1, where `R` is the number of right spine edges. |
| charge | Two values for each selected tile, in tile order. First `uniform(256)`, then `uniform(2)`. Both are always drawn. |
| gauge | Four `uniform(2)` values for each selected tile, in tile order. The four values of a tile go to its four nodes in ascending order. |
| background | One `uniform(2)` value for each edge that is not a tile edge, in ascending edge order. |

## Construction steps

Let `n` be the node count of the base topology. Let `A` be `scale_milli`, let `b` be `background_milli`, and let `c` be `charged_per_256`.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A dense position is the index of a node in the registration order of the base topology.

1. Find the tile candidates of the base topology and its sides. The shape `(zephyr_m, zephyr_t)` selects the mode. A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.
2. Set `left_order` to the identity order of the left spine edges. Shuffle it with the Fisher-Yates shuffle (FAM-24) on the stream `left`.
3. Set `right_order` to the identity order of the right spine edges. Shuffle it the same way on the stream `right`. The shuffle of the left order draws first. The two streams are independent.
4. Select the tiles as `quip/ice-glass` revision 1 does. The result is the sorted list of tiles. Each tile is four dense positions in ascending order. Tile edges are the 6 edges of each selected tile.
5. Draw the charges on the stream `charge`. For each tile `t` in ascending order, draw `r = uniform(charge, 256)` and then `s = uniform(charge, 2)`. Always make both draws. Set `q_t = 0` if `r >= c`. Otherwise set `q_t = +2` if `s = 0` and `q_t = -2` if `s = 1`.
6. Draw the local gauges on the stream `gauge`. For each tile in ascending order, and for each of its 4 nodes in ascending order, draw `d = uniform(gauge, 2)`. The value 0 gives the gauge `+1` and the value 1 gives `-1`. The gauge belongs to the pair of the tile and the node. This is the draw of `quip/twisted-ice-dimer` revision 1.
7. List the nodes in registration order. List every edge of the base topology as `(min, max)` of its dense positions, in ascending order.
8. Give each tile edge `(a, b)` of the tile `t` the coupling `A * g_t(a) * g_t(b)`. Here `g_t` is the gauge of the tile `t`. Each tile edge uses only the gauges of its own tile.
9. Visit every other edge in ascending order. Draw `d = uniform(background, 2)`. If `d = 0`, set the coupling to `+b`. If `d = 1`, set the coupling to `-b`.
10. For each node `i`, set `h[i]` to the sum, over the tiles `t` that contain `i`, of `-A * q_t * g_t(i)`. The field of a node in no tile is 0, and the field of a node in two tiles sums both terms.
11. Set the offset to the sum, over all tiles, of `A * (4 + q_t^2) / 2`. The value is an integer because every `q_t` is even.
12. Write the metadata region with the kinds 1, 2, and 6, in that order. All integers are little-endian. The region is the section count 3 as `u16`, then the sections. Each section starts with its kind as `u16` and its payload length as `u32`. The payload follows.

### Penalty expansion

The penalty of the tile `t` with the spins `s_i` on its four nodes is:

```text
P_t = (A / 2) * (sum_i g_i s_i - q_t)^2
```

The sum runs over the four nodes of the tile. The steps below use `s_i^2 = 1` and `g_i^2 = 1`.

1. Square the bracket: `(sum_i g_i s_i)^2 - 2 q_t sum_i g_i s_i + q_t^2`.
2. Expand the square of the sum: `sum_i (g_i s_i)^2 + 2 sum_{i<j} g_i g_j s_i s_j`. The first part is 4.
3. Multiply by `A / 2`. The result is `A sum_{i<j} g_i g_j s_i s_j - A q_t sum_i g_i s_i + A (4 + q_t^2) / 2`.

The first term gives the coupling `A g_i g_j` of each tile edge. The second term gives the field `-A q_t g_i` on each node of the tile. The third term is a constant. The instance offset is the sum of the constants of all tiles.
The encoded energy of a spin state, offset included, equals the sum of the tile penalties plus the sum of `J s s` over the background edges. The vectors check this equality for four spin states of every salt. The research generator dropped the constant, so its energies are lower than the energies of this revision by exactly the offset.

### Edge rule

The instance keeps every edge of the base topology. The tiles are edge-disjoint and `background_milli` is at least 1, so no coupling is 0. The `J != 0` filter of the research generator removes nothing.

A background edge is never a tile edge. It may join nodes of two different tiles.

This revision differs from the research generator in these ways:

- The streams are ChaCha8 streams from the stream seed of the family (FAM-22) and the shuffles are FAM-24 shuffles. The research generator used one NumPy PCG64 generator seeded from the SHA-256 of the cell name and the nonce.
- The tiles come from the tile selection on the topology. The research generator took them from saved `ice-twisted` suite models.
- The offset includes the penalty constant. The research energies are lower by exactly the offset.
- The charge chance is a parameter. The research generator fixed the shares at one half for no charge, one quarter for `+2`, and one quarter for `-2`.
- Any topology that passes admission is valid, not only the fixed z(12, 4) hardware graph.

The research cohort is not regenerated by this revision.
This revision gives no nonce-for-nonce parity with the archived research instances.

### Recipes

This contract registers one parameter set on Zephyr z(12, 4) as a recipe. The Rust constant is `ising_generators::ice::ICE_CHARGE_RECIPE`. The vectors in the `recipe` section hold two salts on the live topology. Each salt records the hash of the canonical instance, the tile count, the charged tile count, and the largest `|h|`.

| Alias | Cell | `scale_milli` | `background_milli` | `charged_per_256` | Params digest |
|---|---|---|---|---|---|
| `ice-charge` | `ice-charge` | 250 | 31 | 128 | `45b3c0d97e888bc2e75deb0b55c51e3dcb0764efe9ce10944d3cf90ea950d2ef` |

The recipe has `zephyr_m` 12 and `zephyr_t` 4.

### Charge defect diagnostic

The function `ising_generators::ice::charge_defects` counts the tiles whose charge `sum_i g_i s_i` differs from `q_t` for a spin state of the instance.
The result is information only. It never decides acceptance, and no check in this contract uses it.
The function gives no value for a malformed instance, for a state of the wrong length, or for a state with a value other than `+1` or `-1`.
The vectors record the count for each of the four spin states of every salt.

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

The offset is the sum, over all tiles, of `A * (4 + q_t^2) / 2`, in milli units. With no tile, the offset is 0. A tile with `q_t = 0` adds `2 A` and a charged tile adds `4 A`.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 1 | tiles | The tile count as `u32`. Then, for each tile, 4 dense positions as `u32`, ascending, with the tiles in ascending order. |
| 2 | gauges | The tile count as `u32`. Then 4 bytes for each tile, in tile order and node order, `0x00` for the gauge `+1` and `0x01` for `-1`. |
| 6 | tile_charges | The tile count as `u32`. Then one `i8` byte for each tile, in tile order: `0x00` for `0`, `0x02` for `+2`, and `0xFE` for `-2`. |

The region has three sections, kind 1, kind 2, then kind 6, and nothing follows them.
The metadata check `check_metadata` is true only when every one of these rules holds:

- The region has exactly the kinds 1, 2, and 6 in that order, and each payload is well formed. The gauge count and the charge count equal the tile count. Every charge is `0`, `+2`, or `-2`.
- The tiles are ascending and distinct. Each position is below the node count. Each tile pair is an edge of the instance. The tiles are edge-disjoint, and each node is in at most 2 tiles.
- Each tile edge has `J = A * g_t(a) * g_t(b)` for the gauges of its own tile. Every other edge has `J = +b` or `-b`.
- Every `h` equals the formula of step 10, and the offset equals the formula of step 11.

A count that does not fit its payload makes the check false. The check allocates nothing sized by a count before it compares the count with the payload length.
The values `A` and `b` in these rules come from the parameters. A nonzero `h` where the formula gives 0, or a wrong offset, makes the check false.

## Constraint class

`soft`

This family has no feasibility constraints. Every spin assignment is a valid answer.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 1000 milli |
| each `\|J\|` | 250 milli for a tile edge and 999 milli for a background edge |
| the offset | 1000 milli for each tile |
| nodes | 131072 |
| edges | 2097152 |
| metadata bytes | 8388608 |
| max degree of the base topology | 32 |

A node is in at most 2 tiles. Each tile adds at most `2 * A` to `|h|` of its node, so `|h|` is at most `4 * A`, which is 1000 at `A = 250`.
The instance limits are 1000 for each coefficient. The largest coupling has a size of 999, so no product overflows `i32`.
A base topology with a degree of more than 32 returns code 13 at size bounds. This keeps the tile search bounded (FAM-38).
Every other bound is the FAM-37 bound.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Two such values are `scale_milli` 251 and `charged_per_256` 257.
Topology admission then applies these checks in this order. The first failure decides the code.

1. A self-loop gives code 12.
2. A mixed shape, which has exactly one of `zephyr_m` and `zephyr_t` equal to 0, gives code 12.
3. A degree of more than 32 does not fail here. No other tile check runs after it. Admission continues with the size bounds, and they give code 13 afterward.
4. A repeated unordered edge gives code 12 (`DuplicateEdge`).
5. In the Zephyr mode, a node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12 (`NotZephyrNode`).
6. In the Zephyr mode, an edge that is not a coupler of the lattice gives code 12 (`NotZephyrCoupler`).
7. A topology with no 4-clique gives code 12 (`NoCandidates`).
8. In the adjacency mode, a tile whose shared edges are not inside one perfect matching gives code 12 (`SpineConflict`).
9. A node in more than 2 tile slots gives code 12 (`MembershipAboveTwo`).
10. A spine graph with an odd cycle gives code 12 (`NotBipartite`).

After these checks, the size bounds give code 13 for a degree of more than 32 and for any other bound that the topology breaks.
Every admitted salt gives an instance. This family has no code 14.

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

No step retries. Every salt gives an instance when the topology passes admission.
A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
