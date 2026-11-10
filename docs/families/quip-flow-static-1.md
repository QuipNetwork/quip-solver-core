# quip/flow-static revision 1

This contract describes the static flow family. The family is a `hard` research family. It is the adapted active-flow task on an open square-link lattice of side `L`. One unit of flow runs along every link. A state is feasible when each vertex conserves flow against its request, and the objective is the cost of the links that point against their requested direction.
The lattice sits on the native layout of `quip/polariton-static`, with one Zephyr cell K4 for each vertex and one shared segment qubit for each link.
The Rust contract is `ising_generators::flow::FLOW_V1`.
The vectors are the `cases` section of `crates/quip-solver-conformance/vectors/families/flow.json`. The same file holds the flow_witnesses, networks, flow_recipes, and admission sections.
The lattice, the link order, the incidence, and the direct charge follow `docs/square-links.md`.

The family is a static pattern. It makes no claim about a device, a propagation, or a physical flow. It has no research cohort. The registered recipes are the four parameter sets of the Recipes section.

The family differs from two other families.

- `quip/ice-fields` puts fields on tile spins and has no hard conservation rule. A state of this family is feasible only when every vertex holds its charge.
- `quip/two-commodity-routing` has two commodities, and it allows an arc that carries no flow. Here every link carries exactly one unit, and there is one commodity.

The family rests on these decisions.

- **Conservation.** Every vertex has a label. The charge of a vertex is the number of links that point into it minus the number that point out of it. A label of `+1` is a sink and a label of `-1` is a source.
- **Request.** Each link has a target direction `t_e` and an even cost `c_e`. The objective is `O(s) = sum c_e [s_e != t_e]`.
- **Penalty.** A conservation penalty with a checked weight makes every infeasible state cost more than every feasible state.
- **Baseline.** The function `flow_network` returns the supplies, the links, the targets, and the costs. A min-cost circulation solver can read them. The family has no solver, and a solver that finds the optimum with an exact flow method does not fail the family.

## ID

`quip/flow-static`

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
| 4 | cost_half_max | u16 | 1 | 0 | 500 |

The bounds are inclusive and apply to the stored integer.
The pair `zephyr_m` and `zephyr_t` names the lattice `z(zephyr_m, zephyr_t)`. The base topology is a subgraph of that lattice.
The parameter `side` is `L`, the number of cells of the square-link lattice on each side.
The parameter `cost_half_max` is the largest value of `c_e / 2`. A cost is an even number from 0 to `2 * cost_half_max`.

### Constants

| Name | Value | Use |
|---|---|---|
| `DRAW_TRIES` | 4 | The number of boundary draws before the salt gives code 14 |

## Substream labels

The labels come in this order.

| Label | Draws |
|---|---|
| flow | Each try shuffles the list of the edge vertices with `uniform(i + 1)` values for `i` from `K - 1` down to 1, where `K` is the length of the list. After the first try that passes, the stream draws the targets and the costs, one pair for each link in link order. Each pair is one `uniform(2)` value, then one `uniform(cost_half_max + 1)` value. The stream continues across the tries and the pairs and is never opened again. |

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

### The request

Link `e` has a spin `s_e`. The spin `+1` sends the unit from the tail of the link to its head, and the spin `-1` sends it back. The charge of a vertex `v` is `Q_v`, the sum of incidence times spin over its links. A link that points into `v` adds `+1` and a link that points out of `v` adds `-1`.

Corner vertices have degree 2, interior vertices have degree 4, and every other boundary vertex has degree 3 and is an edge vertex. A label has the parity of the degree of its vertex, so a corner and an interior vertex have the label 0 and an edge vertex has an odd label. The request gives an edge vertex the label `+1` or `-1`.

4. **Draw.** Let `W` be the `4 (L - 1)` edge vertices in vertex order of the open lattice (`v = y * (L + 1) + x`). For try 1 to 4, on the stream `flow`:
   1. Copy the list `W` in its natural order and shuffle the copy.
   2. Label the first `2 (L - 1)` entries `+1` and the rest `-1`. Label every other vertex 0. The labels sum to 0.

   Stop and keep this try when the labels pass the checks of `docs/square-links.md` for a request with no fixed links. After 4 failed tries the salt gives no instance. The construction returns code 14, `NoInstance`, and the miner skips the salt (FAM-26). Each try starts from the list in its natural order and continues the same stream.
