# quip/polariton-static revision 1

This contract describes the adapted static charge family, the P-static family of FAM-41. The family is the first `hard` family. An open square-link lattice of side `L` sits natively on Zephyr cells. Each lattice vertex is the K4 of one Zephyr cell, and each link is one segment qubit that the two neighbouring cells share. The interior vertices carry balanced charge labels of `+2`, `-2`, and `0`. The boundary vertices are free. The instance holds the charge penalty of every labeled vertex in its couplings, fields, and offset. A witness is valid only when every labeled vertex has its charge.
The Rust contract is `ising_generators::polariton::POLARITON_V1`.
The vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/polariton.json`. The same file holds the witnesses, polariton recipes, and admission sections.
The lattice, the link order, the incidence, and the direct charge follow `docs/square-links.md`. The open lattice is the only boundary this revision uses.

A static pattern does not show optical preparation, retention, or transport. This revision makes no claim about drive, gain, loss, or transport. Its observables are the direct charges, the number of violated labels, and the charge loss. It has no optical field.
The family has its own id, distinct from `quip/charged-ice`, which is a `soft` family with synthetic tile charges.

## ID

`quip/polariton-static`

## Revision

`1`

## Status

`research`

The registry holds the status (FAM-09). This line must equal the registry status.

## Parameter schema

| Tag | Name | Type | Scale | Min | Max |
|---|---|---|---|---|---|
| 1 | zephyr_m | u16 | 1 | 1 | 32 |
| 2 | zephyr_t | u8 | 1 | 1 | 8 |
| 3 | side | u16 | 1 | 2 | 64 |
| 4 | pairs | u16 | 1 | 0 | 2048 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` names the lattice `z(zephyr_m, zephyr_t)`. The base topology is a subgraph of that lattice.
The parameter `side` is `L`, the number of cells of the square-link lattice on each side.
The parameter `pairs` is the number of interior vertices with the label `+2`, and also the number with the label `-2`. A value of 0 labels every interior vertex 0.
The penalty weight `U` is 250 milli and is a constant of this contract, not a parameter.

### Constants

| Name | Value | Use |
|---|---|---|
| `U` | 250 | The penalty weight, in milli. The penalty is `(U / 2)` times the squared charge gap. |
| `CHARGE_TRIES` | 4 | The number of charge draws before the salt gives code 14 |
| free label | 127 | The metadata value of a vertex without a label |

## Substream labels

The labels come in this order.

| Label | Draws |
|---|---|
| charges | One shuffle of the interior vertex list for each try, in order. Each shuffle draws one `uniform(i + 1)` value for each `i` from `K - 1` down to 1, where `K` is the number of interior vertices. The stream continues across the tries and is never opened again. |

## Construction steps

Let `m` be `zephyr_m`, let `t` be `zephyr_t`, let `L` be `side`, and let `G` be `2m - 1`. A cell is a pair `(c, r)` with `1 <= c, r <= G`.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.

### The window

The window depends on the base topology and the parameters. It does not depend on the nonce.

**Segments.** The function `seg(u, w, k, p)` is the native id of the Zephyr coordinates `(u, w, k, j, z)` with `j = p mod 2` and `z = p / 2` (integer division). It exists when `0 <= p < 2m` and that id is a node of the base topology. Otherwise the result is none. The linear id of a coordinate follows the Zephyr rule of `ising_generators::ice`.

**Cell K4.** The function `cell_k4(c, r, k, k2)` is the four segments `(seg(0, c, k, r - 1), seg(0, c, k, r), seg(1, r, k2, c - 1), seg(1, r, k2, c))`. Call them VLO, VHI, HL, and HR. It exists when all four segments exist and all six pairs of them are edges of the base topology. Otherwise the result is none.

1. **Layout.** Each row has one line and each column has one line. `rows[r]` and `cols[c]` start at 0. A cell `(c, r)` is alive when `cell_k4(c, r, cols[c], rows[r])` exists. Repeat the following until a sweep changes nothing.
   - For each `r` in ascending order, score every line in `0..t` as the number of alive cells of row `r` with the current `cols`. If the best score is larger than the score of `rows[r]`, set `rows[r]` to the smallest line with the best score.
   - Then do the same for each `c` in ascending order and `cols[c]`, with the updated `rows`.
2. **Window.** The candidate origins are `(c0, r0)` with `c0 >= 1`, `r0 >= 1`, `c0 + L <= G`, and `r0 + L <= G`, in order of `c0` ascending and then `r0` ascending. The window is the first origin whose `(L + 1)^2` cells `(c0 + x, r0 + y)`, for `0 <= x, y <= L`, are all alive. Lattice vertex `(x, y)` of the open lattice is the cell `(c0 + x, r0 + y)`.
3. **Links.** The open link from vertex `(x, y)` along `+x` is `seg(1, r0 + y, rows[r0 + y], c0 + x)`. This link is the HR corner of its tail cell and the HL corner of its head cell. The link along `+y` is `seg(0, c0 + x, cols[c0 + x], r0 + y)`. This link is the VHI corner of its tail cell and the VLO corner of its head cell. Each link qubit belongs to the two cells it joins and to no other cell. All four corners of a cell use the same row line and column line, and the links at one vertex are the four corners of one cell K4. Every pair of them is a coupler.

