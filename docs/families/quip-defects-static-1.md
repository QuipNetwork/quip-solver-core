# quip/defects-static revision 1

This contract describes the static defect family. The family is a `hard` research family. It selects a low physical energy square-ice state under requested charges, pins, and obstacles. The lattice is the open square-link lattice of side `L` on the native layout of `quip/polariton-static`. Each lattice vertex is the K4 of one Zephyr cell, and each link is one segment qubit that two neighbouring cells share.
The Rust contract is `ising_generators::defects::DEFECTS_V1`.
The vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/defects.json`. The same file holds the witnesses, routes, table, and admission sections.
The lattice, the link order, the incidence, and the direct charge follow `docs/square-links.md`. The dipolar table follows `docs/dipolar-numerics.md`.

A static pattern shows no propagation, switching, or transport. The family has no research cohort and no registered recipe. A route in this family is combinatorial evidence about one state and makes no propagation claim.

The family rests on three decisions of the maintainer.

- **U1, couplings.** The instance builds a dipolar table with `R2 = 4` over the link midpoints and keeps only the entries whose two links share a lattice vertex. At each vertex these are the perpendicular pairs and the collinear pair through it. Every kept pair is a K4 coupler of the native layout. The dropped entries are the parallel pairs at `K = 4`. The metadata states their exact sum of `|q|`. The table also states the bound for the pairs beyond `R2`.
- **U2, calibration.** The table has the oracle calibration. Revision 1 allows `side` 2 and 3 only and registers no recipe. The sides 4, 6, 8, and 12 wait on a calibrated record, which does not exist yet. The observables always report `InsufficientPhysicalEvidence`.
- **U3, constraints.** The charges come from `pairs` as in `quip/polariton-static`. Obstacles are interior vertices with the label 0, and a route may not enter them. Pins fix boundary links to the reference state. The family checks an optional route for its endpoints, steps, repeats, obstacles, and exact agreement with the final spins.

## ID

`quip/defects-static`

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
| 3 | side | u16 | 1 | 2 | 3 |
| 4 | pairs | u16 | 1 | 0 | 2 |
| 5 | obstacles | u16 | 1 | 0 | 4 |
| 6 | pins | u16 | 1 | 0 | 12 |
| 7 | field_x | i32 | 1 | -1000 | 1000 |
| 8 | field_y | i32 | 1 | -1000 | 1000 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` names the lattice `z(zephyr_m, zephyr_t)`. The base topology is a subgraph of that lattice.
The parameter `side` is `L`, the number of cells of the square-link lattice on each side.
The parameter `pairs` is the number of interior vertices with the label `+2`, and also the number with the label `-2`.
The parameter `obstacles` is the number of interior vertices with the label 0 that a route may not enter.
The parameter `pins` is the number of boundary links that the reference state fixes.
The parameters `field_x` and `field_y` are the field along `+x` and along `+y`. The stored integer is in milli of `D`, so the scale is 1.

### Constants

| Name | Value | Use |
|---|---|---|
| `D` | 1 | The dipolar constant |
| `S` | 1000 | The dipolar scale, so a table entry `q` is the coupling in milli of `D` |
| `R2` | 4 | The squared truncation radius of the table |
| `DRAW_TRIES` | 4 | The number of request draws before the salt gives code 14 |
| free label | 127 | The metadata value of a vertex without a label |

## Substream labels

The labels come in this order.

| Label | Draws |
|---|---|
| charges | Each try draws three shuffles in this order. The first shuffles the interior vertices. The second shuffles the zero-labeled interior vertices. The third shuffles the boundary links. Each shuffle draws one `uniform(i + 1)` value for each `i` from `K - 1` down to 1, where `K` is the length of the list. The stream continues across the shuffles and across the tries and is never opened again. |

## Construction steps

Let `m` be `zephyr_m`, let `t` be `zephyr_t`, let `L` be `side`, and let `G` be `2m - 1`. A cell is a pair `(c, r)` with `1 <= c, r <= G`.
Check every operation. An overflow returns code 13, `BoundsExceeded`.
A defect in a topology that admission accepted is a generator defect. The generator reports it as the construction code 12, `TopologyIncompatible`, and it gives no instance.

### The window

