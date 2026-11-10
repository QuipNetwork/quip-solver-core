//! Which models a binary solves correctly, beyond its node and edge caps.
//!
//! A binary states these in its [`BackendIdentity`](crate::BackendIdentity),
//! they travel as `Capabilities` fields 14 to 16, and the SDK enforces them on
//! every plain job and every `--solve` problem, the same way it enforces
//! `max_nodes` and `max_edges`. A caller reads them to route a model to a
//! binary that solves it, rather than learning from a refusal.

/// Model limits a binary advertises. `None` in any field makes no claim.
///
/// All three are measured on the model as received, in milli units: a field of
/// one unit is `1000`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ModelLimits {
    /// Most edges incident on one node. `Some(0)` is unbounded.
    pub max_degree: Option<u32>,
    /// Largest local field `max_i(|h_i| + Σ_j |J_ij|)`, in milli. `Some(0)` is
    /// unbounded.
    ///
    /// The bound is on the field rather than on each coefficient because a
    /// narrow fast path overflows on the sum: an int8 delta holds only while
    /// `2·s·max_i(|h_i| + Σ_j |J_ij|) <= 127`.
    pub max_local_field_milli: Option<u32>,
    /// The step, in milli, at which coefficients are represented exactly: a
    /// binary that takes only ±1 states `1000`, an int8 path at integer scale
    /// `s` states `1000 / s`, a wide path `1`.
    ///
    /// Without [`EXACT_COEFFICIENTS_FEATURE`](crate::EXACT_COEFFICIENTS_FEATURE)
    /// this is information only: the binary quantizes an off-step value and the
    /// SDK rescores the energies. With it, the SDK refuses one. `Some(0)` is not
    /// a step and makes no claim.
    pub exact_resolution_milli: Option<u32>,
}

impl ModelLimits {
    /// No claim in any field, for a binary that states no limits.
    pub const UNSTATED: Self = Self {
        max_degree: None,
        max_local_field_milli: None,
        exact_resolution_milli: None,
    };

    /// The advertised step, or `None` for no claim (including `Some(0)`).
    fn resolution(self) -> Option<u32> {
        self.exact_resolution_milli.filter(|&r| r != 0)
    }

