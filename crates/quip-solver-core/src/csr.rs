//! CSR (compressed sparse row) representation of an Ising graph for GPU upload.
//!
//! Built from the base [`crate::ising::IsingGraph`] by the GPU backends.
//! Couplings are stored once per directed half-edge so local-field walks are
//! O(degree). Value dtype is f32 for kernel dynamics. Milli copies of h/j are
//! kept for consensus scoring.

use crate::ising::IsingGraph;

/// Ising problem with CSR adjacency plus f32 upload buffers.
#[derive(Clone, Debug)]
pub struct CsrGraph {
    /// Linear biases in milli units, for consensus scoring.
    pub h_milli: Vec<i32>,
    /// Couplings in milli units aligned with `edges`, for consensus scoring.
    pub j_milli: Vec<i32>,
    /// Undirected edge list `(u, v)` in received order.
    pub edges: Vec<(usize, usize)>,
    /// CSR row pointers, length `N + 1`.
    pub row_ptr: Vec<i32>,
    /// CSR column indices, length `nnz`.
    pub col_ind: Vec<i32>,
    /// CSR coupling values (symmetric half-edges), length `nnz`.
    pub j_csr: Vec<f32>,
    /// Linear biases as f32 for kernel upload.
    pub h_f32: Vec<f32>,
}

impl CsrGraph {
    /// Build CSR adjacency from the base problem.
    ///
    /// Edges whose endpoints are out of range for `h.len()` are skipped (same
    /// defensive posture as `energy_milli`). Self-loops `(u, u)` are skipped:
    /// a self-loop in a neighbor row would inject a spurious self-force into
    /// that node's local field, while `energy_milli` scores the loop as an
    /// unoptimizable constant. This matches `CpuGraph::from_base` and
    /// `SbGraph::from_base` in quip-miner-cpu. Couplings shorter than edges
    /// are treated as 0 for the missing entries.
    #[must_use]
    pub fn from_base(g: &IsingGraph) -> Self {
        let n = g.num_nodes();
        let mut adj: Vec<Vec<(usize, f32)>> = vec![Vec::new(); n];
        for (k, &(u, v)) in g.edges.iter().enumerate() {
            if u >= n || v >= n || u == v {
                continue;
            }
            let coup = milli_to_f32(g.j_milli.get(k).copied().unwrap_or(0));
            #[expect(
                clippy::indexing_slicing,
                reason = "u and v checked against n; adj length is n"
            )]
            {
                adj[u].push((v, coup));
                adj[v].push((u, coup));
            }
        }
        // Deterministic neighbor order (matches Python GPU CSR builder).
        for nbrs in &mut adj {
            nbrs.sort_by_key(|&(idx, _)| idx);
        }

        let mut row_ptr = vec![0i32; n + 1];
        let mut col_ind = Vec::new();
        let mut j_csr = Vec::new();
        for i in 0..n {
            #[expect(
                clippy::cast_possible_truncation,
                clippy::cast_possible_wrap,
                clippy::indexing_slicing,
                reason = "i in 0..n indexes adj/row_ptr of length n/n+1; nnz and node \
                          indices for device topologies fit i32 CSR encoding"
            )]
            {
                for &(nbr, coup) in &adj[i] {
                    col_ind.push(nbr as i32);
                    j_csr.push(coup);
                }
                row_ptr[i + 1] = col_ind.len() as i32;
            }
        }

        let h_f32: Vec<f32> = g.h_milli.iter().map(|&v| milli_to_f32(v)).collect();
        Self {
            h_milli: g.h_milli.clone(),
            j_milli: g.j_milli.clone(),
            edges: g.edges.clone(),
            row_ptr,
            col_ind,
            j_csr,
            h_f32,
        }
    }

    /// Number of variables (length of `h_milli`).
    #[must_use]
    pub fn num_nodes(&self) -> usize {
        self.h_milli.len()
    }

    /// Number of directed half-edges in the CSR (`col_ind` length).
    #[must_use]
    pub fn nnz(&self) -> usize {
        self.col_ind.len()
    }
}

