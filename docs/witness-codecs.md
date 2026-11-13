# Compact instance and witness codecs

The eight canonical revision-1 families support lossless compact instances and exact instance-order witnesses.
Use `ising_generators::registry` from Rust or `quip_solver_core.families` from Python.
Protocol result spins, instance digests, symmetry handling, and duplicate checks remain unchanged.
Chain adoption requires a separate validator deployment. WebAssembly and C exports remain deferred.

## Descriptor context and validation

Both codecs require an admitted family ID, revision, and parameter digest.
The payload contains no codec identifier or revision fallback.
Instance decoding preserves canonical bytes, including topology digest, nonce bytes, node order, coefficients, offset, and metadata.
A zero nonce decodes as `Some([0; 32])`. Canonical bytes cannot distinguish this from an absent nonce.

Instance decoding checks structure and bounds without generator replay or energy evaluation.
It accepts a structural superset of generated instances.
Run family validation before accepting an instance from an untrusted source.
Witness preparation validates family metadata once and owns its prepared layout.
Rust callers can reuse that layout across witnesses. Python calls prepare the layout for each operation.
Preparation can cost more than decoding because family validation reconstructs constraints.
Prepared witness decoding takes linear time in the spin count and prepared metadata size.
Instance decoding takes linear time and space in nodes, edges, and metadata.

## Instance wire layout

Every payload is `H || C(ids) || C(endpoints) || D(h) || D(j) || F`.
The 112-byte header preserves descriptor digest, topology digest, and nonce as three 32-byte fields.
It then stores node count and edge count as `u32le`, followed by offset as `i64le`.
Endpoints follow canonical edge order. Node IDs keep their registration order.

`C(values)` starts with a one-byte width, followed by unsigned values packed least-significant bit first.
The width is zero for empty or all-zero values. Otherwise, use the bit length of the largest value.
Each column starts at a byte boundary and has zero high padding.
Counts come from the schema. A nonminimal width is invalid.

`D(values)` contains a `u32le` palette count, ascending distinct `i32le` coefficients, and a column of palette ranks.
Every palette entry must occur. Empty values require an empty palette.
Coefficients keep their integer milli units.

All family scalar integers are little-endian. Each bit array and column has separate byte alignment.
The decoder reconstructs metadata section headers.
Let `B(x)=ceil(x/8)` and `c(n,w)=1+B(n*w)`.
The following table gives the family payload in wire order.

| Family | Payload F |
|---|---|
| polariton-static | Window origin as two u16 values, two-bit interior charges, position column. |
| defects-static | Polariton payload, 48 fixed bytes, u16 obstacle count and obstacle column, u16 pin count and pin column. |
| flow-static | Window origin, boundary-label bits, position column, eight weight bytes, target bits, cost column. |
| twisted-ice-dimer | u32 tile count, tile-position column, local gauge bits, u32 pair count, pair-position column. |
| charged-ice | u32 tile count, tile-position column, local gauge bits, two-bit tile charges. |
| ice-glass | u32 tile count, tile-position column, global gauge bits. |
| ice-fields | u32 tile count, tile-position column, global gauge bits. |
| two-commodity-routing | u32 cell count, cell-coordinate column, two-bit supplies, u32 arc count, arc-position column, cost column, u16 strength. |

Charge symbols are 0 for zero, 1 for +2, and 2 for -2. Symbol 3 is invalid.
Routing supplies encode -1, 0, and +1 as 0, 1, and 2. Symbol 3 is invalid.

The decoder first checks the input cap, counts, expanded sizes, lengths, padding, and symbols.
It then checks descriptor identity, structure, and minimality, in that order.
Column preflight borrows input without allocating count-sized arrays. Minimality is a separate shared check.

## Witness wire layout

All fields use least-significant bits first. A raw bit is 1 for +1 and 0 for -1.
Only the final byte has padding. Extra bytes and nonzero padding are invalid.
No witness carries a count, descriptor, length prefix, or checksum.

### Hard families

Polariton, defects, and flow use a deterministic forest over square links.
Remove pinned links, queue all free vertices in ascending order, then visit the smallest unvisited roots.
Breadth-first traversal follows link index within each adjacency list.
Write non-parent unpinned link bits in link order.
Reverse discovery reconstructs parent spins and checks closed-component root residuals.
Defects pins are +1. Obstacles remove no witness links.
Flow labels every vertex and ignores targets during witness encoding.
Only feasible hard witnesses can encode.

