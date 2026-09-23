//! Salt leases: the recipe the coordinator and the miner share.
//!
//! Salt `i` is [`salt_with_counter`] of `base_salt` and `salt_start + i`.
//! An all-zero `base_salt` is the coordinator's counter salt. The chain
//! re-derives the same nonce and the same draw in `submit_proof`.

use crate::chacha8::DrawError;
use crate::target::{ProofStats, Target, TargetMiss};

/// The deterministic generator a lease uses to draw each problem.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Generator {
    /// `BLAKE3` nonce, then the `ChaCha8` milli draw.
    Blake3Chacha8V1,
}

/// Why a lease specification was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaseError {
    /// `salt_count` is zero, so the lease names no salts.
    ZeroCount,
    /// `salt_start + salt_count` does not fit in a `u64`.
    CounterOverflow,
    /// A fixed-width field had the wrong number of bytes. `field` names it.
    BadLength {
        /// Name of the field that had the wrong length.
        field: &'static str,
    },
    /// The generator id is not [`Generator::Blake3Chacha8V1`].
    UnknownGenerator(i32),
}

impl std::fmt::Display for LeaseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ZeroCount => write!(f, "lease salt_count is zero"),
            Self::CounterOverflow => write!(f, "lease salt counter overflows u64"),
            Self::BadLength { field } => write!(f, "lease field {field} has the wrong length"),
            Self::UnknownGenerator(id) => write!(f, "unknown generator id {id}"),
        }
    }
}

impl std::error::Error for LeaseError {}

/// One contiguous run of salts a miner may draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaseSpec {
    /// Generator that draws each salt.
    pub generator: Generator,
    /// `last_proof` input to [`crate::derive::derive_nonce`].
    pub last_proof_block_hash: [u8; 32],
    /// Miner account input to [`crate::derive::derive_nonce`].
    pub miner_account: [u8; 32],
    /// Salt template. Bytes 0 to 7 are replaced by the counter.
    pub base_salt: [u8; 32],
    /// Counter of salt index 0.
    pub salt_start: u64,
    /// Number of salts in the lease. Never zero on a value from [`Self::new`].
    pub salt_count: u64,
}

impl LeaseSpec {
    /// Build a lease that names `salt_count` salts beginning at `salt_start`.
    ///
    /// # Errors
    ///
    /// Returns [`LeaseError::ZeroCount`] when `salt_count` is zero, and
    /// [`LeaseError::CounterOverflow`] when `salt_start + salt_count` does not
    /// fit in a `u64`. `salt_start + salt_count == u64::MAX` is accepted. The
    /// last counter is then `u64::MAX - 1`.
    pub const fn new(
        generator: Generator,
        last_proof_block_hash: [u8; 32],
        miner_account: [u8; 32],
        base_salt: [u8; 32],
        salt_start: u64,
        salt_count: u64,
    ) -> Result<Self, LeaseError> {
        if salt_count == 0 {
            return Err(LeaseError::ZeroCount);
        }
        if salt_start.checked_add(salt_count).is_none() {
            return Err(LeaseError::CounterOverflow);
        }
        Ok(Self {
            generator,
            last_proof_block_hash,
            miner_account,
            base_salt,
            salt_start,
            salt_count,
        })
    }

    /// Salt at `index`, or `None` when `index` is outside `0..salt_count`.
    #[must_use]
    pub fn salt(&self, index: u64) -> Option<[u8; 32]> {
        if index >= self.salt_count {
            return None;
        }
        let counter = self.salt_start.checked_add(index)?;
        Some(salt_with_counter(&self.base_salt, counter))
    }

    /// Nonce for the salt at `index`, or `None` when that salt is outside the lease.
    #[must_use]
    pub fn nonce(&self, index: u64) -> Option<[u8; 32]> {
        self.salt(index).map(|salt| {
            crate::derive::derive_nonce(self.last_proof_block_hash, self.miner_account, salt)
        })
    }

