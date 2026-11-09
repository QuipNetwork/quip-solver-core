# Dipolar coefficient tables

The module `ising_generators::dipolar` holds quantized dipolar coupling tables for axis-aligned square ice. It also holds the exact integer validator for those tables. The module registers no family. A later family reads a table through its commitment.

Every check in the module uses integers. The source has no floating point type and no floating point literal. A test searches the source for them.

## Geometry

An island has an integer center `(x, y)` with `0 <= x, y <= 256`. It has an easy axis along `+x`, `-x`, `+y`, or `-y`. It has an integer moment `m` with `|m| <= 16`. Two islands never share a center.

A geometry has the islands in index order, the table constant `D`, the integer scale `S`, and the squared truncation radius `R2`.

## Formula

Take islands `i` and `j`. Let `r` be the vector from island `i` to island `j`. Let `K = r . r`. Let `u_i` and `u_j` be the unit axis vectors. The coupling is:

```
J = D * m_i * m_j * N / K^(5/2)
N = K * (u_i . u_j) - 3 * (u_i . r) * (u_j . r)
```

The value `N` is an integer because the axes are axis aligned. `N` has the same value for `r` and for `-r`, so the pair has one coupling. The units of `D` and of the moments come from the calibration. An oracle table has no units.

## Rounding

The table stores `q = round(S * J)`. The rounding goes to the nearest integer, and a tie goes away from zero. A tie can happen only when `K` is a perfect square, because otherwise `K^(5/2)` is irrational.

Let `x = S * D * m_i * m_j * N`. This is an integer. Let `T = K^(5/2)`. An entry `q` is correct when:

- for `q > 0`: `(2q - 1) T <= 2x < (2q + 1) T`
- for `q < 0`: `(2q - 1) T < 2x <= (2q + 1) T`
- for `q = 0`: `-T < 2x < T`

## The exact check

The check compares an integer `b` with `a * K^2 * sqrt(K)`, where `a` is an odd integer. The check first compares the signs of `b` and `a`. When the signs differ, the sign decides. When both are zero, they are equal. When both are non-negative, it compares `b^2` with `a^2 * K^5` in `u128`. When both are negative, it compares the same values and reverses the result. The validator rejects an entry that is too large before it calls this check, so no product overflows, see the next section. The helper still saturates an overflowing product to the largest value, which is correct because such a product is at least `2^128` and `b^2` is below `2^128`.

The unit tests compare this helper with a second method. The range of `K` is 1 to 40. The range of `a` is minus 9 to 9. The range of `b` is minus 3000 to 3000. The second method brackets `sqrt(K)` between two fixed point integers.

## The `u128` bound

The bounds are:

- at most 8320 islands
- `0 <= x, y <= 256`
- `|m| <= 16`
- `1 <= S` and `1 <= |D|` with `S * |D| <= 2^20`
- `R2 <= 2 * 256^2 = 2^17`

Then `K <= 2 * 256^2 = 2^17`. The value `|N|` is at most `2K`. If the axes are parallel, `N` is `K` minus `3 d^2` or `-K` plus `3 d^2`, where `d^2 <= K` is the square of the offset along the axis. Both forms lie between `-2K` and `2K`. If the axes are perpendicular, `|N| = 3 |d_x d_y| <= 3K/2`. This gives:

```
|x| <= 2^20 * 16^2 * 2K = 2^29 * K <= 2^46
|2x| <= 2^47
(2x)^2 <= 2^94
```

A table entry `q` is an `i32`, so it can be far from the correct value. The check rejects an entry before it squares anything. Let `s = isqrt(K)`. The limit is `L = ceil(|x| / (K^2 * s)) + 1`. The correct coefficient has `|q| <= ceil(|x| / K^(5/2))`. Since `s <= sqrt(K)`, this is at most `ceil(|x| / (K^2 * s))`, which is less than `L`. The limit admits every correct coefficient. The check calls an entry with `|q| > L` wrong and does no further arithmetic on it.

Every other entry has `|q| <= L`, so `|a| = |2q +- 1| <= 2L + 1 <= 2|x| / (K^2 * s) + 5`. Multiply by `K^(5/2)` and use `sqrt(K) < s + 1 <= 2s`:

```
|a| * K^(5/2) <= 2 |x| * sqrt(K) / s + 5 * K^(5/2)
              <  4 |x| + 5 * K^(5/2)
              <= 2^48 + 5 * 2^43
              <  2^49
```

The last two lines use `|x| <= 2^46` and `K^(5/2) < 2^43`, because `K <= 2^17`. Squaring gives `a^2 * K^5 < 2^98`, which is below `2^128`. The other side has `b^2 <= 2^94`. Both products fit in `u128` with no overflow. The saturating multiplication in the helper never triggers for a value that reaches it from the validator, and this proof does not depend on it. The helper keeps the saturation so that it stays correct for any input.

The estimate step shifts `|x|` left by 30 bits, which gives at most `2^76`. The denominator `K^2 * floor(sqrt(K) * 2^30)` is at most `2^34 * 2^38.5`, which is below `2^73`.

