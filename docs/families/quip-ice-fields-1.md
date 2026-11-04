# quip/ice-fields revision 1

This contract describes the globally gauged ice glass of `quip/ice-glass` revision 1 with a sparse random field. The construction picks edge-disjoint ice tiles in the base topology. It gives each tile edge the strong coupling and each other edge a random glass coupling. A random gauge then hides the tiles. A field of fixed size is then set on a fixed share of the nodes that sit in exactly two selected tiles.
Its Rust contract is `ising_generators::ice::ICE_FIELDS_V1`.
Its vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/ice_fields.json`. The same file holds the selection, field recipe, and admission sections.
The construction uses the tile structure of `ising_generators::ice`, as `quip/ice-glass` revision 1 does.

The base is the globally gauged glass of `quip/ice-glass` revision 1, not the twisted ice of `quip/twisted-ice-dimer`. Every tile edge keeps the product `J * g_u * g_v = +1000` for one global gauge `g`. The field is the only difference from the glass.
This family is not an adapted family and it does not belong to the P or D-static constraint families.

## ID

`quip/ice-fields`

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
| 3 | glass_milli | u16 | 1 | 1 | 999 |
| 4 | field_milli | u16 | 1 | 1 | 1000 |
| 5 | field_divisor | u16 | 1 | 1 | 65535 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` is the lattice shape. The shape `(0, 0)` is the adjacency mode. The shape with both values greater than 0 is the Zephyr mode on the lattice `z(zephyr_m, zephyr_t)`.
`glass_milli` is the size of every glass coupling, in milli units. Any value from 1 to 999 is valid.
`field_milli` is the size of every field, in milli units. Any value from 1 to 1000 is valid.
`field_divisor` sets the number of field nodes. The count is `n / field_divisor` with integer division, where `n` is the node count. A divisor greater than `n` gives the count 0 and an instance with no field. That is valid.

## Substream labels

| Label | Draws |
|---|---|
| left | The shuffle of the left side order. One `uniform(i + 1)` value for each `i` from `L - 1` down to 1, where `L` is the number of left spine edges. |
| right | The shuffle of the right side order. One `uniform(i + 1)` value for each `i` from `R - 1` down to 1, where `R` is the number of right spine edges. |
| glass | One `uniform(2)` value for each edge that is not a tile edge, in ascending edge order. |
| gauge | One `uniform(2)` value for each node, in node registration order. |
| field | The shuffle of the eligible node list. One `uniform(i + 1)` value for each `i` from `K - 1` down to 1, where `K` is the number of eligible nodes. |
| field_sign | One `uniform(2)` value for each field node, in ascending node order. |

The streams `field` and `field_sign` are independent of the other four. With the same descriptor digest and the same nonce, a change of `field_milli` or `field_divisor` leaves every coupling and the gauge unchanged. The descriptor digest binds every parameter, so a change of any parameter through the public entry point gives new streams.

## Construction steps

Let `n` be the node count of the base topology. Let `g` be `glass_milli`, let `f` be `field_milli`, and let `d` be `field_divisor`.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A dense position is the index of a node in the registration order of the base topology.

1. Find the tile candidates of the base topology and its sides. The shape `(zephyr_m, zephyr_t)` selects the mode. A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.
2. Set `left_order` to the identity order of the left spine edges. Shuffle it with the Fisher-Yates shuffle (FAM-24) on the stream `left`.
3. Set `right_order` to the identity order of the right spine edges. Shuffle it the same way on the stream `right`. The shuffle of the left order draws first. The two streams are independent.
4. Select the tiles as `quip/ice-glass` revision 1 does. The result is the sorted list of tiles. Each tile is four dense positions in ascending order. Tile edges are the 6 edges of each selected tile.
5. List the nodes in registration order. List every edge of the base topology as `(min, max)` of its dense positions, in ascending order.
6. Give each tile edge the coupling `+1000`. Visit every other edge in ascending order. Draw `d = uniform(glass, 2)`. If `d = 0`, set the coupling to `+g`. If `d = 1`, set the coupling to `-g`.
7. For each node in registration order, draw `uniform(gauge, 2)`. The value 0 gives the gauge `+1` and the value 1 gives `-1`. Then multiply each coupling `J(u, v)` by the gauge of `u` and the gauge of `v`.
8. Count the selected tiles that contain each node. A node is eligible when it is in exactly two selected tiles. List the eligible nodes as dense positions in ascending order. Set the field count `c` to `n / d`, with integer division.
9. If the number of eligible nodes is below `c`, the salt gives no instance. The construction returns code 14, `NoInstance`, and the miner skips the salt (FAM-26). Nothing draws on the stream `field` before this check.
10. Shuffle the eligible list with the Fisher-Yates shuffle (FAM-24) on the stream `field`. Keep the first `c` entries and sort them in ascending order. These are the field nodes.
11. For each field node in ascending order, draw `s = uniform(field_sign, 2)`. The value 0 gives the sign `+1` and the value 1 gives `-1`. Set `h[i]` to `sign * f * gauge[i]`, where `gauge[i]` is the gauge of the node from step 7. Set `h = 0` for every other node.
12. Set the offset to 0.
13. Write the metadata region with the kinds 1 and 2, in that order. All integers are little-endian. The region is the section count 2 as `u16`, then the sections. Each section starts with its kind as `u16` and its payload length as `u32`. The payload follows.

Every instance has exactly `c` field nodes. Each field node is in exactly two selected tiles, and each field has the size `f`.

### The field rule

The field is the drawn sign times the gauge. The metadata gives the tiles and the gauge, but the drawn sign stays hidden, so a field of either sign fits the same metadata. The metadata check counts the field nodes and the tile membership of each. It also checks the size of each field. The sign of a field is outside the check. This is a stated limit.