The window uses the rule of `quip/polariton-static` without change. The text of that rule follows.

**Segments.** The function `seg(u, w, k, p)` is the native id of the Zephyr coordinates `(u, w, k, j, z)` with `j = p mod 2` and `z = p / 2` (integer division). It exists when `0 <= p < 2m` and that id is a node of the base topology. Otherwise the result is none.

**Cell K4.** The function `cell_k4(c, r, k, k2)` is the four segments `(seg(0, c, k, r - 1), seg(0, c, k, r), seg(1, r, k2, c - 1), seg(1, r, k2, c))`. Call them VLO, VHI, HL, and HR. It exists when all four segments exist and all six pairs of them are edges of the base topology. Otherwise the result is none.

1. **Layout.** Each row has one line and each column has one line. `rows[r]` and `cols[c]` start at 0. A cell `(c, r)` is alive when `cell_k4(c, r, cols[c], rows[r])` exists. Repeat the following until a sweep changes nothing.
   - For each `r` in ascending order, score every line in `0..t` as the number of alive cells of row `r` with the current `cols`. If the best score is larger than the score of `rows[r]`, set `rows[r]` to the smallest line with the best score.
   - Then do the same for each `c` in ascending order and `cols[c]`, with the updated `rows`.
2. **Window.** The candidate origins are `(c0, r0)` with `c0 >= 1`, `r0 >= 1`, `c0 + L <= G`, and `r0 + L <= G`, in order of `c0` ascending and then `r0` ascending. The window is the first origin whose `(L + 1)^2` cells `(c0 + x, r0 + y)`, for `0 <= x, y <= L`, are all alive. Lattice vertex `(x, y)` of the open lattice is the cell `(c0 + x, r0 + y)`.
3. **Links.** The open link from vertex `(x, y)` along `+x` is `seg(1, r0 + y, rows[r0 + y], c0 + x)`. The link along `+y` is `seg(0, c0 + x, cols[c0 + x], r0 + y)`. The links at one vertex are the four corners of one cell K4, and every pair of them is a coupler.

### The geometry

Link `e` of the open lattice is island `e` of a dipolar geometry. A link along `+x` from the vertex `(x, y)` sits at `(2x + 1, 2y)` with the axis `+x`. A link along `+y` sits at `(2x, 2y + 1)` with the axis `+y`. Every moment is 1. The constants are `D = 1`, `S = 1000`, and `R2 = 4`.

### The reference state

The reference state has every link spin `+1`. A link spin `+1` points from the tail of the link to its head. Each interior vertex has two links that point into it and two that point out of it, so its charge is 0. Every pin uses the spin `+1`. The reference state is the state that a route is measured against.

### The salt

Let `V` be the interior vertices, those with `1 <= x, y <= L - 1`, in vertex order of the open lattice (`v = y * (L + 1) + x`). A vertex lies on the outer border when `x` or `y` is 0 or `L`, and a boundary link has both ends on the outer border.

4. **Draw.** For try 1 to 4, on the stream `charges`, in this order:
   1. Copy the list `V`, shuffle the copy, label the first `pairs` entries `+2`, label the next `pairs` entries `-2`, and label the rest 0. Every boundary vertex is free.
   2. Take the interior vertices with the label 0 in vertex order, and shuffle them. The first `obstacles` entries are the obstacles. Sort them ascending.
   3. Take the boundary links in link order, and shuffle them. The first `pins` entries are the pinned links. Sort them ascending.

   Stop and keep this try when the charge request passes the checks of `docs/square-links.md` for the labels and for the pins as fixed spins of +1. After 4 failed tries the salt gives no instance. The construction returns code 14, `NoInstance`, and the miner skips the salt (FAM-26). Each try starts from the lists in their natural order and continues the same stream.