The geometry constructor rejects a value one step over a bound. A test covers each bound at the limit and one step over it.

## Building a table

`quantize` returns every pair inside the radius with its exact `q`. For each pair it computes an integer estimate from `floor(sqrt(K) * 2^30)`. The estimate is within one of the answer. It then moves by one step at a time until the exact check accepts, and it fails with an error after four steps. A test pins the result for a fixed geometry.

## Truncation

The table holds every pair with `K <= R2` and no other pair. The validator first checks the shape of every entry in this order: index, mirrored pair, self pair, and order. It then checks the set of pairs and every value. Each fault has its own error. The faults are a wrong value, a missing pair, a pair outside the radius, a mirrored pair, a pair of an island with itself, an entry out of order, and an index out of range.

The omitted part of the coupling has an upper bound. For every pair with `K > R2`, the per-pair upper bound is `|x| / (K^2 * isqrt(K))`, where `isqrt` is the integer square root rounded down. Since `isqrt(K) <= sqrt(K)`, this is at least `S * |J|` for that pair. The bound is the sum of the per-pair bounds, each rounded up, as a `u128`. The sum of the rounded terms can be larger than the rounded sum of the terms. This is the amended rule D4.

The rounding error is at most one half unit of `q` for each entry. `rounding_bound` returns the number of entries, which is the count of half units.

## Digests

Both digests use BLAKE3 in key derivation mode. Integers are little endian.

- The geometry digest uses the context `quip dipolar geometry v1`. The bytes are the island count as a `u32`. Each island follows with `x` as a `u16`, `y` as a `u16`, the axis code as a `u8`, and the moment as an `i8`. The axis codes are 0 for `+x`, 1 for `-x`, 2 for `+y`, and 3 for `-y`. The constants end the bytes: `D` as an `i32`, `S` as a `u32`, and `R2` as a `u32`.
- The table commitment uses the context `quip dipolar table v1`. The bytes start with the geometry digest and the entry count as a `u64`. Each entry follows with `i` as a `u32`, `j` as a `u32`, and `q` as an `i32`. The calibration ends the bytes. The byte 0 marks an oracle. The byte 1 marks a record, and the material and the normalization follow it, each with a `u32` length. The lattice spacing, the approximation error, and the 32 byte source digest finish the record.

The commitment covers the geometry digest. A change to a coordinate, an axis, a moment, `D`, `S`, or `R2` changes the commitment.

## Calibration status

A table has either `Calibration::Oracle` or `Calibration::Recorded`. The record of a recorded table names the material, the lattice spacing in picometers, the normalization, the approximation error in parts per million, and a 32 byte source digest. The two text fields are not empty and have at most 256 bytes.

`physical_status` returns `InsufficientPhysicalEvidence` for an oracle table. It returns `RecordedUnreviewed` for a recorded table. No result is a physical validation success. A reviewer approval of a model is a later step, and no code in this module performs it.

## The oracle table

The test `the_oracle_table_is_frozen` pins one table. The table is an oracle and a fixed test table. No calibrated device model backs it.

The islands are the 12 links of the open lattice with `L = 2` from `ising_generators::square`, in link order. Each link is one island at its midpoint, in doubled coordinates so that the midpoints are integers. A horizontal link from `(x, y)` to `(x + 1, y)` is the island at `(2x + 1, 2y)` with axis `+x`. A vertical link is the island at `(2x, 2y + 1)` with axis `+y`. Every moment is 1. The constants are `D = 1`, `S = 1024`, and `R2 = 8`.

The table has 38 entries `(i, j, q)`:

```
(0, 1, 543)  (0, 2, -256)  (0, 3, -543)  (0, 5, 128)
(0, 7, -23)  (1, 3, 128)  (1, 5, -543)  (1, 6, -256)
(1, 8, -23)  (2, 3, 543)  (2, 4, -543)  (2, 5, -23)
(2, 7, 128)  (3, 4, 128)  (3, 5, 543)  (3, 6, -23)
(3, 7, -543)  (3, 8, -256)  (3, 9, -23)  (4, 7, 543)
(4, 8, -23)  (4, 9, -256)  (5, 6, 543)  (5, 7, -256)
(5, 8, -543)  (5, 10, 128)  (5, 11, -23)  (6, 8, 128)
(6, 10, -543)  (7, 8, 543)  (7, 9, -543)  (7, 10, -23)
(7, 11, 128)  (8, 9, 128)  (8, 10, 543)  (8, 11, -543)
(9, 11, 543)  (10, 11, -256)
```

The geometry digest is `45a43f9d9641ae8e2100274213b46e2738fa0ad78e5efc9dd14db9f30ae5b692`. The table commitment is `efe917918f6d04ba87c27941caf2341ec3cb8625c3c7668fefa217bd292e365f`.

## Open decisions

**blocked / needs-decision:** no calibrated physical record exists yet. No table supports a physical claim until a reviewer approves a model and a record.