### The shortfall

A selection of tiles can leave fewer eligible nodes than `c`. Competing candidate tiles share a spine edge, so only one of them is kept. The node with two slots from those spine edges then has one selected tile.

The nonce decides the shortfall. Its salt gets code 14 and the miner skips it. Admission has no say here. Admission rejects only a field count that no selection can reach, as the topology admission rules below state.

This revision differs from the research generator in these ways:

- The streams are ChaCha8 streams from the stream seed of the family (FAM-22) and the shuffles are FAM-24 shuffles. The research generator used one NumPy PCG64 generator seeded from the SHA-256 of the nonce, and a separate field nonce from the SHA-256 of the nonce and `ice-fields-v1`.
- The field sign is drawn only for the selected nodes. The research generator drew a value for every node and kept the selected ones.
- A shortfall of eligible nodes gives code 14. The research generator raised an error.
- Any topology that passes admission is valid. The research generator required the live 4575 node topology.

The research cohort is not regenerated by this revision.
This revision gives no nonce-for-nonce parity with the archived research instances.

### Recipes

This contract registers two parameter sets on Zephyr z(12, 4) as recipes. The Rust constant is `ising_generators::ice::ICE_FIELDS_RECIPES`. The vectors in the `field_recipes` section hold two salts of each recipe on the live topology. Each salt records the hash of the canonical instance, the tile count, the field count, and the numbers of positive and negative fields.

| Alias | Cell | `glass_milli` | `field_milli` | `field_divisor` | Params digest |
|---|---|---|---|---|---|
| `h500` | `ice-fields-h500` | 125 | 500 | 16 | `9c3b08894d2efa4170e90c2cc60b548366adb715e7c1b7ba581539631a9c728f` |
| `h1000` | `ice-fields-h1000` | 125 | 1000 | 16 | `8df306c9b33e82fd184d366d287cd9022c97b14eb9b6f206e93e18267848e5d4` |

Both recipes have `zephyr_m` 12 and `zephyr_t` 4.

### Diagnostics

This family adds no diagnostic function. It has no charge and no constraint target.

### Tile structure

The tile structure is the one of `quip/ice-glass` revision 1.

- Tile: a 4-clique of the base topology, with 6 edges. Candidate tiles may share edges.
- Spine edge: an edge that two or more candidate tiles share.
- Spine of a tile: a perfect matching of the tile that holds every spine edge of the tile.
- Free tile: a tile with no spine edge.
- Tile slots: each node has at most 2 slots. A free tile fills one slot on each of its 4 nodes, while a spine tile fills one slot on each end of its 2 spine edges. Each spine edge counts once even when many tiles hold it.
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
| 2 | gauge | The node count as `u32`. Then one byte for each node in node order, `0x00` for the gauge `+1` and `0x01` for `-1`. |

The region has two sections, kind 1 then kind 2, and nothing follows them. The layout is byte for byte the layout of `quip/ice-glass` revision 1. The gauge count must equal the node count.
The metadata check `check_metadata` is true only when every one of these rules holds:

- The ice-glass checks hold. The tiles are ascending and distinct, each position is below the node count, each tile pair is an edge of the instance, the tiles are edge-disjoint, and each node is in at most 2 tiles.
- Each tile edge has `J * g_u * g_v = 1000`. Each other edge has `J * g_u * g_v = +g` or `-g`.
- Exactly `n / d` nodes have a nonzero `h`. Each of them is in exactly two tiles of the metadata, and each nonzero `h` has the size `f`.
- The offset is 0.

The check does not test the sign of a field (see the field rule).
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

Each nonzero `h` has the size `field_milli`, which is at most 1000, so the limit of 1000 holds for every field.
The shape rule: `zephyr_m` and `zephyr_t` are both 0 or both more than 0. A mixed shape gives code 12 at topology admission.
The strong coupling is 1000 and every glass coupling has a size of at most 999, so no product overflows `i32`.
A base topology with a degree of more than 32 returns code 13 at size bounds. This keeps the tile search bounded (FAM-38).
Every other bound is the FAM-37 bound.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Three such values are `field_milli` 1001, `glass_milli` 1000, and `field_divisor` 0.
Topology admission then applies these checks in this order. The first failure decides the code.

1. A self-loop gives code 12.
2. A mixed shape, which has exactly one of `zephyr_m` and `zephyr_t` equal to 0, gives code 12.
3. A degree of more than 32 does not fail here. No other topology check runs after it, including item 11. Admission continues with the size bounds, and they give code 13 afterward.
4. A repeated unordered edge gives code 12 (`DuplicateEdge`).
5. In the Zephyr mode, a node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12 (`NotZephyrNode`).
6. In the Zephyr mode, an edge that is not a coupler of the lattice gives code 12 (`NotZephyrCoupler`).
7. A topology with no 4-clique gives code 12 (`NoCandidates`).
8. In the adjacency mode, a tile whose shared edges are not inside one perfect matching gives code 12 (`SpineConflict`).
9. A node in more than 2 tile slots gives code 12 (`MembershipAboveTwo`).
10. A spine graph with an odd cycle gives code 12 (`NotBipartite`).
11. A field count `n / field_divisor` greater than the number of nodes with exactly two tile slots in the candidate structure gives code 12. A node in two selected tiles has two slots, so no selection can reach a larger count. The registry never admits a parameter set that can never give an instance on the topology.

After these checks, the size bounds give code 13 for a degree of more than 32 and for any other bound that the topology breaks.
A count within the bound can still miss the selection of a nonce. That gives code 14 for the salt, not an admission code.

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

No step retries. A salt whose selection has fewer eligible nodes than the field count gives code 14 and the miner skips it. Every other salt gives an instance when the topology passes admission.
A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
