# quip/two-commodity-routing revision 1

This contract describes a two-commodity unit routing problem on a Manhattan-street template of Zephyr cells. Each cell of the Zephyr grid is a node of a routing graph. Arcs join the neighbours of a cell, with one direction for each row and column. Each of the two commodities has a set of sources and a set of sinks of equal size, and it routes one unit from each source to some sink over the arcs. Each arc holds one unit of each commodity, and the two commodities need separate arcs. The sinks of a commodity are interchangeable, so no source has a fixed sink. A shared arc or a high cost raises the energy.
The Rust contract is `ising_generators::routing::ROUTING_V1`.
The vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/two_commodity.json`. The same file holds the terminals, routing recipe, and admission sections.

Commodity 1 uses one spin for each arc, shared by the two cells that the arc joins. Commodity 2 uses a chain of two spins for each arc, one for each end, so the two cells share no commodity 2 spin. The first chain spin crosses the commodity 1 spin of the same arc, and the capacity term sits on that coupler.
This family is not an adapted family and it does not belong to the P or D-static constraint families.

## ID

`quip/two-commodity-routing`

## Revision

`1`

## Status

`research`

The registry holds the status (FAM-09). This line must equal the registry status.

## Parameter schema

| Tag | Name | Type | Scale | Min | Max |
|---|---|---|---|---|---|
| 1 | zephyr_m | u16 | 1 | 1 | 32 |
| 2 | zephyr_t | u8 | 1 | 3 | 8 |
| 3 | pairs_divisor | u16 | 1 | 4 | 65535 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` names the lattice `z(zephyr_m, zephyr_t)`. The base topology is a subgraph of that lattice.
The parameter `pairs_divisor` gives the number of sources of each commodity, and the same number of sinks. With `N` template nodes, the number is `N / pairs_divisor` with integer division. A divisor greater than `N` gives 0 sources and 0 sinks, and an instance with no terminals. That is valid.
The weights, the costs, and the street pattern are constants of this contract and not parameters.

### Constants

| Name | Value | Use |
|---|---|---|
| `CONSERVE` | 248 | The weight of each conservation square, in milli |
| `CAPACITY` | 496 | The weight of the capacity coupler of each arc, in milli |
| `COSTS` | 62 and 124 | The two arc costs, in milli |
| `TERMINAL_TRIES` | 4 | The number of terminal draws before the salt gives code 14 |
| chain strength cap | 1000 | The largest chain strength, in milli |

## Substream labels

The labels come in this order.

| Label | Draws |
|---|---|
| terminal | One shuffle of the template node indices for each try, in order. Each shuffle draws one `uniform(i + 1)` value for each `i` from `N - 1` down to 1. The stream continues across the tries and is never opened again. |
| cost | For each arc in template order, one `uniform(2)` value for the cost of commodity 1, then one for commodity 2. |

## Construction steps

Let `m` be `zephyr_m`, let `t` be `zephyr_t`, and let `L` be `2m - 1`. A cell is a pair `(c, r)` with `1 <= c, r <= L`. Cells sort by `(c, r)`, so by `c` first.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.

### The template

The template depends on the base topology and the parameters. It does not depend on the nonce.

**Segments.** The function `seg(u, w, k, p)` is the native id of the Zephyr coordinates `(u, w, k, j, z)` with `j = p mod 2` and `z = p / 2` (integer division). It exists when `0 <= p < 2m` and that id is a node of the base topology. Otherwise the result is none. The linear id of a coordinate follows the Zephyr rule of `ising_generators::ice`.

**Cell K4.** The function `cell_k4(c, r, k, k2)` is the four segments `(seg(0, c, k, r - 1), seg(0, c, k, r), seg(1, r, k2, c - 1), seg(1, r, k2, c))`. Call them VLO, VHI, HL, and HR. It exists when all four segments exist and all six pairs of them are edges of the base topology. Otherwise the result is none.

1. **Layout.** `rows[r]` and `cols[c]` are triples of distinct lines in `0..t`. Every triple starts at `(0, 1, 2)`. A cell `(c, r)` is alive for a row triple `R` and a column triple `C` when `cell_k4(c, r, C[0], R[0])` and `cell_k4(c, r, C[1 + r mod 2], R[1 + c mod 2])` both exist. Repeat the following until a sweep changes nothing.
   - For each `r` in ascending order, score every ordered triple of distinct lines in `0..t` as the number of alive cells of row `r` with the current `cols`. Try the triples in lexicographic order. If the best score beats the score of `rows[r]`, set `rows[r]` to the first triple in lexicographic order with the best score.
   - Then do the same for each `c` in ascending order and `cols[c]`, with the updated `rows`.