5. **Table.** Build the geometry above and the table of every pair of islands with `K <= R2`, where `K` is the squared distance between the two midpoints. The entry is `q = round(S * J)` with ties away from zero, in the arithmetic of `docs/dipolar-numerics.md`. The table has the oracle calibration. Keep an entry `(i, j, q)` when links `i` and `j` share a lattice vertex. Let `dropped` be the sum of `|q|` over the other entries, and let `kept_magnitude` be the sum of `|q|` over the kept entries.
6. **Energy.** All values are in milli.
   - Physical: add `q` to `J[i, j]` for each kept entry, and subtract `field_x` from `h[e]` for each link along `+x` and `field_y` for each link along `+y`.
   - Weight: `U` is the penalty weight of the penalty bound below.
   - Charges: each labeled vertex `v` in vertex order has the charge `Q_v`, the sum of incidence times spin over its links. It adds `(U / 2) * (Q_v - q_v)^2` in three parts. Each pair of links `e < f` at `v` adds `U * inc(v, e) * inc(v, f)` to `J[e, f]`. Each link `e` at `v` subtracts `U * q_v * inc(v, e)` from `h[e]`. The vertex also adds `(U / 2) * (deg(v) + q_v^2)` to the offset.
   - Pins: for each pinned link `e`, subtract `U` from `h[e]` and add `U` to the offset. The term is `U * (1 - s_e)`.

   The edges are the pairs with a nonzero `J`, in ascending dense positions. Every such pair is an edge of the base topology, because the two links share a vertex and are corners of one cell K4. Any other pair is a generator defect.
7. **Output.** The instance nodes are every link qubit, in the registration order of the base topology (FAM-31). A link that touches no labeled vertex and has no field keeps its place as a node.
8. **Metadata.** Write the region with the kinds 4, 5, and 8, as the metadata section below states.

### The penalty bound

The physical energy is `P(s) = sum J_phys s_i s_j + sum h_phys s_e`. Each term `c * x` with `x` equal to `+1` or `-1` ranges over `[-|c|, |c|]`, so its range has width `2 |c|`. The range of `P` over all states is at most

```text
R = 2 * (sum |q| over the kept entries + sum |h_phys| over the links)
```

The weight is `U = R / 2 + 1` in integer division, raised by 1 when that value is odd. Then `U` is even and `2 U > R`.

A labeled vertex has the degree of its links, and the label has the same parity as that degree, because the interior degree is 4 and the labels are `-2`, 0, and `+2`. Then `Q_v - q_v` is even. A violated label costs at least `(U / 2) * 4 = 2 U`. A violated pin has `s_e = -1` and costs `U * (1 - s_e) = 2 U`. Then every infeasible state has a penalty of at least `2 U`, and every feasible state has a penalty of 0.

Let `f` be a feasible state and `g` an infeasible state. Then

```text
E(g) >= P(g) + 2 U > min P + R >= max P >= P(f) = E(f)
```

The instance energy of every infeasible state is above the instance energy of every feasible state. The lowest energy of the instance is a feasible state with the lowest physical energy.

### Feasibility

A state is feasible exactly when every label holds and every pin has spin +1. A state with energy below the feasible energies cannot exist, as the bound shows. A state with low physical energy and a violated label or pin is not feasible, whatever its physical energy.
Admission accepts a request only when the draw can succeed. The draw never gives code 14 for admitted parameters. A boundary link joins two boundary vertices, so a pin never touches a labeled vertex. Every interior vertex has degree 4. The interior links of the `(L - 1)` by `(L - 1)` block can form cycles that give each interior vertex one link in and one link out, which adds charge 0. Each interior vertex also has free links to boundary vertices. At `L = 2` the one interior vertex has four free links. At `L = 3` each interior vertex has two. Those links give charge `+2` with both in, `-2` with both out, and 0 with one of each. Every set of labels holds. The contract keeps the four tries and code 14 for the same draw rule as `quip/polariton-static`, and the vectors have no skipped salt.

### Routes

A route is a list of paths, and each path is a sequence of at least 2 lattice vertex ids. Each step of a path joins two vertices that share a link. The endpoints of every path are one `+2` and one `-2` labeled vertex, and every `+2` or `-2` labeled vertex is the endpoint of exactly one path.

The function `check_route(params, instance, spins, route)` returns `Ok` or a `RouteError`. The checks run in this order over the whole route, and the first failure wins.

