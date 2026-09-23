//! Integer ports of the chain's diversity rules.
//!
//! The validator selects and scores proof solutions with integer arithmetic in
//! `quantum-validation` (`select_diverse`, `calculate_diversity`). A miner that
//! decides locally whether a salt wins must reach the same integers, so these
//! are line-for-line ports, pinned by `golden_target.json`. The `f64`
//! [`crate::scoring::set_diversity`] is a different, display-only measure.

use crate::scoring::hamming_flip_invariant;

/// Mean pairwise flip-invariant Hamming distance, in milli, rounded half up.
///
/// `0` for fewer than two solutions, a zero width, or a ragged set. The chain
/// rejects a ragged set outright. No proof set reaches here ragged, because
/// every read of one problem has the problem's width.
#[must_use]
pub fn diversity_milli(solutions: &[&[i8]]) -> u32 {
    let Some(first) = solutions.first() else {
        return 0;
    };
    let width = first.len();
    if solutions.len() < 2 || width == 0 || solutions.iter().any(|s| s.len() != width) {
        return 0;
    }
    let mut total: u64 = 0;
    let mut pairs: u64 = 0;
    for (i, a) in solutions.iter().enumerate() {
        for b in solutions.iter().skip(i + 1) {
            total += u64::from(hamming_flip_invariant(a, b));
            pairs += 1;
        }
    }
    let numerator = total * 1000;
    let denominator = pairs * u64::try_from(width).unwrap_or(u64::MAX);
    let rounded = (numerator + denominator / 2) / denominator;
    u32::try_from(rounded).unwrap_or(u32::MAX)
}

/// Greedy farthest-point selection of `target_count` solutions.
///
/// Starts from the most distant pair (lowest indices on a tie), then adds the
/// candidate whose minimum distance to the selected set is largest (lowest
/// index on a tie). Indices refer to `solutions`.
#[must_use]
pub fn select_diverse(solutions: &[&[i8]], target_count: usize) -> Vec<usize> {
    let n = solutions.len();
    if n == 0 || target_count == 0 {
        return Vec::new();
    }
    if n <= target_count {
        return (0..n).collect();
    }
    if target_count == 1 {
        return vec![0];
    }
    let dist = pairwise_distances(solutions);
    let (first, second) = most_distant_pair(&dist);
    let mut selected = vec![first, second];
    let mut chosen = vec![false; n];
    mark(&mut chosen, first);
    mark(&mut chosen, second);
    let mut min_dist: Vec<u32> = (0..n)
        .map(|candidate| cell(&dist, candidate, first).min(cell(&dist, candidate, second)))
        .collect();
    while selected.len() < target_count {
        let mut pick: Option<usize> = None;
        let mut pick_dist = 0u32;
        for (candidate, &is_chosen) in chosen.iter().enumerate() {
            if is_chosen {
                continue;
            }
            let distance = min_dist.get(candidate).copied().unwrap_or(0);
            if pick.is_none() || distance > pick_dist {
                pick = Some(candidate);
                pick_dist = distance;
            }
        }
        let Some(idx) = pick else { break };
        selected.push(idx);
        mark(&mut chosen, idx);
        for (candidate, &is_chosen) in chosen.iter().enumerate() {
            if !is_chosen {
                let distance = cell(&dist, candidate, idx);
                if let Some(slot) = min_dist.get_mut(candidate) {
                    *slot = (*slot).min(distance);
                }
            }
        }
    }
    selected
}

fn pairwise_distances(solutions: &[&[i8]]) -> Vec<Vec<u32>> {
    let n = solutions.len();
    let mut dist = vec![vec![0u32; n]; n];
    for (i, a) in solutions.iter().enumerate() {
        for (j, b) in solutions.iter().enumerate().skip(i + 1) {
            let distance = hamming_flip_invariant(a, b);
            set_cell(&mut dist, i, j, distance);
            set_cell(&mut dist, j, i, distance);
        }
    }
    dist
}