    /// Index of `salt` in this lease.
    ///
    /// Bytes 8 to 31 must equal [`Self::base_salt`]. The little-endian counter
    /// in bytes 0 to 7 must lie in `salt_start..salt_start + salt_count`. The
    /// returned index is that counter minus `salt_start`.
    #[must_use]
    pub fn index_of(&self, salt: &[u8; 32]) -> Option<u64> {
        if !salt.iter().skip(8).eq(self.base_salt.iter().skip(8)) {
            return None;
        }
        let mut counter_bytes = [0u8; 8];
        for (slot, byte) in counter_bytes.iter_mut().zip(salt.iter()) {
            *slot = *byte;
        }
        let counter = u64::from_le_bytes(counter_bytes);
        let index = counter.checked_sub(self.salt_start)?;
        if index >= self.salt_count {
            return None;
        }
        Some(index)
    }
}

/// Copy `base` and replace bytes 0 to 7 with the little-endian `counter`.
#[must_use]
pub fn salt_with_counter(base: &[u8; 32], counter: u64) -> [u8; 32] {
    let mut salt = *base;
    for (slot, byte) in salt.iter_mut().zip(counter.to_le_bytes()) {
        *slot = byte;
    }
    salt
}

/// Nodes, edges, and the allowed milli-values [`Self::draw`] selects from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TopologyView {
    /// Number of spins in one solution.
    pub num_nodes: usize,
    /// Coupling endpoints, in the chain's edge order.
    pub edges: Vec<(usize, usize)>,
    /// Field values the draw may select, in milli.
    pub allowed_h_milli: Vec<i32>,
    /// Coupling values the draw may select, in milli.
    pub allowed_j_milli: Vec<i32>,
}

impl TopologyView {
    /// Draw field and coupling milli-values for `nonce`.
    ///
    /// # Errors
    ///
    /// Returns [`DrawError::EmptyAllowedValues`] when nodes are present and
    /// `allowed_h_milli` is empty, or edges are present and `allowed_j_milli`
    /// is empty.
    pub fn draw(&self, nonce: [u8; 32]) -> Result<(Vec<i32>, Vec<i32>), DrawError> {
        crate::chacha8::draw_ising_milli(
            nonce,
            self.num_nodes,
            self.edges.len(),
            &self.allowed_h_milli,
            &self.allowed_j_milli,
        )
    }
}

/// Why a reported lease result was rejected.
#[derive(Debug, PartialEq, Eq)]
pub enum VerifyError {
    /// The salt is not one this lease issues.
    SaltOutsideLease,
    /// The nonce is not [`crate::derive::derive_nonce`] of the lease inputs and this salt.
    NonceMismatch,
    /// Solution `index` does not have one spin per node.
    MalformedSpins {
        /// Position of the solution in the reported set.
        index: usize,
    },
    /// Solution `index` reports an energy other than the drawn problem's.
    EnergyMismatch {
        /// Position of the solution in the reported set.
        index: usize,
    },
    /// The nonce does not draw a problem on this topology.
    Draw(DrawError),
    /// The solutions, as given and in this order, miss the proof target.
    BelowTarget(TargetMiss),
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SaltOutsideLease => write!(f, "salt is outside the lease"),
            Self::NonceMismatch => write!(f, "nonce does not match the derived nonce"),
            Self::MalformedSpins { index } => {
                write!(f, "solution {index} does not have one spin per node")
            }
            Self::EnergyMismatch { index } => {
                write!(f, "solution {index} reports the wrong energy")
            }
            Self::Draw(err) => write!(f, "cannot draw the problem: {err}"),
            Self::BelowTarget(_) => write!(f, "proof set is below the target"),
        }
    }
}

impl std::error::Error for VerifyError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        if let Self::Draw(err) = self {
            Some(err)
        } else {
            None
        }
    }
}

/// A lease result that passed every check, with the stats the chain records.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Verified {
    /// Salt that was checked.
    pub salt: [u8; 32],
    /// Nonce derived for [`Self::salt`].
    pub nonce: [u8; 32],
    /// Stats [`crate::target::validate_proof_set`] computed for the reported solutions.
    pub stats: ProofStats,
}