### Soft families

The first bit selects structured mode 0 or fallback mode 1.
Select disjoint blocks in metadata order, with twisted dimers before tiles.
Routing selects equal-spin arc chains. Glass and fields use global gauges.
Charged and twisted ice use local gauges.

Structured mode writes block ranks, then uncovered raw spins in ascending instance position.
Pair ranks are `--=0` and `++=1`.
Tile mask bit `i` is one when `g[i]*s[i]=+1`.
Zero-charge masks `[3,5,6,9,10,12]` use three-bit ranks.
Charge +2 masks `[7,11,13,14]` and charge -2 masks `[1,2,4,8]` use two-bit ranks.

Fallback writes every raw spin after the mode bit.
Fallback is legal exactly when at least one selected block fails its rule.
An empty selection always uses structured mode.
Ignored tiles, fields, routing capacity, and conservation do not constrain structured eligibility.
Every correctly sized vector containing only -1 and +1 remains representable for soft families.

## Errors and Python usage

Python raises `CodecError(ValueError)` with a stable `name` attribute.
The exception is separate from `FamilyError` and carries no FAM numeric code.
Descriptor admission still raises `AdmissionError`. Allocation failure raises `MemoryError`.

Stable codec names are `unsupported_family`, `descriptor_mismatch`, `invalid_instance`, `bounds_exceeded`,
`length`, `padding`, `spin_value`, `invalid_symbol`, `infeasible`, `non_canonical`, `allocation`, and `unencodable_instance`.

Given an already generated or validated `instance`:

```python
from quip_solver_core import CodecError, families

context = (family_id, 1, params, families.params_digest(params))
compact_instance = families.encode_instance(*context, instance)
restored = families.decode_instance(*context, compact_instance)
assert restored.canonical_bytes() == instance.canonical_bytes()
compact_witness = families.encode_witness(*context, instance, spins)
assert families.decode_witness(*context, instance, compact_witness) == spins
```

## Size formulas

Canonical instance bytes equal `113+8N+12E+M`.
Let `d(n,k)=4+4k+c(n,ceil(log2(max(1,k))))`.
Compact prefix bytes equal `112+c(N,w_id)+c(2E,w_endpoint)+d(N,k_h)+d(E,k_j)`.
Add the family payload bytes, with each column header and padded bit array included.
Instance bit counts equal eight times stored byte counts.

Witness bit counts exclude final padding.
For selected pairs P, zero-charge tiles T0, and charged tiles T2, structured bits equal `1+N-P-T0-2*T2`.
Fallback bits equal `1+N`. Hard bits equal independent forest links.
One-bit storage uses `ceil(N/8)` bytes. Savings compare actual stored bytes and can be negative.

Contract bounds are not deployed chain limits.
Square topology admission limits polariton and flow to side 62, even though their parameter schemas permit side 64.
Defects permits sides 2 and 3. Ice adjacency mode permits 131072 nodes if metadata and topology admission succeed.
Routing admits templates through m=32, subject to retained arcs after pruning and strongly connected component selection.
Upper block-count estimates remain conditional when generation does not realize them.

## Reproduce measurements

The baseline is commit `8a9fb591ec5284e9a64539e799071ea989492dad`, plus the integration changes in this branch.
Inputs use generated Zephyr lattices, disjoint or overlapping K4 graphs, and the archived live topology for recipe rows.
New cases try all-zero nonce bytes first. Retries use bytes 1, 2, and 3 only after construction skips.
Rust independently admits each topology and regenerates each successful instance.
Before reporting sizes, it checks canonical bytes, digests, compact re-encoding, and witness round trips.
Rejected and skipped rows keep their error codes in the JSON fixture.

```sh
PYTHONPATH=conformance python scripts/witness-codec-size-inputs.py inputs.json
cargo run -p ising-generators --example witness_codec_sizes -- --inputs inputs.json --json
cargo run -p ising-generators --example witness_codec_sizes -- --inputs inputs.json --markdown
```

## Measured sizes

The table reports both instance and witness representations in bits and bytes.

