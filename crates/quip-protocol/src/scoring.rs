//! Energy and diversity scoring in integer-milli arithmetic.
//!
//! Wire coefficients travel as `i32` milli (`wire::decode_i32_le`).
//! [`energy_from_milli`] scores them directly. [`energy_milli`] keeps the
//! historical `f64` signature for the Python, WASM, and C bindings, but recovers
//! each coefficient's exact milli value and accumulates in integers, so both
//! agree with the integer-milli re-score the coordinator and chain perform
//! (`quantum_validation`). A miner-reported `energy_milli` is still never the
//! basis for the accept decision. The coordinator always re-scores.

/// The value [`energy_milli`] returns when a coefficient it reads is not finite.
///
/// Deliberately distinct from `i64::MAX` / `i64::MIN`, which mean "finite but out
/// of `i64` range", so a caller can tell a malformed problem from a saturated
/// score. Pinned by the `sentinel` section of `conformance/golden_vectors.json`
/// so every language binding reports the same number.
pub const ENERGY_MILLI_NON_FINITE: i64 = 1 << 62;

/// Map a spin to ±1 (`s > 0` → `+1`), matching the wire's sign convention.
fn sign(s: i8) -> i64 {
    if s > 0 {
        1
    } else {
        -1
    }
}

/// Recover the exact integer-milli value of a wire coefficient.
///
/// Every coefficient reaching this crate was produced as `v as f64 / 1000.0` from
/// an `i32` milli value `v`. That division rounds to nearest, so the stored `h`
/// differs from the real `v/1000` by at most half an ulp; scaling back by 1000
/// adds at most half an ulp more. The combined relative error stays below
/// `2^-52`, so `|h * 1000.0 - v| < 0.5` for every `|v|` up to roughly `2e15` —
/// a million times the `i32` range the wire can actually carry. Rounding to
/// nearest therefore reproduces `v` exactly for every representable coefficient.
///
/// Truncation does not. `v/1000.0` is exact only when `v` is a multiple of 125,
/// so an ordinary 0.1 field is stored slightly low or high; ten of them summed in
/// `f64` reach 0.999999999999999889, and truncating that yields 999 milli where
/// the coordinator computes 1000. Recovering `v` per coefficient and summing in
/// integers removes the error at its source rather than at the end.
///
/// Returns `None` for a non-finite coefficient, which [`energy_milli`] turns into
/// [`ENERGY_MILLI_NON_FINITE`]. A coefficient that is finite but large enough
/// that `c * 1000.0` overflows to infinity saturates on the cast instead, which
/// preserves the documented "finite values saturate" edge.
fn coefficient_milli(c: f64) -> Option<i64> {
    if !c.is_finite() {
        return None;
    }
    #[expect(
        clippy::cast_possible_truncation,
        reason = "round() is exact over the i32 wire range; larger finite values saturate by design"
    )]
    Some((c * 1000.0).round() as i64)
}

/// Clamp an `i128` sum to the `i64` the wire carries.
fn saturate_i64(acc: i128) -> i64 {
    i64::try_from(acc).unwrap_or(if acc.is_negative() {
        i64::MIN
    } else {
        i64::MAX
    })
}