/// Check one salt's solutions in the order `submit_proof` uses.
///
/// The salt must belong to `spec`, `nonce` must be the derived nonce, every
/// solution must have `topology.num_nodes` spins and the energy of the drawn
/// problem, and the set as given must pass [`crate::target::validate_proof_set`].
///
/// # Errors
///
/// The first failed check: [`VerifyError::SaltOutsideLease`],
/// [`VerifyError::NonceMismatch`], [`VerifyError::Draw`],
/// [`VerifyError::MalformedSpins`], [`VerifyError::EnergyMismatch`], or
/// [`VerifyError::BelowTarget`].
pub fn verify_lease_solutions(
    spec: &LeaseSpec,
    topology: &TopologyView,
    target: &Target,
    salt: &[u8; 32],
    nonce: &[u8; 32],
    solutions: &[(Vec<i8>, i64)],
) -> Result<Verified, VerifyError> {
    if spec.index_of(salt).is_none() {
        return Err(VerifyError::SaltOutsideLease);
    }
    if crate::derive::derive_nonce(spec.last_proof_block_hash, spec.miner_account, *salt) != *nonce
    {
        return Err(VerifyError::NonceMismatch);
    }
    let (h_milli, j_milli) = topology.draw(*nonce).map_err(VerifyError::Draw)?;
    let mut proof = Vec::with_capacity(solutions.len());
    for (index, (spins, reported)) in solutions.iter().enumerate() {
        if spins.len() != topology.num_nodes {
            return Err(VerifyError::MalformedSpins { index });
        }
        if crate::scoring::energy_from_milli(spins, &h_milli, &j_milli, &topology.edges)
            != *reported
        {
            return Err(VerifyError::EnergyMismatch { index });
        }
        proof.push((spins.as_slice(), *reported));
    }
    let stats =
        crate::target::validate_proof_set(&proof, target).map_err(VerifyError::BelowTarget)?;
    Ok(Verified {
        salt: *salt,
        nonce: *nonce,
        stats,
    })
}

#[cfg(feature = "session")]
impl LeaseSpec {
    /// Decode and validate a wire lease.
    ///
    /// # Errors
    /// Rejects unknown generators, incorrect field lengths, and invalid counters.
    pub fn from_proto(g: &quip_proto::v1::IsingProblemGenerator) -> Result<Self, LeaseError> {
        if g.algorithm != quip_proto::v1::GeneratorAlgorithm::Blake3Chacha8V1 as i32 {
            return Err(LeaseError::UnknownGenerator(g.algorithm));
        }
        let fixed = |bytes: &[u8], field| {
            bytes
                .try_into()
                .map_err(|_| LeaseError::BadLength { field })
        };
        Self::new(
            Generator::Blake3Chacha8V1,
            fixed(&g.last_proof_block_hash, "last_proof_block_hash")?,
            fixed(&g.miner_account, "miner_account")?,
            fixed(&g.base_salt, "base_salt")?,
            g.salt_start,
            g.salt_count,
        )
    }
}

/// Why a wire topology cannot map node identifiers to positions.
#[cfg(feature = "session")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TopologyError {
    /// A node identifier occurs twice in registration order.
    DuplicateNode(u32),
    /// An edge names a node outside the registration list.
    UnknownNode(u32),
    /// The endpoint arrays have different lengths.
    UnequalEdgeLists,
}

#[cfg(feature = "session")]
impl TopologyView {
    /// Map registered node identifiers to dense positions without changing order.
    ///
    /// # Errors
    /// Rejects duplicate nodes, unknown endpoints, and unequal endpoint arrays.
    pub fn from_proto(t: &quip_proto::v1::Topology) -> Result<Self, TopologyError> {
        let mut positions = std::collections::HashMap::with_capacity(t.nodes.len());
        for (index, &node) in t.nodes.iter().enumerate() {
            if positions.insert(node, index).is_some() {
                return Err(TopologyError::DuplicateNode(node));
            }
        }
        let mut edges = Vec::new();
        if let Some(e) = &t.edges {
            if e.u.len() != e.v.len() {
                return Err(TopologyError::UnequalEdgeLists);
            }
            for (&u, &v) in e.u.iter().zip(&e.v) {
                let u = *positions.get(&u).ok_or(TopologyError::UnknownNode(u))?;
                let v = *positions.get(&v).ok_or(TopologyError::UnknownNode(v))?;
                edges.push((u, v));
            }
        }
        Ok(Self {
            num_nodes: t.nodes.len(),
            edges,
            allowed_h_milli: t.allowed_h_milli.clone(),
            allowed_j_milli: t.allowed_j_milli.clone(),
        })
    }
}