2. **Corners.** `corners1(c, r)` is the four segments of `cell_k4` on the lines `cols[c][0]` and `rows[r][0]`. `corners2(c, r)` is the four segments on the lines `cols[c][1 + r mod 2]` and `rows[r][1 + c mod 2]`. Neither one checks the clique, and any entry can be none.
3. **Arcs.** For `c` in ascending order, then `r` in ascending order, there are two arc candidates in this order.
   - Horizontal, when `c + 1 <= L`. The commodity 1 spin `x1` is `corners1(c, r).HR`. The chain is `(corners2(c, r).VHI, corners2(c + 1, r).HL)`. The tail is `(c, r)` and the head is `(c + 1, r)` when `r` is even. Otherwise swap them.
   - Vertical, when `r + 1 <= L`. The spin `x1` is `corners1(c, r).VHI`. The chain is `(corners2(c, r).HR, corners2(c, r + 1).VLO)`. The tail is `(c, r)` and the head is `(c, r + 1)` when `c` is even. Otherwise swap them.

   A candidate becomes an arc when `x1` and both chain spins exist, the two chain spins share an edge, and `x1` shares an edge with the first chain spin.
4. **Pruning.** Repeat until a pass drops nothing. Build the incident list of each cell. Go through the arcs in list order and append the index of each arc to the list of its tail and then to the list of its head. Next, take the cells in cell order. At each cell, `k` is 0 and then 1, and the pairs are `i < j` in list order of the arcs of the cell that are not yet dropped. Drop the arc `max(i, j)` when `spin(i, v, k)` and `spin(j, v, k)` share no edge. Here `spin(a, v, 0)` is `x1` of the arc `a`. `spin(a, v, 1)` is the first chain spin when `v` is the smaller cell of the arc, and the second chain spin otherwise. Then remove the dropped arcs and keep the order of the others.
5. **Core.** Take the strongly connected components of the directed graph whose nodes are the arc ends. Keep the component with the most nodes. Among components with the same node count, keep the one whose smallest cell is smallest in cell order. Keep the arcs with both ends in it, in list order. The template nodes are the cells of the component in cell order, and a node index is a position in that list.

### The salt

Let `N` be the number of template nodes and `pairs` be `N / pairs_divisor`.

6. **Terminals.** For try 1 to 4, shuffle the list `0..N` with the Fisher-Yates shuffle (FAM-24) on the stream `terminal`. Commodity 1 gets the supply `+1` on the entries `2i` and `-1` on the entries `2i + 1` of the shuffled list, for `i < pairs`. This only fixes which nodes are sources and which are sinks. No source has a fixed sink. Commodity 2 does the same on the entries that start at `2 * pairs`. When both commodities pass the feasibility check below, stop and keep this try. After 4 failed tries the salt gives no instance. The construction returns code 14, `NoInstance`, and the miner skips the salt (FAM-26). With `pairs` equal to 0, try 1 passes. Each try still shuffles.
7. **Costs.** For each arc in template order, draw `uniform(cost, 2)` for commodity 1 and then for commodity 2. The value 0 gives 62 and the value 1 gives 124.
8. **Spins.** The instance nodes are every `x1` spin and every chain spin of the template arcs, in the registration order of the base topology (FAM-31). No spin repeats.
9. **Energy.** The energy has the form `x = (1 - s) / 2` in quarter-milli integers `h4`, `j4`, and `c4`, with these operations.
   - `lin(i, c)` adds `c * x_i`: `h4[i] -= 2c` and `c4 += 2c`.
   - `pair(i, j, c)` adds `c * x_i * x_j`: `j4[i, j] += c`, `h4[i] -= c`, `h4[j] -= c`, and `c4 += c`.
   - `square(w, coeffs, b0)` adds `w * (sum of c_i * x_i + b0)^2`. For each term `i`, `lin(i, w * (c_i^2 + 2 * b0 * c_i))`. For each pair of terms `i < j`, `pair(i, j, 2 * w * c_i * c_j)`. Then `c4 += 4 * w * b0^2`.
   - For each template node in order and each commodity `k` in 0 and 1, the coefficients are `+1` on `spin(a, v, k)` of the out arcs and `-1` on the same spin of the in arcs. When there is at least one coefficient, apply `square(248, coeffs, -supply[v][k])`.
   - For each arc, apply `pair(x1, chain[0], 496)`, `lin(x1, cost1)`, and `lin(chain[0], cost2)`.

   Then `h = h4 / 4`, `J = j4 / 4`, and `offset = c4 / 4`. Each division is exact, as the offset proof below states.
