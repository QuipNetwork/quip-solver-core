# vectors/restricted-field revision 1

This contract describes the layered test construction. The construction is a test family for the conformance suite and the template for SC-8.
Its Rust contract is `quip_solver_conformance::families::layered::RESTRICTED_V1`.
Its vectors are in `crates/quip-solver-conformance/vectors/families/layered.json`.

## ID

`vectors/restricted-field`

## Revision

`1`

## Status

`research`

The registry holds the status (FAM-09). This line must equal the registry status.

## Parameter schema

| Tag | Name | Type | Scale | Min | Max |
|---|---|---|---|---|---|
| 1 | n_layers | u16 | 1 | 1 | 16 |
| 2 | field_milli | i32 | 1000 | -4000 | 4000 |

The bounds are inclusive and apply to the stored integer.

## Substream labels

| Label | Draws |
|---|---|
| salt | One `uniform(8)` value `w`. |
| couplings | One `uniform(3)` value per instance edge, in sorted edge order. |

## Construction steps

Let `L` be `n_layers`, `f` be `field_milli`, and `n` be the base node count.
Check every operation. An overflow returns code 13, `BoundsExceeded`.

1. Draw `w = uniform(salt, 8)`. If `w = 0`, return code 14, `NoInstance`.
2. If `w = 2`, compute `f * 2^21` in `i32`. An overflow returns code 13. The value is not used.
3. Set `scale = 2` if `w = 1`, else `scale = 1`. Set `h = f * scale` for every node.
4. Number the nodes `k * n + pos` for `k` in `0..L` and `pos` in `0..n`.
5. For each layer `k`, add each base edge `(a, b)` as `(k * n + min(a, b), k * n + max(a, b))`. For `k` in `0..L-1`, add `(k * n + pos, (k + 1) * n + pos)` for each `pos`. Sort the edges in ascending order.
6. For each edge in sorted order, set `J = (uniform(couplings, 3) - 1) * 1000`.
7. Set `offset_milli = 250 * L`.
8. Write the metadata region, 14 bytes. The bytes are `01 00`, `01 00`, `06 00 00 00`, then `L` as `u16` LE, then `n` as `u32` LE.

Topology admission rejects a base topology that repeats an edge.
Instance generation then applies the contract limits. A coefficient with `|h| > 2000` returns code 13.

## Output order

`default`

This is the default order (FAM-31). Node `k * n + pos` is output position `k * n + pos`.

## Offset

`250 * n_layers`

The offset is in milli units.

## Metadata

| Kind | Name | Payload |
|---|---|---|
| 1 | tiles | 6 bytes. `n_layers` as `u16` LE, then the base node count `n` as `u32` LE. |

Validation: the region is exactly one section of kind 1 with that payload. The product `n * n_layers` must equal the instance node count.

## Constraint class

`soft`

This family has no feasibility constraints.

## Bounds

| Quantity | At most |
|---|---|
| each `\|h\|` | 2000 milli |
| each `\|J\|` | 2000 milli |

Every other bound is the FAM-37 bound.

Step 2 overflows by design when `|field_milli| >= 1024`. This exercises generation code 13 (FAM-27).
This breaks the FAM-38 Bounds rule, which requires bounds that make overflow impossible.
A production family must not copy step 2.

## Size bounds

`n_layers` is the parameter value `L`. The size bounds are:

```text
N(p, T) = L * n_nodes(T)
E(p, T) = L * n_edges(T) + (L - 1) * n_nodes(T)
D(p, T) = max_degree(T) + min(L - 1, 2)
```

## Max retries

`none`

No step retries. Step 1 returns code 14 when `w = 0`, to exercise the skip path.
That is not retry exhaustion (FAM-26). A production family must not copy step 1.