| Family / case / mode | Canonical instance bits / bytes | Compact instance bits / bytes | One-bit witness bits / bytes | Compact witness bits / bytes | Status |
|---|---:|---:|---:|---:|---|
| quip/polariton-static / side-2-nonce0 / hard | 2864 / 358 | 1336 / 167 | 12 / 2 | 11 / 2 | measured |
| quip/polariton-static / side-8-nonce0 / hard | 43760 / 5470 | 8824 / 1103 | 144 / 18 | 95 / 12 | measured |
| quip/polariton-static / side-62-nonce0 / hard | 2926064 / 365758 | 837936 / 104742 | 7812 / 977 | 4091 / 512 | measured |
| quip/polariton-static / side-64-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/flow-static / side-2-nonce0 / hard | 4992 / 624 | 1616 / 202 | 12 / 2 | 4 / 1 | measured |
| quip/flow-static / side-8-nonce0 / hard | 58080 / 7260 | 11024 / 1378 | 144 / 18 | 64 / 8 | measured |
| quip/flow-static / side-62-nonce0 / hard | 3309312 / 413664 | 882064 / 110258 | 7812 / 977 | 3844 / 481 | measured |
| quip/flow-static / side-64-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/defects-static / side-2-obstacles-0-pins-0-nonce0 / hard | 4864 / 608 | 2136 / 267 | 12 / 2 | 11 / 2 | measured |
| quip/defects-static / side-2-obstacles-0-pins-2-nonce0 / hard | 4928 / 616 | 2216 / 277 | 12 / 2 | 9 / 2 | measured |
| quip/defects-static / side-2-obstacles-1-pins-2-nonce0 / hard | 4960 / 620 | 2192 / 274 | 12 / 2 | 9 / 2 | measured |
| quip/defects-static / side-3-obstacles-0-pins-0-nonce0 / hard | 8952 / 1119 | 2768 / 346 | 24 / 3 | 20 / 3 | measured |
| quip/defects-static / side-3-obstacles-0-pins-2-nonce0 / hard | 9016 / 1127 | 2872 / 359 | 24 / 3 | 18 / 3 | measured |
| quip/defects-static / side-3-obstacles-1-pins-2-nonce0 / hard | 9048 / 1131 | 2848 / 356 | 24 / 3 | 18 / 3 | measured |
| quip/defects-static / side-4-obstacles-0-pins-0-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/defects-static / side-4-obstacles-0-pins-2-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/defects-static / side-4-obstacles-1-pins-2-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/ice-glass / k4-16-nonce0 / structured | 5048 / 631 | 1488 / 186 | 16 / 2 | 13 / 2 | measured |
| quip/ice-glass / k4-16-nonce0 / fallback | 5048 / 631 | 1488 / 186 | 16 / 2 | 17 / 3 | measured |
| quip/charged-ice / k4-16-charge-0-nonce0 / structured | 5160 / 645 | 1496 / 187 | 16 / 2 | 13 / 2 | measured |
| quip/charged-ice / k4-16-charge-0-nonce0 / fallback | 5160 / 645 | 1496 / 187 | 16 / 2 | 17 / 3 | measured |
| quip/charged-ice / k4-16-charge-256-nonce0 / structured | 5160 / 645 | 1544 / 193 | 16 / 2 | 9 / 2 | measured |
| quip/charged-ice / k4-16-charge-256-nonce0 / fallback | 5160 / 645 | 1544 / 193 | 16 / 2 | 17 / 3 | measured |
| quip/twisted-ice-dimer / k4-16-pairs-0-nonce0 / structured | 5320 / 665 | 1608 / 201 | 16 / 2 | 13 / 2 | measured |
| quip/twisted-ice-dimer / k4-16-pairs-0-nonce0 / fallback | 5320 / 665 | 1608 / 201 | 16 / 2 | 17 / 3 | measured |
| quip/twisted-ice-dimer / k4-16-pairs-2-nonce0 / structured | 5448 / 681 | 1568 / 196 | 16 / 2 | 15 / 2 | measured |
| quip/twisted-ice-dimer / k4-16-pairs-2-nonce0 / fallback | 5448 / 681 | 1568 / 196 | 16 / 2 | 17 / 3 | measured |
| quip/ice-fields / overlap-16-nonce0 / structured | 4120 / 515 | 1528 / 191 | 16 / 2 | 16 / 2 | measured |
| quip/ice-fields / overlap-16-nonce0 / fallback | 4120 / 515 | 1528 / 191 | 16 / 2 | 17 / 3 | measured |
| quip/ice-glass / k4-4096-nonce0 / structured | 1016888 / 127111 | 257128 / 32141 | 4096 / 512 | 3073 / 385 | measured |
| quip/ice-glass / k4-4096-nonce0 / fallback | 1016888 / 127111 | 257128 / 32141 | 4096 / 512 | 4097 / 513 | measured |
| quip/charged-ice / k4-4096-charge-0-nonce0 / structured | 1025160 / 128145 | 259176 / 32397 | 4096 / 512 | 3073 / 385 | measured |
| quip/charged-ice / k4-4096-charge-0-nonce0 / fallback | 1025160 / 128145 | 259176 / 32397 | 4096 / 512 | 4097 / 513 | measured |
| quip/charged-ice / k4-4096-charge-256-nonce0 / structured | 1025160 / 128145 | 263304 / 32913 | 4096 / 512 | 2049 / 257 | measured |
| quip/charged-ice / k4-4096-charge-256-nonce0 / fallback | 1025160 / 128145 | 263304 / 32913 | 4096 / 512 | 4097 / 513 | measured |
| quip/twisted-ice-dimer / k4-4096-pairs-0-nonce0 / structured | 1066120 / 133265 | 276688 / 34586 | 4096 / 512 | 3073 / 385 | measured |
| quip/twisted-ice-dimer / k4-4096-pairs-0-nonce0 / fallback | 1066120 / 133265 | 276688 / 34586 | 4096 / 512 | 4097 / 513 | measured |
| quip/twisted-ice-dimer / k4-4096-pairs-512-nonce0 / structured | 1098888 / 137361 | 282256 / 35282 | 4096 / 512 | 3585 / 449 | measured |
| quip/twisted-ice-dimer / k4-4096-pairs-512-nonce0 / fallback | 1098888 / 137361 | 282256 / 35282 | 4096 / 512 | 4097 / 513 | measured |
| quip/ice-fields / overlap-4096-nonce0 / unavailable | null / null | null / null | null / null | null / null | skipped |
| quip/ice-fields / overlap-4096-nonce1 / unavailable | null / null | null / null | null / null | null / null | skipped |
| quip/ice-fields / overlap-4096-nonce2 / unavailable | null / null | null / null | null / null | null / null | skipped |
| quip/ice-fields / overlap-4096-nonce3 / unavailable | null / null | null / null | null / null | null / null | skipped |
| quip/ice-glass / k4-131072-nonce0 / structured | 32506936 / 4063367 | 11469928 / 1433741 | 131072 / 16384 | 98305 / 12289 | measured |
| quip/ice-glass / k4-131072-nonce0 / fallback | 32506936 / 4063367 | 11469928 / 1433741 | 131072 / 16384 | 131073 / 16385 | measured |
| quip/charged-ice / k4-131072-charge-0-nonce0 / structured | 32769160 / 4096145 | 11535464 / 1441933 | 131072 / 16384 | 98305 / 12289 | measured |
| quip/charged-ice / k4-131072-charge-0-nonce0 / fallback | 32769160 / 4096145 | 11535464 / 1441933 | 131072 / 16384 | 131073 / 16385 | measured |
| quip/charged-ice / k4-131072-charge-256-nonce0 / structured | 32769160 / 4096145 | 11666568 / 1458321 | 131072 / 16384 | 65537 / 8193 | measured |
| quip/charged-ice / k4-131072-charge-256-nonce0 / fallback | 32769160 / 4096145 | 11666568 / 1458321 | 131072 / 16384 | 131073 / 16385 | measured |
| quip/ice-fields / overlap-131072-nonce0 / structured | 36933208 / 4616651 | 13515824 / 1689478 | 131072 / 16384 | 109266 / 13659 | measured |
| quip/ice-fields / overlap-131072-nonce0 / fallback | 36933208 / 4616651 | 13515824 / 1689478 | 131072 / 16384 | 131073 / 16385 | measured |
| quip/two-commodity-routing / zephyr-2-4-nonce0 / structured | 8296 / 1037 | 2384 / 298 | 24 / 3 | 17 / 3 | measured |
| quip/two-commodity-routing / zephyr-2-4-nonce0 / fallback | 8296 / 1037 | 2384 / 298 | 24 / 3 | 25 / 4 | measured |
| quip/two-commodity-routing / zephyr-12-4-nonce0 / structured | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 1761 / 221 | measured |
| quip/two-commodity-routing / zephyr-12-4-nonce0 / fallback | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 2641 / 331 | measured |
| quip/two-commodity-routing / zephyr-32-3-nonce0 / structured | 9114856 / 1139357 | 3203904 / 400488 | 23424 / 2928 | 15617 / 1953 | measured |
| quip/two-commodity-routing / zephyr-32-3-nonce0 / fallback | 9114856 / 1139357 | 3203904 / 400488 | 23424 / 2928 | 23425 / 2929 | measured |
| quip/polariton-static / polariton-recipe-l4-salt0-nonce0 / hard | 10288 / 1286 | 2720 / 340 | 40 / 5 | 31 / 4 | measured |
| quip/polariton-static / polariton-recipe-l4-salt1-nonce0 / hard | 10288 / 1286 | 2720 / 340 | 40 / 5 | 31 / 4 | measured |
| quip/polariton-static / polariton-recipe-l8-salt0-nonce0 / hard | 43760 / 5470 | 9672 / 1209 | 144 / 18 | 95 / 12 | measured |
| quip/polariton-static / polariton-recipe-l8-salt1-nonce0 / hard | 43760 / 5470 | 9672 / 1209 | 144 / 18 | 95 / 12 | measured |
| quip/polariton-static / polariton-recipe-l9-salt0-nonce0 / hard | 56008 / 7001 | 12056 / 1507 | 180 / 23 | 116 / 15 | measured |
| quip/polariton-static / polariton-recipe-l9-salt1-nonce0 / hard | 56008 / 7001 | 11840 / 1480 | 180 / 23 | 116 / 15 | measured |
| quip/flow-static / flow-recipe-l4-salt0-nonce0 / hard | 15840 / 1980 | 5088 / 636 | 40 / 5 | 16 / 2 | measured |
| quip/flow-static / flow-recipe-l4-salt1-nonce0 / hard | 15840 / 1980 | 5088 / 636 | 40 / 5 | 16 / 2 | measured |
| quip/flow-static / flow-recipe-l6-salt0-nonce0 / hard | 33536 / 4192 | 10216 / 1277 | 84 / 11 | 36 / 5 | measured |
| quip/flow-static / flow-recipe-l6-salt1-nonce0 / hard | 33536 / 4192 | 10184 / 1273 | 84 / 11 | 36 / 5 | measured |
| quip/flow-static / flow-recipe-l8-salt0-nonce0 / hard | 58080 / 7260 | 17824 / 2228 | 144 / 18 | 64 / 8 | measured |
| quip/flow-static / flow-recipe-l8-salt1-nonce0 / hard | 58080 / 7260 | 17888 / 2236 | 144 / 18 | 64 / 8 | measured |
| quip/flow-static / flow-recipe-l12-salt0-nonce0 / unavailable | null / null | null / null | null / null | null / null | rejected |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1200-salt0-nonce0 / structured | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 3312 / 414 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1200-salt0-nonce0 / fallback | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 4576 / 572 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1200-salt1-nonce0 / structured | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 3308 / 414 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1200-salt1-nonce0 / fallback | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 4576 / 572 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1350-salt0-nonce0 / structured | 4647464 / 580933 | 1357064 / 169633 | 4575 / 572 | 3182 / 398 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1350-salt0-nonce0 / fallback | 4647464 / 580933 | 1357064 / 169633 | 4575 / 572 | 4576 / 572 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1350-salt1-nonce0 / structured | 4647464 / 580933 | 1357064 / 169633 | 4575 / 572 | 3176 / 397 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w62-p1350-salt1-nonce0 / fallback | 4647464 / 580933 | 1357064 / 169633 | 4575 / 572 | 4576 / 572 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w45-p1200-salt0-nonce0 / structured | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 3298 / 413 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w45-p1200-salt0-nonce0 / fallback | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 4576 / 572 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w45-p1200-salt1-nonce0 / structured | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 3304 / 413 | measured |
| quip/twisted-ice-dimer / twisted-recipe-x-w45-p1200-salt1-nonce0 / fallback | 4637864 / 579733 | 1353160 / 169145 | 4575 / 572 | 4576 / 572 | measured |
| quip/charged-ice / charge-recipe-ice-charge-salt0-nonce0 / structured | 4575344 / 571918 | 1339352 / 167419 | 4575 / 572 | 3265 / 409 | measured |
| quip/charged-ice / charge-recipe-ice-charge-salt0-nonce0 / fallback | 4575344 / 571918 | 1339352 / 167419 | 4575 / 572 | 4576 / 572 | measured |
| quip/charged-ice / charge-recipe-ice-charge-salt1-nonce0 / structured | 4575344 / 571918 | 1339352 / 167419 | 4575 / 572 | 3309 / 414 | measured |
| quip/charged-ice / charge-recipe-ice-charge-salt1-nonce0 / fallback | 4575344 / 571918 | 1339352 / 167419 | 4575 / 572 | 4576 / 572 | measured |
| quip/ice-fields / fields-recipe-h500-salt0-nonce0 / structured | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 3737 / 468 | measured |
| quip/ice-fields / fields-recipe-h500-salt0-nonce0 / fallback | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 4576 / 572 | measured |
| quip/ice-fields / fields-recipe-h500-salt1-nonce0 / structured | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 3720 / 465 | measured |
| quip/ice-fields / fields-recipe-h500-salt1-nonce0 / fallback | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 4576 / 572 | measured |
| quip/ice-fields / fields-recipe-h1000-salt0-nonce0 / structured | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 3736 / 467 | measured |
| quip/ice-fields / fields-recipe-h1000-salt0-nonce0 / fallback | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 4576 / 572 | measured |
| quip/ice-fields / fields-recipe-h1000-salt1-nonce0 / structured | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 3719 / 465 | measured |
| quip/ice-fields / fields-recipe-h1000-salt1-nonce0 / fallback | 4540464 / 567558 | 1328568 / 166071 | 4575 / 572 | 4576 / 572 | measured |
| quip/two-commodity-routing / routing-recipe-r32-salt0-nonce0 / structured | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 1761 / 221 | measured |
| quip/two-commodity-routing / routing-recipe-r32-salt0-nonce0 / fallback | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 2641 / 331 | measured |
| quip/two-commodity-routing / routing-recipe-r32-salt1-nonce0 / structured | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 1761 / 221 | measured |
| quip/two-commodity-routing / routing-recipe-r32-salt1-nonce0 / fallback | 972616 / 121577 | 286760 / 35845 | 2640 / 330 | 2641 / 331 | measured |
| quip/ice-fields / overlap-4096-divisor32-nonce0 / structured | 1155032 / 144379 | 307488 / 38436 | 4096 / 512 | 3406 / 426 | measured |
| quip/ice-fields / overlap-4096-divisor32-nonce0 / fallback | 1155032 / 144379 | 307488 / 38436 | 4096 / 512 | 4097 / 513 | measured |
| quip/polariton-static / golden-l2-reverse-False / hard | 2864 / 358 | 1336 / 167 | 12 / 2 | 11 / 2 | measured |
| quip/defects-static / golden-defects-l2-0 / hard | 4928 / 616 | 2184 / 273 | 12 / 2 | 9 / 2 | measured |
| quip/flow-static / golden-flow-l2-0 / hard | 4992 / 624 | 2040 / 255 | 12 / 2 | 4 / 1 | measured |
| quip/twisted-ice-dimer / golden-pairs-0-seed-0 / structured | 4200 / 525 | 1528 / 191 | 12 / 2 | 12 / 2 | measured |
| quip/twisted-ice-dimer / golden-pairs-0-seed-0 / fallback | 4200 / 525 | 1528 / 191 | 12 / 2 | 13 / 2 | measured |
| quip/charged-ice / golden-charge-0-0 / structured | 2160 / 270 | 1192 / 149 | 4 / 1 | 4 / 1 | measured |
| quip/charged-ice / golden-charge-0-0 / fallback | 2160 / 270 | 1192 / 149 | 4 / 1 | 5 / 1 | measured |
| quip/ice-glass / golden-ice-glass-adjacency-z12-0 / structured | 10360 / 1295 | 2288 / 286 | 24 / 3 | 23 / 3 | measured |
| quip/ice-glass / golden-ice-glass-adjacency-z12-0 / fallback | 10360 / 1295 | 2288 / 286 | 24 / 3 | 25 / 4 | measured |
| quip/ice-fields / golden-shared-fields-3 / structured | 3616 / 452 | 1504 / 188 | 9 / 2 | 9 / 2 | measured |
| quip/ice-fields / golden-shared-fields-3 / fallback | 3616 / 452 | 1504 / 188 | 9 / 2 | 10 / 2 | measured |
| quip/two-commodity-routing / golden-generated / structured | 8296 / 1037 | 2384 / 298 | 24 / 3 | 17 / 3 | measured |
| quip/two-commodity-routing / golden-generated / fallback | 8296 / 1037 | 2384 / 298 | 24 / 3 | 25 / 4 | measured |