10. **Chain strength.** Let `S` be `1` plus the largest value, over every chain spin, of the sum of `|J|` over the edges of that spin plus `|h|` of that spin. Both are the values before the chain couplers join the energy. If `S` exceeds 1000, the salt gives code 13. Then add `-S` to the coupling of the two chain spins of each arc. The chain coupler adds nothing to the offset.
11. **Output.** The edges are the pairs with a nonzero `J`, in ascending dense positions. Every such pair is an edge of the base topology. Any other pair is a generator defect.
12. **Metadata.** Write the region with the kind 7 only, as the metadata section below states.

### Feasibility check

The check of each try runs on each commodity alone. The check is a unit-capacity max flow from a super source, joined to every source, to a super sink, joined from every sink, over the arcs in their directions. The try passes for a commodity when the flow value equals `pairs`. A source may reach any sink, so the check matches no source with a sink. The flow value has one answer for every algorithm.

Two commodities that each pass alone are not a joint feasibility certificate. A routing of commodity 1 may use an arc that every routing of commodity 2 needs. An instance can have no state with zero violations. A verifier must not read the check as proof that a zero-violation state exists.

### The chain strength bound

The chain strength never reaches the cap. A chain spin sits in the conservation square of one cell only, in the capacity coupler of its arc, and in one cost term. Its load depends only on the arcs at that cell, their directions, their costs, and the supply of commodity 2 at that cell. The test `chain_strength_never_reaches_the_cap` in `ising_generators::routing` runs every such configuration. It covers every nonempty set of the four street neighbours, both row and column parities, all costs, and each supply of –1, 0, and 1. The largest value of `S` is 931, which is below 1000. The live topology reaches 931. No topology can give a larger value, so code 13 for the chain strength cannot occur. The check stays in the construction as a guard.

### The offset proof

Every quarter-milli value divides by 4. The weights are 248 and 496, which are 4 times 62 and 4 times 124. A `lin` call adds `2c`. For a square term, `c` is 248 times an integer, so `2c` is a product of 496 and an integer. For a cost, `c` is 62 or 124, so `2c` is 124 or 248. A `pair` call adds `c`. For a square term, `c` is `2 * 248 * c_i * c_j`, which is 496 times an integer. For the capacity coupler, `c` is 496. The square constant is `4 * 248 * b0^2`. Every term added to `h4`, `j4`, and `c4` is a product of 4 and an integer, so every division is exact. A nonzero rest is a construction defect, which cannot happen with these constants.

### Intended differences from the research generator

- The streams are ChaCha8 streams from the stream seed of the family (FAM-22) and the shuffles are FAM-24 shuffles. The research generator used framed SHAKE256 digests for each template coordinate.
- The terminal retries continue one stream. The research generator drew the whole build again under a new try tag.
- The largest strongly connected component has a fixed tie rule. The research generator left a tie to the iteration order of NetworkX.
- A chain strength over 1000 gives code 13. The research generator rejected the try.
- Any topology that passes admission is valid. The research generator required the live topology.
- The census admission of the research run, which holds the component fractions and the active spins, is not ported.

The template does not depend on the nonce. On the live z(12, 4) topology, this revision gives the same nodes, arcs, arc spins, and chain spins as the research generator. The fixture `conformance/fixtures/two_commodity_research_template.json` holds the research template, and a test in each of Rust and Python compares it with the template of this revision. This revision makes no claim about the terminals, the costs, or the instance bytes of the research instances.
The research cohort is not regenerated by this revision.

### Recipes

This contract registers one parameter set on Zephyr z(12, 4) as a recipe. The Rust constant is `ising_generators::routing::ROUTING_RECIPES`. The vectors in the `routing_recipes` section hold two salts of the recipe on the live topology. Each salt records the hash of the canonical instance, the number of cells, the number of arcs, the chain strength, and the try that passed.

| Alias | Cell | `zephyr_m` | `zephyr_t` | `pairs_divisor` | Params digest |
|---|---|---|---|---|---|
| `r32` | `two-commodity-routing` | 12 | 4 | 32 | `79860ccdaab2f836f62c593697e323f297e28290cfcd41d706f3338267d480e6` |