### The salt

Let `V` be the interior vertices, those with `1 <= x, y <= L - 1`, in vertex order of the open lattice (`v = y * (L + 1) + x`). The count of interior vertices is `(L - 1)^2`.

4. **Charges.** For try 1 to 4, copy the list `V`, shuffle the copy with the Fisher-Yates shuffle (FAM-24) on the stream `charges`, label the first `pairs` entries `+2`, label the next `pairs` entries `-2`, and label the rest `0`. Leave every boundary vertex free. Stop and keep this try when the charge request passes the checks of `docs/square-links.md` in the order that document gives. After 4 failed tries the salt gives no instance. The construction returns code 14, `NoInstance`, and the miner skips the salt (FAM-26). Each try starts from the list `V` in vertex order and continues the same stream.
5. **Energy.** For each labeled vertex `v` in vertex order, with charge `Q_v` equal to the sum of incidence times spin over its links, add `(U / 2) * (Q_v - q_v)^2` as follows.
   - For each pair of links `e < f` at `v`, add `U * inc(v, e) * inc(v, f)` to `J[e, f]`.
   - For each link `e` at `v`, subtract `U * q_v * inc(v, e)` from `h[e]`.
   - Add `(U / 2) * (deg(v) + q_v^2)` to the offset.

   A free vertex adds nothing. The edges are the pairs with a nonzero `J`, in ascending dense positions. Every such pair is an edge of the base topology, because the two links are corners of one cell K4. Any other pair is a generator defect.
6. **Output.** The instance nodes are every link qubit, in the registration order of the base topology (FAM-31). A link on the outer border touches only free vertices, so it has `h = 0` and no edge. It stays a node and stays in the witness.
7. **Metadata.** Write the region with the kinds 4 and 5, as the metadata section below states.

### Feasibility

The energy of a state is `(U / 2)` times the sum of the squared gaps `Q_v - q_v` over the labeled vertices. Every `Q_v - q_v` is even, so the energy is a whole number of times 500 milli and is 0 exactly when every labeled vertex has its charge. A state with energy 0 exists exactly when the labels pass the feasibility check of `docs/square-links.md`. The construction draws a request that fails the check again.
A balanced request can fail the check even with a free boundary. A block of interior vertices that all carry `+2` needs a net charge of twice its size, and only the links that leave the block can supply it. A block of 2 by 3 vertices needs 12 and has 10 such links.

### Observables

The function `polariton_observables(params, instance, spins)` returns `PolaritonObservables`, or `None` when `check_metadata` fails or when `spins` has the wrong length or a value that is not +1 or –1. It has these fields.

| Field | Meaning |
|---|---|
| `charges` | The direct charge of every vertex, in vertex order |
| `violations` | The number of labeled vertices whose charge differs from the label |
| `loss_milli` | `(U / 2)` times the sum of the squared gaps, in milli |

The observables are information only. A static pattern does not show optical preparation, retention, or transport.

### Recipes

This contract registers three parameter sets on Zephyr z(12, 4) as recipes. The Rust constant is `ising_generators::polariton::POLARITON_RECIPES`. The vectors in the `polariton_recipes` section hold two salts of each recipe on the live topology. Each salt records the hash of the canonical instance, the window, and the try that passed. In each recipe, `pairs` is `floor((side - 1)^2 / 8)`.

| Alias | Cell | `zephyr_m` | `zephyr_t` | `side` | `pairs` | Params digest |
|---|---|---|---|---|---|---|
| `l4` | `polariton-l4` | 12 | 4 | 4 | 1 | `80003c2d963059e9372e19605d16ee8f78b6ca9c69a8c5392999e7706e9fe832` |
| `l8` | `polariton-l8` | 12 | 4 | 8 | 6 | `4a38573ca9a2249698ecc6fc523de5ea67a7b314bf2eed669d5ab65771b16cd9` |
| `l9` | `polariton-l9` | 12 | 4 | 9 | 8 | `f711db3917d0859e91e10ea6e3c61f2d89ad78a8d3bd81cf38ae4c8197b02d3f` |

The live z(12, 4) topology has dead qubits. With the layout of step 1, its largest fully alive block of cells is 10 by 10, and side 9 is the largest side that admits on it. A request with `side` 16 needs a window of 17 by 17 cells, so the request gives code 12 on the live topology. The request builds only on a topology with such a window. The recipe `l16` is not registered for that reason, and `l9` takes its place.

### Intended differences from the research cell

The research cell `ice-charge` (`regimes/qpu-f17/generate3.py`) is the origin of the charge penalty. This revision differs from it in these ways.

