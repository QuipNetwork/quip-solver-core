# Topology file format

This document defines the topology blob format, version 1. A topology file has the extension `.bin.gz`. A conforming reader and writer follow this document and nothing else.

The key words MUST, MUST NOT, and MAY have their RFC 2119 meaning.

## File

A file is exactly one gzip member (RFC 1952).

A writer MUST emit this 10-byte header: `1f 8b 08 00 00 00 00 00 02 ff`. The flags are zero, MTIME is 0, XFL is 2, and OS is 255. After the header come the raw DEFLATE stream at compression level 9, the CRC-32 (IEEE) of the payload as a little-endian u32, and the payload length modulo 2^32 (ISIZE) as a little-endian u32.

A reader MUST require ID1 = `1f`, ID2 = `8b`, CM = 8, and FLG = 0. A reader MUST accept any MTIME, XFL, and OS. A reader MUST check the CRC-32 and ISIZE, MUST reject any byte after the member, and MUST reject a payload larger than 64 MiB (67108864 bytes).

A reader MUST reject any input that breaks a rule of this document. When an input breaks more than one rule, which error the reader reports is not part of the format. Two conforming readers MAY report different errors for the same input.

The same topology MUST give the same file bytes from the same writer. Different writers MAY produce different compressed bytes. Compare topologies by payload bytes or by the BLAKE3 digest of the payload, never by the compressed bytes.

## Payload

| Field   | Encoding                                       |
|---------|------------------------------------------------|
| magic   | `51 54 4f 50` (`QTOP`)                         |
| version | u8, value `1`                                  |
| kind    | u8, `0` for explicit, `1` for zephyr-removed   |
| body    | defined by the kind                            |

The payload ends exactly after the body. A decoder MUST reject trailing bytes.

### Varint

A varint is an unsigned LEB128 integer. It has at most 10 bytes, and its value is at most 2^64 - 1. A decoder MUST reject a varint that has a final group of `00` after the first byte (a non-minimal encoding).

Zigzag encoding turns a signed 64-bit integer n into the unsigned integer `(n << 1) ^ (n >> 63)`. The right shift copies the sign bit. The encoder writes that unsigned integer as a varint.

### Bounds

A decoder MUST check every count against the number of bytes that remain before it allocates. Each node and each removed node takes at least 1 byte. Each edge and each removed edge takes at least 2 bytes. A decoder MUST use checked arithmetic and MUST reject a value that overflows.

## Explicit topology (kind 0)

This kind keeps registration order, edge orientation, duplicate edges, and self-loops.

1. `n_nodes` as a varint. Then `n_nodes` zigzag deltas. Each delta is the id minus the previous id, and the previous id before the first node is 0. Each id MUST be in `0..=4294967295`.
2. `n_edges` as a varint. Then for each edge, a zigzag of `x - prev_x` and a zigzag of `y - x`. The value `prev_x` is 0 for the first edge and the `x` of the previous edge after that. Both `x` and `y` MUST be in `0..=4294967295`.

The ends of an edge are native ids. They need not appear in the node list. The consumer checks that the edges fit the node list.

## Zephyr-removed topology (kind 1)

This kind stores a Zephyr z(m, t) lattice minus removals.

1. `m` as a varint, in `1..=32`. Then `t` as a varint, in `1..=8`.
2. `n_removed_nodes` as a varint. Then the removed ids in ascending order, with no repeats, as varint deltas. The first delta is the first id. Each later delta is at least 1. Each id MUST be a node of the lattice.
3. `n_removed_edges` as a varint. Then the removed edges `(a, b)` with `a < b`, in ascending order by `(a, b)`, with no repeats. Each edge is a varint `a - prev_a` followed by a varint `b - a`. The value `prev_a` is 0 for the first edge and the `a` of the previous edge after that. The value `b - a` is at least 1. Each edge MUST be a coupler of the lattice, and the removed node list MUST NOT contain either end.

### Zephyr lattice z(m, t)

A node has five coordinates `(u, w, k, j, z)` with `u` in `0..=1`, `w` in `0..=2m`, `k` in `0..t`, `j` in `0..=1`, and `z` in `0..m`. The lattice has `4*t*m*(2m+1)` nodes. The linear id of a node is:

`q = (((u*(2m+1) + w)*t + k)*2 + j)*m + z`

The ids run from 0 to `4*t*m*(2m+1) - 1`. Each id maps to exactly one coordinate tuple, and the reverse also holds.

Two distinct nodes `A = (uA, wA, kA, jA, zA)` and `B = (uB, wB, kB, jB, zB)` form a coupler when the pair, taken in one of the two orders, matches one of these rules. The first node of the pair in the order is `A`.

1. External: `uA = uB`, `wA = wB`, `kA = kB`, `jA = jB`, and `zB = zA + 1`.
2. Odd: `uA = uB`, `wA = wB`, `kA = kB`, `jA = 0`, `jB = 1`, and `zB = zA` or `zB = zA - 1`.
3. Internal: `uA = 0` and `uB = 1`. Define `reach(index, centre, bit)` as true when `index = 2*centre + 1`, or when `bit = 1` and `index = 2*centre + 2`, or when `bit = 0` and `index = 2*centre`. The pair is a coupler when `reach(wA, zB, jB)` and `reach(wB, zA, jA)` are both true. The values `kA` and `kB` are free.

A coupler is an unordered pair. The edge list uses the pair `(a, b)` with `a < b`, where `a` and `b` are linear ids.

Example: z(1, 1) has 12 nodes, numbered 0 to 11, and 22 edges:

`(0,1) (0,6) (0,8) (1,8) (1,10) (2,3) (2,6) (2,7) (2,8) (2,9) (3,8) (3,9) (3,10) (3,11) (4,5) (4,7) (4,9) (5,9) (5,11) (6,7) (8,9) (10,11)`

For orientation only, the Rust function `zephyr_lattice` in `crates/ising-generators/src/ice/topology.rs` follows these rules. This document is the normative definition.

A decoder expands the body as follows:

- The nodes are the lattice nodes in ascending order, without the removed nodes.
- The edges are the lattice edges `(a, b)` with `a < b` in ascending order. Drop every edge that has a removed node as an end. Drop the removed edges.

## Path references

A JSON field that names a topology holds a string path. The path is relative to the directory of the JSON file. Example: `"topology": "../topologies/t-sparse.bin.gz"`.

The file `vectors/topologies/index.json` lists each topology file with these fields: `name`, `file`, `kind`, `nodes`, `edges`, `order_digest`, `payload_blake3`, `provenance`, and `about`. The entry for the live topology also has `sources`, which holds the external sha256.

## Tools

Two tools convert between JSON and blob files. They read each other's output and use the same exit codes.

- Rust: `quip-topology encode IN.json OUT.bin.gz` and `quip-topology decode IN.bin.gz [OUT.json] [--expand]`. The binary is in the `ising-generators` crate and needs the `cli` feature.
- Python: `python -m family_oracle topology encode IN.json OUT.bin.gz` and `python -m family_oracle topology decode IN.bin.gz [OUT.json] [--expand]`, run from the `conformance` directory.

The decode command prints the JSON to standard output when it has no `OUT.json`. The option `--expand` turns a zephyr-removed topology into the explicit form.

The JSON has one of two shapes. All ids and edge ends are plain numbers.

- Explicit: `{"kind": "explicit", "nodes": [ids], "edges": [[x, y]]}`.
- Zephyr-removed: `{"kind": "zephyr-removed", "m": 12, "t": 4, "removed_nodes": [ids], "removed_edges": [[a, b]]}`.

The exit codes are:

| Code | Meaning |
|------|---------|
| 0    | Success |
| 1    | The input file or the JSON is not valid, or the blob breaks a rule of this format |
| 2    | The command line is not valid |

## Library entry points

The Rust crate `ising-generators` has the module `ising_generators::blob` behind the `blob` feature. The feature needs no standard library. The module holds `encode`, `decode`, and the types `Body` and `Topology`. The function `decode` returns a `Body`. The method `Body::topology` expands it into a `Topology`, which holds the node list and the edge list.

The crate `quip-solver-conformance` has two functions at its root. The function `topology` decodes the embedded file that a vectors reference names. The function `t_sparse` returns the shared T_SPARSE topology.