/// Lowest index pair whose distance is strictly greater than every earlier pair.
fn most_distant_pair(dist: &[Vec<u32>]) -> (usize, usize) {
    let mut best_i = 0;
    let mut best_j = 1;
    let mut best = cell(dist, 0, 1);
    for (i, row) in dist.iter().enumerate() {
        for (j, &distance) in row.iter().enumerate().skip(i + 1) {
            if distance > best {
                best_i = i;
                best_j = j;
                best = distance;
            }
        }
    }
    (best_i, best_j)
}

fn cell(dist: &[Vec<u32>], i: usize, j: usize) -> u32 {
    dist.get(i).and_then(|row| row.get(j)).copied().unwrap_or(0)
}

fn set_cell(dist: &mut [Vec<u32>], i: usize, j: usize, value: u32) {
    if let Some(slot) = dist.get_mut(i).and_then(|row| row.get_mut(j)) {
        *slot = value;
    }
}

fn mark(chosen: &mut [bool], index: usize) {
    if let Some(flag) = chosen.get_mut(index) {
        *flag = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fewer_than_two_solutions_have_zero_diversity() {
        assert_eq!(diversity_milli(&[]), 0);
        assert_eq!(diversity_milli(&[&[1, -1][..]]), 0);
    }

    #[test]
    #[expect(
        clippy::many_single_char_names,
        reason = "each solution in this hand-checked case is named in the brief"
    )]
    fn diversity_rounds_half_up_in_milli() {
        // Three solutions of width 2. Pair distances (flip-invariant): 1, 1, 0.
        // Mean normalized distance = (1 + 1 + 0) / (3 * 2) = 0.3333… → 333.
        let a: &[i8] = &[1, 1];
        let b: &[i8] = &[1, -1];
        let c: &[i8] = &[-1, 1];
        assert_eq!(diversity_milli(&[a, b, c]), 333);
        // Width 8, one pair at distance 1: 1000 / 8 = 125 exactly.
        let d: &[i8] = &[1; 8];
        let e: &[i8] = &[1, 1, 1, 1, 1, 1, 1, -1];
        assert_eq!(diversity_milli(&[d, e]), 125);
        // Width 16, one pair at distance 1: 62.5 → 63 (half rounds up).
        let f: &[i8] = &[1; 16];
        let mut g = [1i8; 16];
        g[0] = -1;
        assert_eq!(diversity_milli(&[f, &g]), 63);
    }

    #[test]
    fn ragged_or_empty_width_sets_score_zero() {
        assert_eq!(diversity_milli(&[&[1, -1][..], &[1][..]]), 0);
        assert_eq!(diversity_milli(&[&[][..], &[][..]]), 0);
    }

    #[test]
    fn select_diverse_returns_everything_when_the_set_is_small() {
        let s: [&[i8]; 2] = [&[1, 1], &[1, -1]];
        assert_eq!(select_diverse(&s, 2), vec![0, 1]);
        assert_eq!(select_diverse(&s, 5), vec![0, 1]);
        assert_eq!(select_diverse(&s, 0), Vec::<usize>::new());
    }

    #[test]
    fn select_diverse_takes_the_first_read_for_a_target_of_one() {
        let s: [&[i8]; 3] = [&[1, 1], &[1, -1], &[-1, 1]];
        assert_eq!(select_diverse(&s, 1), vec![0]);
    }

    #[test]
    fn select_diverse_starts_from_the_most_distant_pair_lowest_index_on_ties() {
        // Width 4. Distances: d(0,1)=1, d(0,2)=2, d(1,2)=1, d(0,3)=2, d(1,3)=1, d(2,3)=0.
        // Most distant pair: (0,2) first at distance 2; (0,3) ties and does not replace it.
        let s: [&[i8]; 4] = [
            &[1, 1, 1, 1],
            &[1, 1, 1, -1],
            &[1, 1, -1, -1],
            &[-1, -1, 1, 1],
        ];
        assert_eq!(select_diverse(&s, 2), vec![0, 2]);
        // Third pick: min distance to {0,2} is 1 for read 1 and 0 for read 3 → read 1.
        assert_eq!(select_diverse(&s, 3), vec![0, 2, 1]);
    }
}
