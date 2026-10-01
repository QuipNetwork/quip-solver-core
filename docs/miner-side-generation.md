# Drawing lease problems on miner hardware

A lease problem depends only on its 32-byte nonce and the lease topology.
Any device that reproduces the draw bit for bit can make the problem itself.
The default draw in `quip-protocol` is the reference and a fast CPU path.
A miner with a GPU or other accelerator can replace it with a device draw.

The host redraws every candidate winner on the CPU and rescores each read.
A device draw that differs from the reference is a device fault and ends the run.
It never produces an invalid proof.

## The default path

Use these functions unless your hardware has a faster way:

| Function | Use |
| --- | --- |
| `Lease::nonce(i)` | Nonce of salt `i`. The lease caches the BLAKE3 state for the bytes that all its salts share. |
| `TopologyView::draw(nonce)` | Field and coupling values in milli. |
| `chacha8::draw_ising(nonce, n_nodes, n_edges, h_table, j_table)` | The same draw, written straight into any `Copy` type. |
| `chacha8::draw_into(nonce, first_word, allowed, out)` | Any range of the draw stream. Use it to split one problem across threads. |

`draw_ising` takes tables that have one entry for each allowed milli value, in the same order.
Convert the allowed values to your coefficient type once per lease, then draw into that type.
The draw then makes no conversion for each value.

On `x86_64`, the draw checks for AVX2 at run time and computes eight ChaCha8 blocks in each pass.
Other targets compute four blocks in each pass.
These times are for one problem on a Ryzen 9 5950X with 4577 nodes and 41,515 edges:

| Step | Time |
| --- | --- |
| `Lease::nonce` | 0.07 µs |
| `draw_ising` with AVX2 | 41 µs |
| Lease expansion into a solver graph, from nonce to `IsingGraph<Fixed<i8, 1>>` | 41 µs to 44 µs |

`IsingGraph::edges` is an `Arc` that all graphs in a lease share.
A new graph does not copy the edge list.

## Split one problem across CPU threads

The draw reads one ChaCha8 keystream.
Fields take keystream words `0..n_nodes`.
Couplings take words `n_nodes..n_nodes + n_edges`.
Each word depends only on the nonce and its position, so `draw_into` can start at any word.
Ranges drawn on separate threads join into the same result as one sequential draw.

```rust
use quip_protocol::chacha8::draw_into;

let n_nodes = h.len();
let (h_left, h_right) = h.split_at_mut(n_nodes / 2);
let (j_left, j_right) = j.split_at_mut(j.len() / 2);
let h_split = h_left.len() as u64;
let j_split = (n_nodes + j_left.len()) as u64;
std::thread::scope(|s| {
    let parts = [
        s.spawn(|| draw_into(nonce, 0, &h_table, h_left)),
        s.spawn(|| draw_into(nonce, h_split, &h_table, h_right)),
        s.spawn(|| draw_into(nonce, n_nodes as u64, &j_table, j_left)),
        s.spawn(|| draw_into(nonce, j_split, &j_table, j_right)),
    ];
    parts.into_iter().try_for_each(|part| part.join().expect("draw thread"))
})?;
```

Each call returns `Err(DrawError::EmptyAllowedValues)` when its output range is not empty and its table is empty.

## Draw on the GPU

A device draw removes the host draw and the copy of each problem to the device.
The device keeps the topology and the converted allowed tables for the whole lease.
For each salt, the host sends only the 32-byte nonce.

### The algorithm

This is the complete draw. The reference is `quip_protocol::chacha8`.

1. Read the nonce as eight little-endian `u32` key words, `k0` to `k7`.
2. Keystream block `b` starts from this 16-word state:
   - Words 0 to 3: `0x61707865`, `0x3320646e`, `0x79622d32`, `0x6b206574`.
   - Words 4 to 11: `k0` to `k7`.
   - Word 12: the low 32 bits of `b`. Word 13: the high 32 bits of `b`.
   - Words 14 and 15: zero.
3. Apply 4 ChaCha double rounds, which is 8 rounds.
   Each double round is the four column quarter-rounds, then the four diagonal quarter-rounds.
4. Add the starting state to the result, word by word, with wrapping addition.
   The 16 words are block `b` of the keystream.
5. Keystream word `w` is word `w % 16` of block `w / 16`.
6. Field `i` is `allowed_h[word(i) % len(allowed_h)]`.
   Coupling `e` is `allowed_j[word(n_nodes + e) % len(allowed_j)]`.
   The remainder is the unsigned 32-bit remainder.

Fields use node order and couplings use edge order, as `TopologyView` holds them.
The order that a topology hash sorts into is not the draw order.

### Kernel layout

Give each thread one keystream block.
Thread `b` computes block `b` and writes up to 16 values, at positions `16 * b` to `16 * b + 15`.
Positions below `n_nodes` are fields.
Positions from `n_nodes` are couplings at index `position - n_nodes`.
One block can hold the last fields and the first couplings.

```c
__global__ void draw(const uint32_t key[8], uint32_t n_nodes, uint32_t n_edges,
                     const float *allowed_h, uint32_t len_h,
                     const float *allowed_j, uint32_t len_j,
                     float *h, float *j) {
    uint64_t b = blockIdx.x * (uint64_t)blockDim.x + threadIdx.x;
    uint64_t first = b * 16;
    if (first >= (uint64_t)n_nodes + n_edges) return;
    uint32_t words[16];
    chacha8_block(key, b, words);  /* steps 2 to 4 */
    for (int i = 0; i < 16; i++) {
        uint64_t p = first + i;
        if (p < n_nodes) h[p] = allowed_h[words[i] % len_h];
        else if (p - n_nodes < n_edges) j[p - n_nodes] = allowed_j[words[i] % len_j];
    }
}
```

Keep `allowed_h` and `allowed_j` in constant or shared memory.
They hold two or three values for the current chain topology.
A small constant divisor lets the compiler replace `%` with a multiply.

### Nonces

Compute nonces on the host with `Lease::nonce(i)` and send them to the device.
A nonce costs about 0.07 µs on the CPU, which is small next to the draw.
The nonce is `BLAKE3(last_proof_block_hash || miner_account || salt)`.
The first 64 bytes are the same for every salt in a lease.
A device nonce needs one BLAKE3 compression for each salt, after one compression for each lease.

### Connect the device draw to the session

Return `true` from `Sampler::generates_locally()` and override `Sampler::sample_lease`.
The session then gives the lease to your sampler, and your sampler draws and samples each salt.
Call `LeaseSink::push(i, reads)` once for each salt.
The [specification](../SPEC.md#optional-local-generation) describes the full contract.

### Test the device draw

Compare the device draw with the reference before you run it against a coordinator:

- Run the `chacha8`, `ising`, and `derive_nonce` sections of `quip_solver_conformance::GOLDEN_VECTORS`.
- Compare full draws with `chacha8::draw_ising` for many random nonces.
  Use topologies where `n_nodes` is not divisible by 16, so that one block holds both fields and couplings.
- Use `chacha8::draw_into` to find the first keystream word that differs.

These are the frequent errors:

| Error | Result |
| --- | --- |
| Couplings start at keystream word 0 | Every coupling is wrong. Couplings start at word `n_nodes`. |
| The block counter is 32 bits | Draws differ after block 2^32. Word 13 holds the high half of the counter. |
| Big-endian key words | Every value is wrong. Key words are little-endian. |
| Signed remainder | Values differ for words from 2^31. Use an unsigned remainder. |
| Sorted node or edge order | Values go to the wrong nodes. Use the order in `TopologyView`. |