5. **Targets and costs.** On the same stream, after the first try that passes, draw one pair for each link `e` in link order. Draw `x = uniform(2)` and set `t_e = +1` when `x` is 1 and `t_e = -1` when `x` is 0. Then draw `u_e = uniform(cost_half_max + 1)` and set `c_e = 2 u_e`. The costs are even, so `c_e / 2` is exact. A cost of 0 is allowed, and its target stays in the metadata.
6. **Weight.** Let `R` be the sum of the costs. Let `U = R / 2 + 1` in integer division, raised by 1 when that value is odd. Then `U` is even and `2 U > R`.
7. **Energy.** All values are in milli.
   - Objective: `O(s) = sum c_e (1 - t_e s_e) / 2`. Subtract `c_e t_e / 2` from `h[e]` and add `c_e / 2` to the offset for each link. The objective adds no coupler.
   - Charges: each vertex `v` in vertex order has the label `q_v` and adds `(U / 2) * (Q_v - q_v)^2` in three parts. Each pair of links `e < f` at `v` adds `U * inc(v, e) * inc(v, f)` to `J[e, f]`. Each link `e` at `v` subtracts `U * q_v * inc(v, e)` from `h[e]`. The vertex also adds `(U / 2) * (deg(v) + q_v^2)` to the offset.

   The edges are the pairs with a nonzero `J`, in ascending dense positions. Two links share at most one vertex, so no pair cancels. Every such pair is an edge of the base topology, because the two links share a vertex and are corners of one cell K4. Any other pair is a generator defect.
8. **Output.** The instance nodes are every link qubit, in the registration order of the base topology (FAM-31).
9. **Metadata.** Write the region with the kinds 4, 5, and 9, as the metadata section below states.

### The penalty bound

The objective `O(s)` lies between 0 and `R`, so `O(g) - O(f) <= R` for any two states. A violated label means `Q_v` differs from `q_v`. Both have the parity of the degree of `v`, so they differ by an even amount and `(Q_v - q_v)^2 >= 4`. Then a state with at least one violated label has a penalty of at least `(U / 2) * 4 = 2 U`. A feasible state has a penalty of 0.

Let `f` be a feasible state and `g` an infeasible state. Then

```text
E(g) >= O(g) + 2 U > O(g) + R >= R >= O(f) = E(f)
```

The instance energy of every infeasible state is above the instance energy of every feasible state. The lowest energy of the instance is a feasible state with the lowest objective. A state with a lower objective that breaks a label is not feasible, and its instance energy is above the best feasible energy.

### Feasibility

A state is feasible exactly when every label holds. The vectors hold, for each case, a feasible state and an infeasible state with a lower objective. At `L = 2` the feasible state has the lowest objective of all 4096 states.

The requests of step 4 are not always feasible. The charge request fails when a set of vertices needs more net flow than the links that leave it can carry. At `L = 5` the labeling that gives the first 8 edge vertices the label `+1` is refused. It sends 8 units across the 6 links between two rows. Exactly 4 of the 12870 balanced labelings at `L = 5` fail, and no balanced labeling fails at `L` of 2, 3, or 4. The retry of step 4 can run, and code 14 needs 4 failed tries in a row. The registered vectors all build at the first try. A Rust test of the construction covers the retry path and the four failures.

### Flow network

The function `flow_network(params, instance)` returns `FlowNetwork`, or `None` when `check_metadata` fails. It has two fields.

| Field | Meaning |
|---|---|
| `supplies` | The supply of each vertex, in vertex order. It is the negated label, so a source has `+1`, a sink has `-1`, and the supplies sum to 0. |
| `arcs` | One arc for each link, in link order. An arc has the `tail` and `head` vertices of the link, the target `+1` or `-1`, and the cost. |

A flow along an arc from `tail` to `head` is the spin `+1`. The network carries one unit on every arc in one of its two directions. The module has no solver.

### Observables

The function `flow_observables(params, instance, spins)` returns `FlowObservables`, or `None` when `check_metadata` fails or when `spins` has the wrong length or a value that is not `+1` or `-1`. It has these fields.

| Field | Meaning |
|---|---|
| `objective` | The sum of the costs of the links with `s_e` not equal to `t_e` |
| `charges` | The direct charge of every vertex, in vertex order |
| `violations` | The number of vertices whose charge differs from the label |
| `reversed` | The number of links with `s_e` not equal to `t_e` |

## Output order

`default`

This is the default order (FAM-31). The nodes are in registration order of the base topology.

## Offset

`sum over links of c_e / 2, plus sum over vertices of (U / 2) * (deg(v) + q_v^2)`

The offset is in milli units. The value is the sum of the offsets of step 7.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 4 | charges | `L` as `u16`, then `(L + 1)^2` labels as `i8` in vertex order. Every vertex has a label. |
| 5 | incidence | `c0` as `u16`, `r0` as `u16`, then `2 L (L + 1)` entries as `u32`. Each entry is the dense instance position of one link, in the link order of the open lattice. |
| 9 | flow | `U` as `u64`, then for each link in link order the target `t_e` as `i8` and the cost `c_e` as `u32`. |

The region has three sections, kind 4, kind 5, and kind 9, and nothing follows them. All integers are little-endian. Kind 9 is registered in FAM-34.

The metadata check `check_metadata` is true only when every one of these rules holds.

- All sections parse exactly, and the `L` of kind 4 equals `side`.
- Every interior vertex and every corner vertex has the label 0. Every edge vertex has the label `+1` or `-1`, and the count of `+1` labels equals the count of `-1` labels.
- The window satisfies `c0 >= 1`, `r0 >= 1`, `c0 + L <= G`, and `r0 + L <= G`.
- The link positions are distinct and are exactly the instance nodes, so `n` equals `2 L (L + 1)`.
- Every target is `+1` or `-1`.
- Every cost is even and at most `2 * cost_half_max`.
- `U` equals the weight of step 6 for the costs.
- The energy rebuilt from the metadata by step 7 equals the instance `h`, edges, `J`, and offset.