### Diagnostics

The function `routing_defects(params, instance, spins)` returns `RoutingDefects`, or `None` when `check_metadata` fails, when `spins` has the wrong length, or when a value is not +1 or –1. It reads `x = (1 - s) / 2`. Commodity 1 uses `x[x1]` and commodity 2 uses `x[chain0]`. The result has these fields.

| Field | Meaning |
|---|---|
| `capacity_conflicts` | The number of arcs with both bits equal to 1 |
| `conservation_violations` | The number of node and commodity pairs where the out flow minus the in flow differs from the supply |
| `used_arcs` | The number of arcs with bit 1, for commodity 1, and then for commodity 2 |
| `broken_chains` | The number of arcs with `x[chain0]` not equal to `x[chain1]` |

The diagnostic is information only. It never decides acceptance (FAM-40).

## Output order

`default`

This is the default order (FAM-31). The nodes are in registration order of the base topology.

## Offset

`c4 / 4`

The offset is in milli units. The value is the exact quotient of step 9.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 7 | routing | See the layout below. |

The region has one section, kind 7, and nothing follows it. All integers are little-endian.

The payload is these parts in order.

1. `N` as `u32`, then `N` node records. Each record is `c` as `u16`, `r` as `u16`, `supply1` as `i8`, and `supply2` as `i8`.
2. `A` as `u32`, then `A` arc records. Each record is `tail` as `u32`, `head` as `u32`, `x1` as `u32`, `chain0` as `u32`, `chain1` as `u32`, `cost1` as `u16`, and `cost2` as `u16`. The values `tail` and `head` are template node indices. The values `x1`, `chain0`, and `chain1` are dense positions in the instance.
3. `S` as `u16`.

The metadata check `check_metadata` is true only when every one of these rules holds.

- The section parses exactly, `N` is at least 1, `A` is at least 1, and the cells increase with no repeat within `1..=L`.
- Each supply is –1, 0, or +1. Each commodity has `N / pairs_divisor` sources and as many sinks. No node is a terminal of both commodities.
- Each arc joins two different cells that are neighbours in the street direction of step 3, and no unordered pair of cells repeats.
- The `3A` spins are distinct and are exactly the instance nodes, so `n` equals `3A`.
- Each cost is 62 or 124, and `S` is at least 1 and at most 1000.
- The energy rebuilt from the metadata by step 9 equals the instance `h`, edges, `J`, and offset. The value `S` equals the rule of step 10 on the rebuilt energy.

A count that does not fit its payload makes the check false. The check allocates nothing sized by a count before it compares the count with the payload length.
The check does not test that a terminal draw passed the feasibility check, because the draw is not part of the metadata.

## Constraint class

`soft`

This family has no feasibility constraints. Every spin assignment is a valid answer. A state with zero violations may not exist (see the feasibility check).

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 1000 milli |
| each `\|J\|` | 1000 milli |
| `\|offset\|` | 1099511627776 milli |
| nodes | 131072 |
| edges | 2097152 |
| metadata bytes | 1048576 |
| chain strength `S` | 1000 milli |

The family has no limit on the degree of the base topology.
The limit of 1000 on `|J|` holds because the chain coupling is `-S` and the other couplings come from one conservation square and one capacity coupler. The chain strength bound gives `S` at most 931.
Every other bound is the FAM-37 bound or a stricter bound of this contract.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Two such values are `pairs_divisor` 3 and `zephyr_t` 2.
Topology admission then applies these checks. The first failure decides the code.

1. A node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12.
2. An edge that is not a coupler of the lattice gives code 12. A self-loop is such an edge.
3. A template with no arc gives code 12. The template does not depend on the nonce, so admission computes it.

After these checks, the size bounds give code 13 when an advertised limit of the solver is below the sizes. The sizes are the sizes of the base topology.
Admission gives no code 14. A salt gets code 14 only from the terminal draw.

## Size bounds

The size bounds are the base topology itself:

```text
N(p, T) = n_nodes(T)
E(p, T) = n_edges(T)
D(p, T) = min(max_degree(T), N - 1)
```

The parameters do not change the sizes.

## Max retries

| Step | Max tries |
|---|---|
| Terminal draw (step 6) | 4 |

The terminal draw tries at most 4 times. After 4 failed tries the salt gives code 14 and the miner skips it. No other step retries. A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
