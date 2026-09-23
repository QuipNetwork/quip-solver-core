//! The chain's checks on the solutions a proof carries.
//!
//! [`validate_proof_set`] ports `validate_proof` and the three difficulty gates
//! of `submit_proof` in the `quantum-pow` pallet. [`meets_target`] chooses the
//! set a miner reports for one salt, then runs those checks on it.

use crate::diversity::{diversity_milli, select_diverse};

/// The difficulty a proof must meet, from `SetTarget`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    /// A solution counts only when its energy is strictly below this.
    pub max_energy_milli: i64,
    /// Fewest valid solutions a proof may carry.
    pub min_solutions: u32,
    /// Least diversity, in milli, over the selected solutions.
    pub min_diversity_milli: u32,
    /// Most solutions one proof may carry (the runtime's `MaxSolutions`).
    pub max_proof_solutions: u32,
}

/// The first check a proof set failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TargetMiss {
    /// No solution is below the energy ceiling.
    InsufficientEnergy,
    /// Fewer valid solutions than `min_solutions`.
    InsufficientSolutions,
    /// Diversity of the selected solutions is below `min_diversity_milli`.
    InsufficientDiversity,
    /// More solutions than one proof may carry.
    TooManySolutions,
}

/// What the chain computes for a proof set that passes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProofStats {
    /// Lowest energy among valid solutions; this ranks the proof for the block.
    pub best_energy_milli: i64,
    /// Diversity of the solutions `select_diverse` picked.
    pub diversity_milli: u32,
    /// Solutions below the energy ceiling.
    pub valid_solution_count: u32,
}

/// The solutions to report for one salt, as indices into the reads.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProofSet {
    /// Indices into the `reads` given to [`meets_target`], in proof order.
    pub indices: Vec<usize>,
    /// The chain's result for this set.
    pub stats: ProofStats,
}

/// Run the chain's proof checks on `set`, in the order given.
///
/// # Errors
/// The first failed check, in the pallet's order: size, energy, count, diversity.
pub fn validate_proof_set(set: &[(&[i8], i64)], target: &Target) -> Result<ProofStats, TargetMiss> {
    let max_proof_solutions = usize::try_from(target.max_proof_solutions).unwrap_or(usize::MAX);
    if set.len() > max_proof_solutions {
        return Err(TargetMiss::TooManySolutions);
    }
    let valid: Vec<(&[i8], i64)> = set
        .iter()
        .copied()
        .filter(|&(_, energy)| energy < target.max_energy_milli)
        .collect();
    let Some(best_energy_milli) = valid.iter().map(|&(_, energy)| energy).min() else {
        return Err(TargetMiss::InsufficientEnergy);
    };
    let spins: Vec<&[i8]> = valid.iter().map(|&(spins, _)| spins).collect();
    let min_solutions = usize::try_from(target.min_solutions.max(1)).unwrap_or(usize::MAX);
    let target_count = spins.len().min(min_solutions);
    let picked: Vec<&[i8]> = select_diverse(&spins, target_count)
        .into_iter()
        .filter_map(|index| spins.get(index).copied())
        .collect();
    let stats = ProofStats {
        best_energy_milli,
        diversity_milli: diversity_milli(&picked),
        valid_solution_count: u32::try_from(valid.len()).unwrap_or(u32::MAX),
    };
    if stats.valid_solution_count < target.min_solutions {
        return Err(TargetMiss::InsufficientSolutions);
    }
    if stats.diversity_milli < target.min_diversity_milli {
        return Err(TargetMiss::InsufficientDiversity);
    }
    Ok(stats)
}

/// Choose the proof set for one salt's reads, then check it.
///
/// Keeps the reads below the ceiling, sorts them by energy (stable, so ties
/// keep read order), keeps the first `max_proof_solutions`, and runs
/// [`validate_proof_set`] on exactly that set. The lowest-energy read is always
/// in the set, because it ranks the proof for the block.
///
/// # Errors
/// The first check the chosen set failed.
pub fn meets_target(reads: &[(&[i8], i64)], target: &Target) -> Result<ProofSet, TargetMiss> {
    let mut indices: Vec<usize> = reads
        .iter()
        .enumerate()
        .filter(|(_, &(_, energy))| energy < target.max_energy_milli)
        .map(|(index, _)| index)
        .collect();
    indices.sort_by_key(|&index| reads.get(index).map_or(i64::MAX, |&(_, energy)| energy));
    let max_proof_solutions = usize::try_from(target.max_proof_solutions).unwrap_or(usize::MAX);
    indices.truncate(max_proof_solutions);
    let set: Vec<(&[i8], i64)> = indices
        .iter()
        .filter_map(|&index| reads.get(index).copied())
        .collect();
    let stats = validate_proof_set(&set, target)?;
    Ok(ProofSet { indices, stats })
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Target = Target {
        max_energy_milli: -100,
        min_solutions: 2,
        min_diversity_milli: 200,
        max_proof_solutions: 3,
    };

    #[test]
    fn energy_equal_to_the_ceiling_does_not_count() {
        let a: &[i8] = &[1, 1, 1, 1];
        assert_eq!(
            validate_proof_set(&[(a, -100)], &T),
            Err(TargetMiss::InsufficientEnergy)
        );
    }

    #[test]
    fn too_few_valid_reads_fail_on_count() {
        let a: &[i8] = &[1, 1, 1, 1];
        let b: &[i8] = &[1, 1, -1, -1];
        assert_eq!(
            validate_proof_set(&[(a, -200), (b, -50)], &T),
            Err(TargetMiss::InsufficientSolutions)
        );
    }

    #[test]
    fn diversity_exactly_at_the_threshold_passes() {
        // Width 5, distance 1 → 1000 / 5 = 200 milli, equal to the threshold.
        let a: &[i8] = &[1, 1, 1, 1, 1];
        let b: &[i8] = &[1, 1, 1, 1, -1];
        let stats = validate_proof_set(&[(a, -300), (b, -200)], &T).unwrap();
        assert_eq!(stats.best_energy_milli, -300);
        assert_eq!(stats.diversity_milli, 200);
        assert_eq!(stats.valid_solution_count, 2);
    }

    #[test]
    fn a_set_larger_than_the_proof_limit_is_rejected() {
        let a: &[i8] = &[1];
        let set = [(a, -200); 4];
        assert_eq!(
            validate_proof_set(&set, &T),
            Err(TargetMiss::TooManySolutions)
        );
    }

    #[test]
    fn meets_target_keeps_the_lowest_energies_in_stable_order() {
        let r0: &[i8] = &[1, 1, 1, 1, 1];
        let r1: &[i8] = &[1, 1, 1, -1, -1];
        let r2: &[i8] = &[1, -1, 1, 1, -1];
        let r3: &[i8] = &[-1, 1, 1, 1, 1];
        let r4: &[i8] = &[1, 1, 1, 1, 1];
        // r4 is above the ceiling. r1 and r2 tie at -300 and keep read order.
        let reads = [(r0, -200), (r1, -300), (r2, -300), (r3, -250), (r4, -50)];
        let set = meets_target(&reads, &T).unwrap();
        assert_eq!(set.indices, vec![1, 2, 3]);
        assert_eq!(set.stats.best_energy_milli, -300);
    }

    #[test]
    fn meets_target_reports_the_first_failed_gate() {
        let a: &[i8] = &[1, 1];
        assert_eq!(
            meets_target(&[(a, 0)], &T).err(),
            Some(TargetMiss::InsufficientEnergy)
        );
    }
}