/// Ising energy `Σ h_i·s_i + Σ j_k·s_u·s_v`, in milli.
///
/// Coefficients are read as exact integer milli (see `coefficient_milli`) and
/// summed in `i128`, so the result is the same integer the coordinator computes.
/// Entries with no matching spin, and edges naming an out-of-range node, are
/// skipped rather than treated as zero-valued terms.
///
/// # Edges
///
/// - A non-finite coefficient that is actually read returns
///   [`ENERGY_MILLI_NON_FINITE`]. Coefficients skipped by the bounds checks above
///   are never inspected, so they cannot trigger the sentinel.
/// - A finite sum outside `i64` saturates to `i64::MAX` / `i64::MIN`. Python's
///   arbitrary-precision `int()` never saturates, so the bindings replicate this
///   clamp deliberately.
#[must_use]
pub fn energy_milli(spins: &[i8], h: &[f64], j: &[f64], edges: &[(usize, usize)]) -> i64 {
    let mut acc: i128 = 0;
    for (i, &s) in spins.iter().enumerate() {
        // `get` keeps the length check and the read in one step: a field with no
        // matching spin index simply contributes nothing.
        if let Some(&coeff) = h.get(i) {
            let Some(milli) = coefficient_milli(coeff) else {
                return ENERGY_MILLI_NON_FINITE;
            };
            acc = acc.saturating_add(i128::from(milli) * i128::from(sign(s)));
        }
    }
    for (k, &(u, v)) in edges.iter().enumerate() {
        // An edge is scored only when its coupling and both endpoints exist.
        let (Some(&coeff), Some(&su), Some(&sv)) = (j.get(k), spins.get(u), spins.get(v)) else {
            continue;
        };
        let Some(milli) = coefficient_milli(coeff) else {
            return ENERGY_MILLI_NON_FINITE;
        };
        acc = acc.saturating_add(i128::from(milli) * i128::from(sign(su)) * i128::from(sign(sv)));
    }
    // i128 holds any sum the i64-bounded terms can reach; clamp to the wire type.
    saturate_i64(acc)
}

/// Ising energy `Σ h_i·s_i + Σ j_k·s_u·s_v` from milli coefficients, in milli.
///
/// The integer form of [`energy_milli`], with the same rules: an entry with no
/// matching spin and an edge naming an out-of-range node contribute nothing, a
/// spin greater than zero is `+1` and any other spin is `-1`, and the `i128`
/// sum saturates to `i64::MIN` or `i64::MAX`. For every input it returns what
/// [`energy_milli`] returns on the `v / 1000.0` floats. Integer input cannot be
/// non-finite, so the non-finite condition cannot arise. A saturated or exact
/// sum can still equal the [`ENERGY_MILLI_NON_FINITE`] value.
#[must_use]
pub fn energy_from_milli(
    spins: &[i8],
    h_milli: &[i32],
    j_milli: &[i32],
    edges: &[(usize, usize)],
) -> i64 {
    let mut acc: i128 = 0;
    // `zip` stops at the shorter slice, so a spin with no bias contributes
    // nothing.
    for (&s, &milli) in spins.iter().zip(h_milli) {
        acc = acc.saturating_add(i128::from(milli) * i128::from(sign(s)));
    }
    for (k, &(u, v)) in edges.iter().enumerate() {
        // An edge is scored only when its coupling and both endpoints exist.
        let (Some(&milli), Some(&su), Some(&sv)) = (j_milli.get(k), spins.get(u), spins.get(v))
        else {
            continue;
        };
        acc = acc.saturating_add(i128::from(milli) * i128::from(sign(su)) * i128::from(sign(sv)));
    }
    saturate_i64(acc)
}

/// Flip-invariant Hamming distance between two spin vectors, `min(d, n - d)`.
///
/// Returns `0` when the vectors differ in width. Mismatched widths are a caller
/// bug with no sound answer: the previous behaviour compared only the common
/// prefix while normalizing by `a.len()`, which reports a confidently wrong
/// distance. This signature has no way to fail loudly, so it returns the one
/// value that cannot inflate a diversity score, matching the zero-width guard in
/// [`set_diversity`].
#[must_use]
pub fn hamming_flip_invariant(a: &[i8], b: &[i8]) -> u32 {
    if a.len() != b.len() {
        return 0;
    }
    let n = a.len();
    let raw = a
        .iter()
        .zip(b)
        // Same threshold as `sign`: s > 0 is +1, else -1.
        .filter(|(x, y)| (**x > 0) != (**y > 0))
        .count();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "API returns u32; distance is at most a.len() / 2"
    )]
    {
        raw.min(n - raw) as u32
    }
}