| Error | Meaning |
|---|---|
| `Metadata` | The metadata of the instance fails `check_metadata`. |
| `Spins` | `spins` is not one `+1` or `-1` entry for each instance node. |
| `Shape` | A path has fewer than 2 vertices, or a vertex is not in the lattice. |
| `NotAdjacent` | A step of a path joins two vertices that share no link. |
| `RepeatedVertex` | A path visits a vertex twice. |
| `RepeatedLink` | Two steps use the same link. |
| `Obstacle` | A path visits an obstacle. |
| `Endpoints` | The endpoints of a path are not one `+2` and one `-2` labeled vertex, or a `+2` or `-2` labeled vertex is not the endpoint of exactly one path. |
| `Difference` | The set of links with spin `-1` is not exactly the union of the links of the paths. |

A route is combinatorial evidence about the state. It is not a propagation claim. It does not say that a signal travels along the path, and it does not say how the state came about. A valid route never changes the result of `feasible`, and an invalid route never changes it either. The vectors hold a valid route with a feasible state and an invalid route with a feasible state, and so show that the two results are independent.

The route vectors use a single `+2` and a single `-2` vertex, so a cross pairing of two paths is not among them. The vector named `swapped-endpoints` replaces the `-2` endpoint of a valid path with a vertex that carries no opposite label. The check gives `Endpoints` for it.

### Observables

The function `defect_observables(params, instance, spins)` returns `DefectObservables`, or `None` when `check_metadata` fails or when `spins` has the wrong length or a value that is not `+1` or `-1`. It has these fields.

| Field | Meaning |
|---|---|
| `physical_energy` | The physical energy `P(s)` in milli, without any penalty |
| `charges` | The direct charge of every vertex, in vertex order |
| `violated_labels` | The number of labeled vertices whose charge differs from the label |
| `violated_pins` | The number of pinned links whose spin is not +1 |
| `truncation_bound` | The table bound on the omitted magnitude of the pairs beyond `R2`, in units of `q` |
| `dropped` | The sum of `|q|` over the entries that the native layout cannot carry |
| `physical_status` | `InsufficientPhysicalEvidence`, always |

The truncation bound is the sum over every pair with `K > R2` of `|x| / (K^2 * isqrt(K))`, each term rounded up, with `x = S * D * N`. The rule is in `docs/dipolar-numerics.md`.
The observables are information only. The table has the oracle calibration, so the physical energy supports no claim about a device. Sides 4, 6, 8, and 12 wait on a calibrated record, and revision 1 refuses them with code 9.

## Output order

`default`

This is the default order (FAM-31). The nodes are in registration order of the base topology.

## Offset

`sum over labeled vertices of (U / 2) * (deg(v) + q_v^2), plus U for each pin`

The offset is in milli units. The value is the sum of the offsets of step 6.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 4 | charges | `L` as `u16`, then `(L + 1)^2` labels as `i8` in vertex order. The value 127 means free. |
| 5 | incidence | `c0` as `u16`, `r0` as `u16`, then `2 L (L + 1)` entries as `u32`. Each entry is the dense instance position of one link, in the link order of the open lattice. |
| 8 | defects | `U` as `u64`, `dropped` as `u64`, the table commitment as 32 bytes, `n_obstacles` as `u16`, then `n_obstacles` vertex ids as `u32` ascending, `n_pins` as `u16`, then `n_pins` link ids as `u32` ascending. |

The region has three sections, kind 4, kind 5, and kind 8, and nothing follows them. All integers are little-endian. Kind 8 is registered in FAM-34.
The table commitment is the BLAKE3 commitment of `docs/dipolar-numerics.md` over the geometry above, its entries, and the oracle calibration.

The metadata check `check_metadata` is true only when every one of these rules holds.

- All sections parse exactly, and the `L` of kind 4 equals `side`.
- Every boundary label is 127, and every interior label is `-2`, 0, or `+2`. The count of `+2` labels is exactly `pairs`, and so is the count of `-2` labels.
- The window satisfies `c0 >= 1`, `r0 >= 1`, `c0 + L <= G`, and `r0 + L <= G`.
- The link positions are distinct and are exactly the instance nodes, so `n` equals `2 L (L + 1)`.
- The obstacles are distinct interior vertices with the label 0, and their count equals `obstacles`.
- The pins are distinct boundary links, and their count equals `pins`.
- The commitment and `dropped` equal the values rebuilt from the geometry, and `U` equals the weight of the penalty bound.
- The energy rebuilt from the metadata by step 6 equals the instance `h`, edges, `J`, and offset.