/// Kernel upload value of one milli coefficient: `v / 1000` rounded to `f64`,
/// then to `f32`. The two roundings match the pre-milli `IsingGraph`, which
/// stored the `f64` quotient that this module narrowed.
#[expect(
    clippy::cast_possible_truncation,
    reason = "kernel upload path intentionally narrows f64 to f32"
)]
fn milli_to_f32(v: i32) -> f32 {
    (f64::from(v) / 1000.0) as f32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn csr_symmetric_two_node() {
        let g = CsrGraph::from_base(&IsingGraph::new(vec![1000, -1000], vec![500], vec![(0, 1)]));
        assert_eq!(g.row_ptr, vec![0, 1, 2]);
        assert_eq!(g.col_ind, vec![1, 0]);
        assert_eq!(g.j_csr, vec![0.5, 0.5]);
        assert_eq!(g.nnz(), 2);
    }

    #[test]
    fn empty_graph() {
        let g = CsrGraph::from_base(&IsingGraph::new(vec![], vec![], vec![]));
        assert_eq!(g.row_ptr, vec![0]);
        assert_eq!(g.nnz(), 0);
    }

    /// A self-loop `(u, u)` must not appear in the CSR adjacency: it should
    /// contribute no entry to `col_ind`/`j_csr` and not inflate `row_ptr`,
    /// matching `CpuGraph::from_base` and `SbGraph::from_base` in
    /// quip-miner-cpu, which both skip `u == v` outright.
    #[test]
    fn csr_drops_self_loops() {
        let g = CsrGraph::from_base(&IsingGraph::new(
            vec![500, -250, 0],
            vec![2000, -1000, 750],
            vec![(0, 0), (0, 1), (1, 2)],
        ));
        assert_eq!(g.row_ptr, vec![0, 1, 3, 4]);
        assert_eq!(g.col_ind, vec![1, 0, 2, 1]);
        assert_eq!(g.j_csr, vec![-1.0, -1.0, 0.75, 0.75]);
        assert_eq!(g.nnz(), 4);
    }

    /// A pure self-loop on a single node produces an empty adjacency row, not
    /// a row containing the node itself.
    #[test]
    fn csr_pure_self_loop_yields_empty_row() {
        let g = CsrGraph::from_base(&IsingGraph::new(vec![0], vec![3000], vec![(0, 0)]));
        assert_eq!(g.row_ptr, vec![0, 0]);
        assert!(g.col_ind.is_empty());
        assert!(g.j_csr.is_empty());
        assert_eq!(g.nnz(), 0);
    }

    /// FNV-1a over `j_csr` then `h_f32`, each `f32` widened to a `u64` of
    /// its bits and hashed little-endian.
    fn upload_digest(g: &CsrGraph) -> u64 {
        let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
        for bits in g
            .j_csr
            .iter()
            .chain(&g.h_f32)
            .map(|x| u64::from(x.to_bits()))
        {
            for byte in bits.to_le_bytes() {
                hash ^= u64::from(byte);
                hash = hash.wrapping_mul(0x0100_0000_01b3);
            }
        }
        hash
    }

    /// `upload_digest` for each entry of `GOLDEN_GRAPHS`, captured from
    /// 0.0.2-rc1, where `IsingGraph` stored `v / 1000.0` floats.
    const PINNED_UPLOADS: [u64; 9] = [
        0x3e56_7ffb_92e0_6498,
        0x3e56_7ffb_92e0_6498,
        0x3e56_7ffb_92e0_6498,
        0x4fc5_7a07_f058_afe9,
        0x4c08_7e86_c6d7_596a,
        0xcc93_a767_3c4a_fd51,
        0xfb0d_dc99_1374_076d,
        0x3e56_7ffb_92e0_6498,
        0xa2da_55ea_77ad_e2b8,
    ];

    #[test]
    fn upload_buffers_are_bit_identical_to_the_f64_graph() {
        for ((section, index), want) in crate::ising::GOLDEN_GRAPHS.into_iter().zip(PINNED_UPLOADS)
        {
            let g = CsrGraph::from_base(&crate::ising::golden_graph(section, index));
            assert_eq!(upload_digest(&g), want, "{section}[{index}]");
        }
    }
}