/// Mean pairwise flip-invariant Hamming distance over a solution set, normalized
/// by spin width.
///
/// The result lies in `[0.0, 0.5]`: [`hamming_flip_invariant`] never exceeds half
/// the width, so a normalized pair distance never exceeds 0.5 and neither does
/// their mean.
///
/// Returns `0.0` for fewer than two solutions, for zero width, and for a ragged
/// set. A set whose vectors disagree in width has no single width to normalize
/// by; taking `solutions[0].len()` as the set width, as this previously did,
/// silently rescales every pair against one arbitrary member and can report a
/// value outside the range above.
#[must_use]
pub fn set_diversity(solutions: &[Vec<i8>]) -> f64 {
    let Some(first) = solutions.first() else {
        return 0.0;
    };
    if solutions.len() < 2 {
        return 0.0;
    }
    let width = first.len();
    if width == 0 || solutions.iter().any(|s| s.len() != width) {
        return 0.0;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "spin width is problem-sized; f64 mantissa holds typical N"
    )]
    let width = width as f64;
    let mut sum = 0.0f64;
    let mut pairs = 0u64;
    for (i, a) in solutions.iter().enumerate() {
        for b in solutions.iter().skip(i + 1) {
            sum += f64::from(hamming_flip_invariant(a, b)) / width;
            pairs += 1;
        }
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "pair count fits exact integer f64 for practical solution-set sizes"
    )]
    {
        // `solutions.len() >= 2` guarantees at least one pair.
        sum / pairs as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn energy_milli_scales_and_rounds_coefficients() {
        type Case = (
            &'static str,
            Vec<i8>,
            Vec<f64>,
            Vec<f64>,
            Vec<(usize, usize)>,
            i64,
        );
        let cases: [Case; 3] = [
            // spins [+1,-1]; h=[1.0, -0.5]; edge (0,1) J=2.0
            // E = (1000*1) + (-500*-1) + (2000 * 1 * -1) = 1000 + 500 - 2000 = -500 milli
            (
                "sign and scaling",
                vec![1, -1],
                vec![1.0, -0.5],
                vec![2.0],
                vec![(0, 1)],
                -500,
            ),
            // The consensus bug this module was fixed for: ten 100-milli fields with
            // +1 spins sum to 1000 in integer milli, but 0.1 is not representable in
            // binary, so f64 accumulation reached 0.999999999999999889 and truncated
            // to 999 — rejecting every solver on any ordinary 0.1-valued field.
            (
                "ten 0.1 fields",
                vec![1; 10],
                vec![0.1; 10],
                vec![],
                vec![],
                1000,
            ),
            // Three 37-milli fields: the smallest case that diverged (111 vs 110).
            (
                "three 0.037 fields",
                vec![1; 3],
                vec![0.037; 3],
                vec![],
                vec![],
                111,
            ),
        ];
        for (label, spins, h, j, edges, expected) in cases {
            assert_eq!(energy_milli(&spins, &h, &j, &edges), expected, "{label}");
        }
        // Rounding recovers the wire value exactly across the i32 extremes, which
        // is the precondition the coefficient_milli doc argues for.
        for v in [i32::MIN, i32::MAX, 999, 1, -1, 123_456_789, -2_147_483_647] {
            let coeff = f64::from(v) / 1000.0;
            assert_eq!(
                energy_milli(&[1], &[coeff], &[], &[]),
                i64::from(v),
                "coefficient {v} milli did not round-trip"
            );
        }
    }

    #[test]
    fn energy_milli_skips_out_of_range_edges_without_reading_them() {
        let cases = [
            // edge (0, 5) references node 5, out of range for a 2-spin problem; must
            // be skipped like a length-mismatched h/j entry, not panic.
            // E = (1000*1) + (1000*-1) = 0 milli
            ("out-of-range edge", 1.0),
            // The coupling is non-finite but its edge names an out-of-range node, so
            // it is skipped before inspection and cannot raise the sentinel.
            ("non-finite coupling on a skipped edge", f64::NAN),
        ];
        for (label, coupling) in cases {
            let e = energy_milli(&[1, -1], &[1.0, 1.0], &[coupling], &[(0, 5)]);
            assert_eq!(e, 0, "{label}");
        }
    }

    #[test]
    fn energy_milli_saturates_both_ways() {
        let cases = [
            // 1e16 -> 1e19 milli, past i64::MAX (~9.223e18) -> saturates.
            ("1e16", vec![1], vec![1e16], i64::MAX),
            // Finite but so large that c*1000.0 overflows to infinity; the cast
            // saturates rather than reporting the non-finite sentinel, because the
            // input the caller supplied was finite.
            ("1e308", vec![1], vec![1e308], i64::MAX),
            // Two saturating terms accumulate in i128 and clamp once at the end.
            ("two 1e308 terms", vec![1, 1], vec![1e308, 1e308], i64::MAX),
            // Ground states are negative-energy, so the negative overflow path is
            // realistic; it must saturate to i64::MIN, not wrap.
            ("-1e16", vec![1], vec![-1e16], i64::MIN),
            ("-1e308", vec![1], vec![-1e308], i64::MIN),
            (
                "two -1e308 terms",
                vec![1, 1],
                vec![-1e308, -1e308],
                i64::MIN,
            ),
        ];
        for (label, spins, h, expected) in cases {
            assert_eq!(energy_milli(&spins, &h, &[], &[]), expected, "{label}");
        }
    }

    #[test]
    fn hamming_flip_invariant_handles_equal_and_ragged_widths() {
        let cases: [(&str, &[i8], &[i8], u32); 4] = [
            ("one flipped bit", &[1, 1, 1], &[-1, 1, 1], 1),
            // Exact inverse: raw distance 3, complement 0 -> 0.
            ("exact inverse", &[1, 1, 1], &[-1, -1, -1], 0),
            // The old code zipped the common prefix (distance 2) but normalized by
            // a.len() = 4, reporting 2 for vectors it never fully compared.
            ("ragged, longer first", &[1, 1, 1, 1], &[-1, -1], 0),
            ("ragged, longer second", &[-1, -1], &[1, 1, 1, 1], 0),
        ];
        for (label, a, b, expected) in cases {
            assert_eq!(hamming_flip_invariant(a, b), expected, "{label}");
        }
    }

    #[test]
    fn set_diversity_is_zero_for_degenerate_sets() {
        let cases: [(&str, Vec<Vec<i8>>); 6] = [
            ("fewer than two solutions", vec![vec![1, 1]]),
            // Two zero-length solution vectors would divide by width 0; must return
            // 0.0, matching the shared reference (not NaN).
            ("zero-width solutions", vec![vec![], vec![]]),
            ("empty set", vec![]),
            // The bead's case: widths 2 and 4. The old code took width 2 from
            // solutions[0] and compared only the 2-element prefix.
            ("ragged widths", vec![vec![1, 1], vec![1, 1, 1, 1]]),
            // Ragged in the other direction, and ragged past the first pair.
            ("ragged, longer first", vec![vec![1, 1, 1, 1], vec![1, 1]]),
            (
                "ragged past the first pair",
                vec![vec![1, 1], vec![-1, 1], vec![1, 1, 1]],
            ),
        ];
        #[expect(
            clippy::float_cmp,
            reason = "exact golden equality for degenerate sets"
        )]
        for (label, set) in cases {
            assert_eq!(set_diversity(&set), 0.0, "{label}");
        }
    }

    #[test]
    fn diversity_never_exceeds_one_half() {
        // Maximally different equal-width solutions still cap at 0.5, because the
        // distance is flip-invariant.
        let d = set_diversity(&[vec![1, 1, -1, -1], vec![1, -1, 1, -1], vec![1, 1, 1, -1]]);
        assert!((0.0..=0.5).contains(&d), "diversity {d} outside [0, 0.5]");
    }

    use ising_generators::rng::ChaCha8Rng;

    /// The unit-float form a binding builds from milli coefficients.
    fn unit(milli: &[i32]) -> Vec<f64> {
        milli.iter().map(|&v| f64::from(v) / 1000.0).collect()
    }

    #[test]
    fn energy_from_milli_scores_wire_integers() {
        type Case = (
            &'static str,
            Vec<i8>,
            Vec<i32>,
            Vec<i32>,
            Vec<(usize, usize)>,
            i64,
        );
        let n = 1000;
        let cases: [Case; 5] = [
            // Same problem as the float scorer's sign and scaling case, in milli.
            (
                "sign and scaling",
                vec![1, -1],
                vec![1000, -500],
                vec![2000],
                vec![(0, 1)],
                -500,
            ),
            // Spin 2 has no bias, coupling 1 has no edge, and edge (0, 5) names a
            // node that does not exist. Only the two biases score: 1000 - 1000.
            (
                "skipped bias, coupling and edge",
                vec![1, -1, 1],
                vec![1000, 1000],
                vec![1000, 7000],
                vec![(0, 5)],
                0,
            ),
            // 0 and i8::MIN score as -1. 2 and i8::MAX score as +1.
            (
                "non-positive spins score as minus one",
                vec![0, i8::MIN, 2, i8::MAX],
                vec![1, 10, 100, 1000],
                vec![],
                vec![],
                -1 - 10 + 100 + 1000,
            ),
            (
                "i32::MIN biases over many spins",
                vec![1; n],
                vec![i32::MIN; n],
                vec![],
                vec![],
                i64::from(i32::MIN) * 1000,
            ),
            (
                "i32::MAX coupling",
                vec![-1, -1],
                vec![0, 0],
                vec![i32::MAX],
                vec![(0, 1)],
                i64::from(i32::MAX),
            ),
        ];
        for (label, spins, h, j, edges, expected) in cases {
            assert_eq!(
                energy_from_milli(&spins, &h, &j, &edges),
                expected,
                "{label}"
            );
        }
    }

    #[test]
    fn saturate_i64_clamps_both_ends() {
        assert_eq!(saturate_i64(i128::from(i64::MAX) + 1), i64::MAX);
        assert_eq!(saturate_i64(i128::from(i64::MIN) - 1), i64::MIN);
        assert_eq!(saturate_i64(-7), -7);
    }

    /// A value in `0..bound`.
    fn below(rng: &mut ChaCha8Rng, bound: u32) -> usize {
        usize::try_from(rng.next_u32() % bound).unwrap()
    }

    /// Any `i32`, with each extreme drawn one time in eight.
    fn random_milli(rng: &mut ChaCha8Rng) -> i32 {
        match rng.next_u32() % 8 {
            0 => i32::MIN,
            1 => i32::MAX,
            _ => i32::from_le_bytes(rng.next_u32().to_le_bytes()),
        }
    }

    #[test]
    fn energy_from_milli_matches_energy_milli_on_random_problems() {
        let mut rng = ChaCha8Rng::from_seed([0x5A; 32]);
        for case in 0..2000 {
            // Up to 39 spins. Bias, coupling, and edge counts vary on their
            // own, and endpoints reach past the last spin, so every skip rule
            // is exercised.
            let spins: Vec<i8> = (0..below(&mut rng, 40))
                .map(|_| i8::from_le_bytes([rng.next_u32().to_le_bytes()[0]]))
                .collect();
            let h: Vec<i32> = (0..below(&mut rng, 44))
                .map(|_| random_milli(&mut rng))
                .collect();
            let edges: Vec<(usize, usize)> = (0..below(&mut rng, 80))
                .map(|_| (below(&mut rng, 44), below(&mut rng, 44)))
                .collect();
            let j: Vec<i32> = (0..below(&mut rng, 84))
                .map(|_| random_milli(&mut rng))
                .collect();
            assert_eq!(
                energy_from_milli(&spins, &h, &j, &edges),
                energy_milli(&spins, &unit(&h), &unit(&j), &edges),
                "case {case}"
            );
        }
    }
}