    /// Check a model, coefficients in milli, against these limits.
    ///
    /// `exact` is whether the binary lists `exact-coefficients`; only then is
    /// an off-step coefficient refused. The model has already passed the shape
    /// check, so every endpoint is in range.
    ///
    /// # Errors
    ///
    /// Returns which limit the model exceeds. Every refusal is capacity-class:
    /// the same model fails again on this binary and may succeed on another.
    pub(crate) fn check(
        self,
        h_milli: &[f64],
        j_milli: &[f64],
        edges: &[(usize, usize)],
        exact: bool,
    ) -> Result<(), String> {
        if exact {
            if let Some(step) = self.resolution() {
                let step = f64::from(step);
                let on_step = |v: f64| v.fract() == 0.0 && (v % step) == 0.0;
                if let Some(v) = h_milli.iter().chain(j_milli).find(|&&v| !on_step(v)) {
                    return Err(format!(
                        "coefficient {v} milli is not a multiple of this binary's \
                         exact resolution of {step} milli"
                    ));
                }
            }
        }
        let max_degree = self.max_degree.filter(|&d| d != 0);
        let max_field = self.max_local_field_milli.filter(|&f| f != 0);
        if max_degree.is_none() && max_field.is_none() {
            return Ok(());
        }
        let mut degree = vec![0_u64; h_milli.len()];
        let mut field: Vec<f64> = h_milli.iter().map(|v| v.abs()).collect();
        for (&(u, v), &jv) in edges.iter().zip(j_milli) {
            for node in [u, v] {
                if let (Some(d), Some(f)) = (degree.get_mut(node), field.get_mut(node)) {
                    *d += 1;
                    *f += jv.abs();
                }
            }
        }
        if let Some(limit) = max_degree {
            if let Some((node, &d)) = degree
                .iter()
                .enumerate()
                .find(|&(_, &d)| d > u64::from(limit))
            {
                return Err(format!(
                    "node {node} has degree {d}, above this binary's maximum of {limit}"
                ));
            }
        }
        if let Some(limit) = max_field {
            if let Some((node, &f)) = field
                .iter()
                .enumerate()
                .find(|&(_, &f)| f > f64::from(limit))
            {
                return Err(format!(
                    "node {node} has local field {f} milli, above this binary's \
                     maximum of {limit} milli"
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::ModelLimits;

    const STAR: [(usize, usize); 3] = [(0, 1), (0, 2), (0, 3)];

    fn limits(degree: u32, field: u32, step: u32) -> ModelLimits {
        ModelLimits {
            max_degree: Some(degree),
            max_local_field_milli: Some(field),
            exact_resolution_milli: Some(step),
        }
    }

    /// `(label, limits, h, j, edges, exact, expected)`. `expected` is `None`
    /// when the check passes and `Some(needle)` when it fails with a message
    /// containing the needle.
    type Case<'a> = (
        &'a str,
        ModelLimits,
        &'a [f64],
        &'a [f64],
        &'a [(usize, usize)],
        bool,
        Option<&'a str>,
    );

    fn limit_cases() -> [Case<'static>; 7] {
        let zeros: &[f64] = &[0.0; 4];
        let ones: &[f64] = &[1000.0; 3];
        // Node 0: |h| 500 + three couplings of 1000 = 3500.
        let field_h: &[f64] = &[-500.0, 0.0, 0.0, 0.0];
        let field_j: &[f64] = &[1000.0, -1000.0, 1000.0];
        let pair: &[(usize, usize)] = &[(0, 1)];
        [
            (
                "unstated limits accept anything",
                ModelLimits::UNSTATED,
                &[1e12, -0.5],
                &[3.0],
                pair,
                true,
                None,
            ),
            (
                "zero degree and field are unbounded",
                limits(0, 0, 1),
                zeros,
                &[1e9, 1e9, 1e9],
                &STAR,
                true,
                None,
            ),
            (
                "a node at the degree limit passes",
                limits(3, 0, 1),
                zeros,
                ones,
                &STAR,
                false,
                None,
            ),
            (
                "one above the degree limit fails",
                limits(2, 0, 1),
                zeros,
                ones,
                &STAR,
                false,
                Some("degree 3"),
            ),
            (
                "a field at the limit passes",
                limits(0, 3500, 1),
                field_h,
                field_j,
                &STAR,
                false,
                None,
            ),
            (
                "a field one above the limit fails",
                limits(0, 3499, 1),
                field_h,
                field_j,
                &STAR,
                false,
                Some("local field 3500"),
            ),
            (
                "duplicate edges count twice",
                limits(1, 0, 1),
                &[0.0, 0.0],
                &[1000.0, 1000.0],
                &[(0, 1), (0, 1)],
                false,
                Some(""),
            ),
        ]
    }

    fn step_cases() -> [Case<'static>; 5] {
        let off_step_h: &[f64] = &[1500.0, 0.0];
        let pair: &[(usize, usize)] = &[(0, 1)];
        [
            (
                "an off-step coefficient passes for a non-exact binary",
                limits(0, 0, 1000),
                off_step_h,
                &[1000.0],
                pair,
                false,
                None,
            ),
            (
                "an off-step coefficient fails for an exact binary",
                limits(0, 0, 1000),
                off_step_h,
                &[1000.0],
                pair,
                true,
                Some("1500"),
            ),
            (
                "a finer step accepts the same coefficient",
                limits(0, 0, 500),
                off_step_h,
                &[1000.0],
                pair,
                true,
                None,
            ),
            (
                "sub-milli is off any step",
                limits(0, 0, 1),
                &[0.5],
                &[],
                &[],
                true,
                Some(""),
            ),
            (
                "a zero step makes no claim",
                limits(0, 0, 0),
                &[0.5],
                &[],
                &[],
                true,
                None,
            ),
        ]
    }

    #[test]
    fn model_limits_check_follows_the_table() {
        for (label, limits, h, j, edges, exact, expected) in
            limit_cases().into_iter().chain(step_cases())
        {
            match (limits.check(h, j, edges, exact), expected) {
                (Ok(()), None) => {}
                (Err(err), Some(needle)) => assert!(err.contains(needle), "{label}: {err}"),
                (got, expected) => panic!("{label}: got {got:?}, expected {expected:?}"),
            }
        }
    }
}