- The research cell puts the charge penalty on the edge-disjoint tetrahedra of the suite. This revision puts it on the vertices of a square-link lattice that sits on Zephyr cells.
- The research cell draws a charge for each tile with the chances 1/2 for 0, 1/4 for `+2`, and 1/4 for `-2`. This revision labels exactly `pairs` interior vertices `+2` and `pairs` vertices `-2`, which balances the labels, and it leaves the boundary free.
- The research cell adds a background of plus or minus 31 milli on every coupler that no tile uses. This revision has no background. Every other qubit stays out of the instance.
- The research cell has no feasibility constraint. This revision is `hard`.
- The streams are ChaCha8 streams from the stream seed of the family (FAM-22) and the shuffle is the FAM-24 shuffle. The research cell used a SHA-256 digest of a tag and the nonce.

This family has no research cohort.

## Output order

`default`

This is the default order (FAM-31). The nodes are in registration order of the base topology.

## Offset

`sum over labeled vertices of (U / 2) * (deg(v) + q_v^2)`

The offset is in milli units. The value is the sum of step 5. With `U` equal to 250, every term is a whole number of times 125 milli.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 4 | charges | `L` as `u16`, then `(L + 1)^2` labels as `i8` in vertex order. The value 127 means free. |
| 5 | incidence | `c0` as `u16`, `r0` as `u16`, then `2 L (L + 1)` entries as `u32`. Each entry is the dense instance position of one link, in the link order of the open lattice. |

The region has two sections, kind 4 and then kind 5, and nothing follows them. All integers are little-endian.

The metadata check `check_metadata` is true only when every one of these rules holds.

- Both sections parse exactly, and the `L` of kind 4 equals `side`.
- Every boundary label is 127, and every interior label is `-2`, `0`, or `+2`. The count of `+2` labels is exactly `pairs`, and so is the count of `-2` labels.
- The window satisfies `c0 >= 1`, `r0 >= 1`, `c0 + L <= G`, and `r0 + L <= G`.
- The link positions are distinct and are exactly the instance nodes, so `n` equals `2 L (L + 1)`.
- The energy rebuilt from the metadata by step 5 equals the instance `h`, edges, `J`, and offset.

A count that does not fit its payload makes the check false. The check allocates nothing sized by a count before it compares the count with the payload length.
The check does not test that the charge draw passed, because the draw is not part of the metadata.

## Constraint class

`hard`

A witness is valid only when it meets every feasibility constraint, at any energy. The constraints are these.

| Constraint | Meaning |
|---|---|
| Length | The witness has exactly one spin of +1 or –1 for each instance node, in instance node order. |
| Metadata | The metadata of the instance passes `check_metadata`. |
| Charges | For every labeled vertex, the direct charge of the witness equals the label. The direct charge is the sum of incidence times spin over the links of the vertex, computed with the link positions of kind 5. |

The Rust hook is the `feasible` field of `ising_generators::polariton::POLARITON`. The function `ising_generators::family::feasible` returns the result of the hook.
The coordinator wiring of `feasible` into result verification (FAM-53) is a separate change. This contract defines the constraint.

### Instance encoding

The lossless [compact instance layout](../witness-codecs.md#instance-wire-layout) preserves canonical bytes and requires admitted descriptor context.
Decompression checks structure. Family validation remains separate.

### Witness encoding

The [compact witness layout](../witness-codecs.md#witness-wire-layout) preserves exact instance-order spins.
Hard families require feasible witnesses. Soft families preserve constraint violations through canonical fallback mode.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 1000 milli |
| each `\|J\|` | 250 milli |
| `\|offset\|` | 1099511627776 milli |
| nodes | 131072 |
| edges | 2097152 |
| metadata bytes | 1048576 |

A link is in at most two labeled vertices, and each adds at most `U * 2 = 500` to its `h`, so `|h|` is at most 1000. Two links share at most one vertex in an open lattice, so each `J` has one term of size 250.
Every other bound is the FAM-37 bound or a stricter bound of this contract.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Three such values are `side` 1, `side` 65, and `pairs` 2049.
Topology admission then applies these checks. The first failure decides the code.

1. A node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12.
2. An edge that is not a coupler of the lattice gives code 12. A self-loop is such an edge.
3. `2 * pairs > (L - 1)^2` gives code 12.
4. `L + 1 > G` gives code 12.
5. A topology with no fully alive window gives code 12. The window does not depend on the nonce, so admission computes it.

After these checks, the size bounds give code 13 when an advertised limit of the solver is below the sizes.
Admission gives no code 14. A salt gets code 14 only from the charge draw.

## Size bounds

```text
N(p, T) = min(n_nodes(T), 2 L (L + 1))
E(p, T) = min(n_edges(T), 6 (L - 1)^2)
D(p, T) = min(max_degree(T), 6, N - 1)
```

Each interior vertex adds at most 6 edges. A link is in at most 2 labeled vertices, so its degree is at most 6.

## Max retries

| Step | Max tries |
|---|---|
| Charge draw (step 4) | 4 |

The charge draw tries at most 4 times. After 4 failed tries the salt gives code 14 and the miner skips it. No other step retries. A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