/// Decode a wire result and verify its lease, nonce, energies, and proof set.
///
/// # Errors
/// Returns the first malformed wire field or failed lease verification check.
#[cfg(feature = "session")]
pub fn verify_lease_result(
    lease: &quip_proto::v1::IsingProblemGenerator,
    topology: &TopologyView,
    target: &Target,
    result: &quip_proto::v1::Result,
) -> Result<Verified, VerifyError> {
    let lease = LeaseSpec::from_proto(lease).map_err(|_| VerifyError::SaltOutsideLease)?;
    let salt = result
        .salt
        .as_slice()
        .try_into()
        .map_err(|_| VerifyError::SaltOutsideLease)?;
    if lease.index_of(&salt).is_none() {
        return Err(VerifyError::SaltOutsideLease);
    }
    let nonce = result
        .nonce
        .as_slice()
        .try_into()
        .map_err(|_| VerifyError::NonceMismatch)?;
    if crate::derive::derive_nonce(lease.last_proof_block_hash, lease.miner_account, salt) != nonce
    {
        return Err(VerifyError::NonceMismatch);
    }
    let mut solutions = Vec::with_capacity(result.solutions.len());
    for (index, solution) in result.solutions.iter().enumerate() {
        let spins = crate::wire::decode_spins_packed(&solution.spins, topology.num_nodes)
            .map_err(|_| VerifyError::MalformedSpins { index })?;
        solutions.push((spins, solution.energy_milli));
    }
    verify_lease_solutions(&lease, topology, target, &salt, &nonce, &solutions)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec(start: u64, count: u64) -> LeaseSpec {
        let mut base = [0u8; 32];
        base[8..].fill(0x33);
        LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            [0x11; 32],
            [0x22; 32],
            base,
            start,
            count,
        )
        .unwrap()
    }

    #[test]
    fn a_zero_base_salt_reproduces_the_coordinator_counter_salt() {
        let s =
            LeaseSpec::new(Generator::Blake3Chacha8V1, [0; 32], [0; 32], [0; 32], 7, 3).unwrap();
        let mut expected = [0u8; 32];
        expected[..8].copy_from_slice(&9u64.to_le_bytes());
        assert_eq!(s.salt(2), Some(expected));
        assert_eq!(s.salt(3), None);
    }

    #[test]
    fn the_counter_overwrites_bytes_zero_to_seven_only() {
        let s = spec(5, 4);
        let salt = s.salt(1).unwrap();
        assert_eq!(&salt[..8], &6u64.to_le_bytes());
        assert!(salt[8..].iter().all(|&b| b == 0x33));
    }

    #[test]
    fn zero_count_and_overflow_are_rejected() {
        assert_eq!(
            LeaseSpec::new(Generator::Blake3Chacha8V1, [0; 32], [0; 32], [0; 32], 0, 0).err(),
            Some(LeaseError::ZeroCount)
        );
        assert_eq!(
            LeaseSpec::new(
                Generator::Blake3Chacha8V1,
                [0; 32],
                [0; 32],
                [0; 32],
                u64::MAX,
                2
            )
            .err(),
            Some(LeaseError::CounterOverflow)
        );
        // start + count == u64::MAX is allowed: the last counter is u64::MAX - 1.
        assert!(LeaseSpec::new(
            Generator::Blake3Chacha8V1,
            [0; 32],
            [0; 32],
            [0; 32],
            u64::MAX - 2,
            2
        )
        .is_ok());
    }

    #[test]
    fn index_of_accepts_only_salts_inside_the_lease() {
        let s = spec(5, 4);
        assert_eq!(s.index_of(&s.salt(3).unwrap()), Some(3));
        assert_eq!(s.index_of(&salt_with_counter(&s.base_salt, 9)), None); // past the end
        assert_eq!(s.index_of(&salt_with_counter(&s.base_salt, 4)), None); // before the start
        let mut foreign = s.salt(0).unwrap();
        foreign[20] ^= 1;
        assert_eq!(s.index_of(&foreign), None);
    }

    #[test]
    fn nonce_matches_derive_nonce_on_the_salt() {
        let s = spec(5, 4);
        let salt = s.salt(2).unwrap();
        assert_eq!(
            s.nonce(2),
            Some(crate::derive::derive_nonce([0x11; 32], [0x22; 32], salt))
        );
    }

    fn tiny_topology() -> TopologyView {
        TopologyView {
            num_nodes: 4,
            edges: vec![(0, 1), (1, 2), (2, 3), (3, 0)],
            allowed_h_milli: vec![-1000, 0, 1000],
            allowed_j_milli: vec![-1000, 1000],
        }
    }

    fn scored(topo: &TopologyView, nonce: [u8; 32], spins: Vec<i8>) -> (Vec<i8>, i64) {
        let (h, j) = topo.draw(nonce).unwrap();
        let e = crate::scoring::energy_from_milli(&spins, &h, &j, &topo.edges);
        (spins, e)
    }

    const EASY: Target = Target {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        min_diversity_milli: 0,
        max_proof_solutions: 32,
    };

    #[test]
    fn honest_solutions_verify() {
        let s = spec(5, 4);
        let topo = tiny_topology();
        let (salt, nonce) = (s.salt(1).unwrap(), s.nonce(1).unwrap());
        let sols = vec![scored(&topo, nonce, vec![1, -1, 1, -1])];
        let v = verify_lease_solutions(&s, &topo, &EASY, &salt, &nonce, &sols).unwrap();
        assert_eq!(v.salt, salt);
        assert_eq!(v.stats.valid_solution_count, 1);
    }

    #[test]
    fn empty_required_values_report_draw_error() {
        let s = spec(5, 4);
        let mut topo = tiny_topology();
        topo.allowed_h_milli.clear();
        assert!(matches!(
            verify_lease_solutions(
                &s,
                &topo,
                &EASY,
                &s.salt(0).unwrap(),
                &s.nonce(0).unwrap(),
                &[]
            ),
            Err(VerifyError::Draw(_))
        ));
    }

    #[cfg(feature = "session")]
    #[test]
    fn wire_membership_and_nonce_precede_malformed_spins() {
        let s = spec(5, 4);
        let generator = quip_proto::v1::IsingProblemGenerator {
            algorithm: 1,
            last_proof_block_hash: s.last_proof_block_hash.to_vec(),
            miner_account: s.miner_account.to_vec(),
            base_salt: s.base_salt.to_vec(),
            salt_start: 5,
            salt_count: 4,
            ..Default::default()
        };
        let mut result = quip_proto::v1::Result {
            salt: salt_with_counter(&s.base_salt, 100).to_vec(),
            nonce: vec![0; 32],
            solutions: vec![quip_proto::v1::Solution {
                spins: vec![],
                energy_milli: 0,
            }],
            ..Default::default()
        };
        assert_eq!(
            verify_lease_result(&generator, &tiny_topology(), &EASY, &result).err(),
            Some(VerifyError::SaltOutsideLease)
        );
        result.salt = s.salt(0).unwrap().to_vec();
        assert_eq!(
            verify_lease_result(&generator, &tiny_topology(), &EASY, &result).err(),
            Some(VerifyError::NonceMismatch)
        );
    }

    #[test]
    fn each_failure_has_its_own_error() {
        let s = spec(5, 4);
        let topo = tiny_topology();
        let (salt, nonce) = (s.salt(1).unwrap(), s.nonce(1).unwrap());
        let good = vec![scored(&topo, nonce, vec![1, -1, 1, -1])];

        let outside = salt_with_counter(&s.base_salt, 100);
        assert_eq!(
            verify_lease_solutions(&s, &topo, &EASY, &outside, &nonce, &good).err(),
            Some(VerifyError::SaltOutsideLease)
        );
        assert_eq!(
            verify_lease_solutions(&s, &topo, &EASY, &salt, &[0; 32], &good).err(),
            Some(VerifyError::NonceMismatch)
        );
        let mut lying = good.clone();
        lying.first_mut().unwrap().1 -= 1;
        assert_eq!(
            verify_lease_solutions(&s, &topo, &EASY, &salt, &nonce, &lying).err(),
            Some(VerifyError::EnergyMismatch { index: 0 })
        );
        let short = vec![(vec![1, -1], good.first().unwrap().1)];
        assert_eq!(
            verify_lease_solutions(&s, &topo, &EASY, &salt, &nonce, &short).err(),
            Some(VerifyError::MalformedSpins { index: 0 })
        );
        let hard = Target {
            max_energy_milli: i64::MIN,
            ..EASY
        };
        assert_eq!(
            verify_lease_solutions(&s, &topo, &hard, &salt, &nonce, &good).err(),
            Some(VerifyError::BelowTarget(TargetMiss::InsufficientEnergy))
        );
    }
}
