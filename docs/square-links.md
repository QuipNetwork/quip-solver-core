# Square-link primitives

The module `ising_generators::square` and the module `ising_generators::witness` hold the shared primitives for link-based families on a square lattice. The modules give a signed link graph, direct charges, charge requests, packed witness bytes, labeled constraints, and a canonical witness identity. A later family registers its own id, vectors, and registry entry.

A charge check reads the spins and the request only. The check does not depend on any penalty energy, coefficient, or Ising instance. Neither module imports instance code.

## Lattice

The side `L` is at least 2 and at most 64.

- A periodic lattice has vertices `(x, y)` with `0 <= x, y < L` and index `v = y * L + x`. Each vertex owns a `+x` link to `((x + 1) mod L, y)` and a `+y` link to `(x, (y + 1) mod L)`. It has `L^2` vertices and `2 L^2` links. Every vertex has degree 4. For `L = 2` the wrap makes parallel links. Links 0 and 2 both join vertices 0 and 1.
- An open lattice has `L` cells per side and vertices `(x, y)` with `0 <= x, y <= L`. The index is `v = y * (L + 1) + x`. A vertex owns a `+x` link when `x < L` and a `+y` link when `y < L`. It has `(L + 1)^2` vertices and `2 L (L + 1)` links.
- The links follow the index order of their owning vertex. The `+x` link comes first, then the `+y` link. The owning vertex is the tail and the neighbor is the head.

Spin +1 points from the tail to the head. The incidence is minus 1 at the tail and plus 1 at the head. The charge of a vertex is the sum over its links of incidence times spin. It equals the arrows into the vertex minus the arrows out of it. Three facts hold for every spin vector. The charge has the same parity as the degree. Its size is at most the degree. The charges sum to 0.

## Request

A request has three parts.

- `charges` has one entry per vertex. A value labels the vertex with a target charge. `None` leaves the vertex free. On an open lattice the boundary vertices are free unless a label names them.
- `fixed` lists links with a fixed spin, either plus 1 or minus 1, in ascending link order with no repeats.
- `fields` is empty, or has one signed value per link.

`check_request` runs these checks in order. The first failure wins.

1. Length: `charges`, then `fields`.
2. Fixed links. The range test, the ascending test, and the spin test each run over the whole list before the next test starts.
3. Degree bound.
4. Parity.
5. Total: the labels cover every vertex and do not sum to 0.
6. Feasibility: some orientation of the links meets every label and every fixed spin.

The first five checks are local. A request can pass them and still fail the last check. On the periodic lattice with `L = 2`, vertices 0 and 1 cannot both take charge 4. A fixed spin that contradicts a label also fails the last check.

## Witness

A witness holds one spin per link in link order. Link `i` is bit `i % 8` of byte `i / 8`. A set bit means spin +1. Padding bits are 0, and the length is exactly `ceil(links / 8)`. This is the FAM-53 packing rule.

`check_witness` first rejects malformed bytes with an error: a wrong length or a set padding bit. Only then does it check constraints. `constraints` lists the fixed spins by link, then the charge labels by vertex. The label of a constraint is its position in that list. `check_witness` returns the labels of the failed constraints in ascending order. A fixed spin that contradicts a charge label shows up as a failed label.

## Symmetries

The candidate lattice maps are the 8 elements of the dihedral group D4. On a periodic lattice the maps act about vertex 0 modulo `L` and combine with the `L^2` translations. On an open lattice the maps act about the center of the grid and use no translation. Each lattice map appears twice: with global spin reversal and without it.

A map sends the link with tail `t` and direction `d` to the link with tail `g(t)` and direction `R d`, where `R` is the linear part of the map. If `R d` is `+x` or `+y`, the image is that link with sign plus 1. If `R d` is `-x` or `-y`, the image is the link owned by `g(t) + R d` in the opposite direction, with sign minus 1. Links map by tail and direction, never by vertex pair. This keeps the parallel links of a small torus apart.

A candidate passes when it maps the request onto itself. With `r = -1` under reversal and `r = 1` otherwise, these conditions hold.

- `charges[g(v)] = r * charges[v]` for every vertex.
- The fixed set maps onto the fixed set, with each spin multiplied by `sign * r`.
- `fields[g(e)] = sign * r * fields[e]` for every link.

Fields and boundary labels can reduce the group. A request with all-zero fields keeps every candidate. A field-biased state is never merged with its reversal.

`symmetries` returns every candidate that passes in a fixed order: translation, then D4 element, then no reversal before reversal. It does not remove candidates that act the same way on the links. On small tori some do.

## Canonical witness

The canonical bytes of a witness are the least packed image over the candidates that pass, compared as bytes. When only the identity passes, the bytes stay the same.

The cost is at most `2 * 8 * L^2` images, and each image takes time linear in the number of links. For `L = 64` that is 65,536 images of 8,192 links.