A count that does not fit its payload makes the check false, and the check allocates nothing sized by a count before it compares the count with the payload length.
The check does not test that the draw passed, because the draw is not part of the metadata.

## Constraint class

`hard`

A witness is valid only when it meets every feasibility constraint, at any energy. The constraints are these.

| Constraint | Meaning |
|---|---|
| Length | The witness has exactly one spin of `+1` or `-1` for each instance node, in instance node order. |
| Metadata | The metadata of the instance passes `check_metadata`. |
| Charges | For every labeled vertex, the direct charge of the witness equals the label. The direct charge is the sum of incidence times spin over the links of the vertex, computed with the link positions of kind 5. |
| Pins | Every pinned link has spin +1. |

The Rust hook is the `feasible` field of `ising_generators::defects::DEFECTS`. The function `ising_generators::family::feasible` returns the result of the hook. The hook reads neither a route nor the physical energy, and the coordinator does not call the hook yet. The wiring of `feasible` into result verification (FAM-53) is a separate change.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 189328 milli |
| each `\|J\|` | 47612 milli |
| `\|offset\|` | 1318296 milli |
| nodes | 24 |
| edges | 96 |
| metadata bytes | 254 |

The limits hold for `side` 3 with `|field_x| = |field_y| = 1000`. The weight depends only on the table and the fields, and side 3 with both fields at 1000 gives the largest `U`.

- The table has 52 kept entries at side 3, one for each pair of links at a vertex. A corner vertex has 1 pair, an edge vertex has 3, and an interior vertex has 6, and the lattice has 4 corner, 8 edge, and 4 interior vertices. Of these, 16 pairs are collinear with `|q|` of 250, and 36 pairs are perpendicular with `|q|` of 530. The kept magnitude is `16 * 250 + 36 * 530 = 23080`. The 24 links each have `|h| = 1000`, so `sum |h| = 24000`. Then `R = 2 * 47080`, `R / 2 + 1 = 47081` is odd, and `U = 47082`.
- Nodes: 24 links, which is `2 L (L + 1)`.
- Edges: 96, the size bound. The 52 pairs at a vertex are the most that a coupler can join.
- Each `|h|`: a link between two interior vertices with the labels `+2` and `-2` has `4 U` from the charge penalty, and the field adds at most 1000. The result is `4 * 47082 + 1000 = 189328`.
- Each `|J|`: a coupler has at most `530 + U = 47612`.
- Offset: each of the 4 interior vertices adds at most `(U / 2) * (4 + 4) = 4 U`, which is `16 U` in total. The 12 pins add `12 U`. The total is `28 * 47082 = 1318296`.
- Metadata: the count takes 2 bytes and each section has 6 bytes of header. The payload of kind 4 has 2 + 16 bytes. The payload of kind 5 has 4 + 24 * 4 bytes. The payload of kind 8 has 8 + 8 + 32 + 2 + 4 * 4 + 2 + 12 * 4 bytes. The total is 254 bytes.

Every other bound is the FAM-37 bound or a stricter bound of this contract.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. Side 4 is such a value, so the sides above 3 give code 9.
Topology admission then applies these checks. The first failure decides the code.

1. A node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12.
2. An edge that is not a coupler of the lattice gives code 12. A self-loop is such an edge.
3. `2 * pairs + obstacles > (L - 1)^2` gives code 12.
4. `pins > 4 L`, the number of boundary links, gives code 12.
5. `L + 1 > G` gives code 12.
6. A topology with no fully alive window gives code 12. The window does not depend on the nonce, so admission computes it.

After these checks, the size bounds give code 13 when an advertised limit of the solver is below the sizes.
Admission gives no code 14. The `admission` section of the vectors records the code of each request.

## Size bounds

```text
N(p, T) = min(n_nodes(T), 2 L (L + 1))
E(p, T) = min(n_edges(T), 6 (L + 1)^2)
D(p, T) = min(max_degree(T), 6, N - 1)
```

A link shares a vertex with at most six other links, so its degree is at most 6. The `(L + 1)^2` vertices each join at most six pairs.

## Max retries

| Step | Max tries |
|---|---|
| Request draw (step 4) | 4 |

The request draw tries at most 4 times. After 4 failed tries the salt gives code 14 and the miner skips it. No other step retries. A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