A count that does not fit its payload makes the check false, and the check allocates nothing sized by a count before it compares the count with the payload length.
The check does not test that the draw passed, because the draw is not part of the metadata.

The energy cannot see the target of a link with cost 0. A metadata region with the target of such a link flipped passes `check_metadata`, because the rebuilt energy is the same. The instance digest covers the metadata bytes, so a verifier that compares the digest detects the change. The target of a link with a nonzero cost fixes the field `h` of that link, and a target that disagrees with `h` fails the check.

## Constraint class

`hard`

A witness is valid only when it meets every feasibility constraint, at any energy. The constraints are these.

| Constraint | Meaning |
|---|---|
| Length | The witness has exactly one spin of `+1` or `-1` for each instance node, in instance node order. |
| Metadata | The metadata of the instance passes `check_metadata`. |
| Charges | For every vertex, the direct charge of the witness equals the label. The direct charge is the sum of incidence times spin over the links of the vertex, computed with the link positions of kind 5. |

The Rust hook is the `feasible` field of `ising_generators::flow::FLOW`. The wiring of `feasible` into result verification (FAM-53) is a separate change, so the coordinator does not call the hook yet.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 8320504 milli |
| each `\|J\|` | 4160002 milli |
| `\|offset\|` | 35139536892 milli |
| nodes | 8320 |
| edges | 24574 |
| metadata bytes | 79139 |

The limits are computed for `side` 64 with `cost_half_max` 500. That is a safe upper bound. The window rule `L + 1 <= 2m - 1` and the bound `m <= 32` give 62 as the largest side that builds, so side 63 and side 64 never build. The weight grows with the side, so the bound at side 64 covers every side that builds.

- Nodes: `2 L (L + 1) = 8320` links.
- Edges: each pair of links at a vertex is one coupler. Interior vertices give 6 pairs each, which is `63^2 * 6 = 23814`. Edge vertices give 3 pairs each, which is `252 * 3 = 756`. The 4 corners give 1 pair each. The total is 24574.
- The sum of the half costs is at most `8320 * 500 = 4160000`, so `R / 2 + 1 = 4160001` is odd and `U = 4160002`.
- Each `|h|`: a link has two ends. Each end has a label of 0 or of magnitude 1, so the charge penalty gives at most `2 U = 8320004`. The objective adds at most 500. The result is 8320504.
- Each `|J|`: a coupler comes from one vertex and has the magnitude `U`, which is 4160002.
- Offset: the charge penalty is `(U / 2) * sum (deg(v) + q_v^2)`. The degrees sum to `2 * 8320 = 16640`, and the squared labels sum to `4 (L - 1) = 252`. The penalty then is `2080001 * 16892 = 35135376892`. The objective adds at most `4160000`. The total is 35139536892.
- Metadata: the count takes 2 bytes and each section has 6 bytes of header. The payload of kind 4 has `2 + 4225` bytes. The payload of kind 5 has `4 + 8320 * 4 = 33284` bytes. The payload of kind 9 has `8 + 8320 * 5 = 41608` bytes. The total is 79139 bytes.

Every other bound is the FAM-37 bound or a stricter bound of this contract.

### Topology admission

Params faults come first. A malformed params region gives code 7, and a value outside its bounds gives code 9. A `side` of 1 or 65 is such a value.
Topology admission then applies these checks. The first failure decides the code.

1. A node that is not a node of `z(zephyr_m, zephyr_t)` gives code 12.
2. An edge that is not a coupler of the lattice gives code 12. A self-loop is such an edge.
3. `L + 1 > G` gives code 12.
4. A topology with no fully alive window gives code 12. The window does not depend on the nonce, so admission computes it.

After these checks, the size bounds give code 13 when an advertised limit of the solver is below the sizes.
Admission gives no code 14. The `admission` section of the vectors records the code of each request.

The vectors record two cases of code 12 for a window. The topology of `flow-dead-cell` has a dead qubit in the first window, so the window moves to the origin `(1, 2)` and the request is admitted. The topology without an alive window gives code 12.

### Recipes

The recipes are the parameter sets on the live z(12, 4) topology with `cost_half_max` 500. The `flow_recipes` section of the vectors holds two salts of each.

| Alias | Cell | `side` | Result on the live topology |
|---|---|---|---|
| `l4` | `flow-l4` | 4 | Builds |
| `l6` | `flow-l6` | 6 | Builds |
| `l8` | `flow-l8` | 8 | Builds |
| `l12` | `flow-l12` | 12 | Code 12 |

The recipe `l12` needs a window without dead qubits. The live topology has dead qubits and no fully alive window above side 9, so admission gives code 12. The recipe has no salts in the vectors.

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
| Boundary draw (step 4) | 4 |

The boundary draw tries at most 4 times. After 4 failed tries the salt gives code 14 and the miner skips it. No other step retries. A topology that fails admission gets code 12 or code 13, and no salt gives a draw.
