//! The `IQM_SAMPLE` cases of the `--solve` gate.
//!
//! A solver that lists `IQM_SAMPLE` in `supportedKinds` must solve the IQM
//! form, with every value within its bound and every energy equal to the
//! consensus score of its values, and must refuse a malformed one. A solver
//! that does not list it must refuse the IQM form with exit 64.

use super::{
    expect_refused, run, skip_pair, Advertised, Run, SolveReport, BASE_SEED, EXIT_CONFIG_INVALID,
};
use quip_protocol::scoring::iqm_energy_from_milli;
use serde_json::{json, Value};

/// The `supportedKinds` entry that claims the IQM form.
pub(super) const IQM_KIND: &str = "IQM_SAMPLE";

/// The gate's integer quadratic model, coefficients in milli.
#[derive(Debug, Clone, PartialEq, Eq)]
struct GateIqm {
    bounds: Vec<u32>,
    h_milli: Vec<i32>,
    j_milli: Vec<i32>,
    edges: Vec<(usize, usize)>,
    num_reads: usize,
}

/// A four-variable ring with mixed signs, so neither all-zero nor
/// all-at-bound values are the minimum.
fn gate_problem() -> GateIqm {
    GateIqm {
        bounds: vec![1, 2, 3, 7],
        h_milli: vec![1000, -2000, 500, -1000],
        j_milli: vec![-1000, 1000, -500, 250],
        edges: vec![(0, 1), (1, 2), (2, 3), (3, 0)],
        num_reads: 4,
    }
}

impl GateIqm {
    fn to_json(&self) -> Value {
        let units = |milli: &[i32]| {
            milli
                .iter()
                .map(|&m| f64::from(m) / 1000.0)
                .collect::<Vec<_>>()
        };
        json!({
            "problem": "iqm",
            "bounds": self.bounds,
            "h": units(&self.h_milli),
            "j": units(&self.j_milli),
            "edges": self.edges,
            "num_reads": self.num_reads,
            "num_sweeps": 64,
            "sweeps_per_beta": 1,
            "beta_range": null,
            "seed": BASE_SEED,
        })
    }

    fn bytes(&self) -> Vec<u8> {
        self.to_json().to_string().into_bytes()
    }

    fn energy(&self, values: &[u32]) -> i64 {
        iqm_energy_from_milli(values, &self.h_milli, &self.j_milli, &self.edges)
    }
}

/// The values of read `index`: one integer per variable, each within its
/// bound.
fn read_values(index: usize, read: &Value, bounds: &[u32]) -> Result<Vec<u32>, String> {
    let values: Vec<u32> = read
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("solution {index} has no values array"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|v| u32::try_from(v).ok())
                .ok_or_else(|| format!("solution {index} has a value that is not a u32: {v}"))
        })
        .collect::<Result<_, _>>()?;
    if values.len() != bounds.len() {
        return Err(format!(
            "solution {index} has {} values for {} variables",
            values.len(),
            bounds.len()
        ));
    }
    for (variable, (&x, &bound)) in values.iter().zip(bounds).enumerate() {
        if x > bound {
            return Err(format!(
                "solution {index} sets variable {variable} to {x}, above its bound {bound}"
            ));
        }
    }
    Ok(values)
}

/// Grade one IQM run: exit 0, `num_reads` reads, each within the bounds, with
/// the consensus energy of its values, coherent timing, and the sampler echo.
fn check_iqm_solutions(adv: &Advertised, problem: &GateIqm, run: &Run) -> Result<(), String> {
    if run.code != Some(0) {
        return Err(format!(
            "exited {:?}, expected 0; stderr: {}",
            run.code,
            run.stderr_tail()
        ));
    }
    let out: Value =
        serde_json::from_slice(&run.stdout).map_err(|e| format!("stdout is not JSON: {e}"))?;
    let reads = out.as_array().ok_or("stdout is not a JSON array")?;
    if reads.len() != problem.num_reads {
        return Err(format!(
            "{} solutions for num_reads {}",
            reads.len(),
            problem.num_reads
        ));
    }
    let expected_sampler = json!({
        "backend": adv.backend,
        "algorithm": adv.algorithm,
        "num_reads": problem.num_reads,
        "num_sweeps": 64,
        "sweeps_per_beta": 1,
        "beta_range": null,
        "beta_schedule_type": "geometric",
        "seed": BASE_SEED,
    });
    for (index, read) in reads.iter().enumerate() {
        let values = read_values(index, read, &problem.bounds)?;
        let reported = read
            .get("energy_milli")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("solution {index} has no integer energy_milli"))?;
        let rescored = problem.energy(&values);
        if reported != rescored {
            return Err(format!(
                "solution {index} reports energy_milli {reported}, its values score {rescored}"
            ));
        }
        let micros = |key: &str| {
            read.get("timing")
                .and_then(|t| t.get(key))
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("solution {index} timing has no integer {key}"))
        };
        if micros("sample_us")? > micros("total_us")? {
            return Err(format!("solution {index} sample_us exceeds total_us"));
        }
        // A ladder report is optional for the IQM form.
        let mut sampler = read.get("sampler").cloned().unwrap_or(Value::Null);
        if let Some(object) = sampler.as_object_mut() {
            let _ = object.remove("beta_ladder");
        }
        if sampler != expected_sampler {
            return Err(format!(
                "solution {index} sampler is {sampler}, expected {expected_sampler}"
            ));
        }
    }
    Ok(())
}

/// Run the IQM cases: solve and refuse a malformed model when the solver
/// lists `IQM_SAMPLE`, or refuse the form with exit 64 when it does not.
pub(super) async fn iqm_cases(bin: &str, adv: &Advertised, report: &mut SolveReport) {
    let problem = gate_problem();
    if !adv.lists_kind(IQM_KIND) {
        report.grade(
            "iqm-unsupported",
            expect_refused(bin, &problem.bytes(), EXIT_CONFIG_INVALID).await,
        );
        skip_pair(report, ("iqm-solve", "iqm-bounds-mismatch"), IQM_KIND);
        return;
    }
    let solved = match run(bin, &["--solve"], &problem.bytes()).await {
        Ok(output) => check_iqm_solutions(adv, &problem, &output),
        Err(why) => Err(why),
    };
    report.grade("iqm-solve", solved);
    let short = GateIqm {
        bounds: problem.bounds.iter().skip(1).copied().collect(),
        ..problem
    };
    report.grade(
        "iqm-bounds-mismatch",
        expect_refused(bin, &short.bytes(), EXIT_CONFIG_INVALID).await,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adv() -> Advertised {
        Advertised {
            backend: "mock".into(),
            algorithm: "sa".into(),
            supported_kinds: vec!["ISING_SAMPLE".into(), IQM_KIND.into()],
            ..Advertised::default()
        }
    }

    fn ok_run(stdout: &Value) -> Run {
        Run {
            code: Some(0),
            stdout: stdout.to_string().into_bytes(),
            stderr: String::new(),
        }
    }

    fn read(values: &[u32], energy: i64) -> Value {
        json!({
            "values": values,
            "energy_milli": energy,
            "timing": {"sample_us": 1, "total_us": 2},
            "sampler": {
                "backend": "mock", "algorithm": "sa", "num_reads": 1, "num_sweeps": 64,
                "sweeps_per_beta": 1, "beta_range": null, "beta_schedule_type": "geometric",
                "seed": BASE_SEED,
            },
        })
    }

    fn one_read() -> GateIqm {
        GateIqm {
            num_reads: 1,
            ..gate_problem()
        }
    }

    #[test]
    fn the_gate_problem_scores_with_the_consensus_scorer() {
        let p = gate_problem();
        assert_eq!(p.bounds.len(), p.h_milli.len());
        assert_eq!(p.j_milli.len(), p.edges.len());
        let v = p.to_json();
        assert_eq!(v.get("problem"), Some(&json!("iqm")));
        assert_eq!(v.get("bounds"), Some(&json!(p.bounds)));
        assert_eq!(p.energy(&[0, 0, 0, 0]), 0);
    }

    #[test]
    fn check_iqm_solutions_accepts_a_correct_read_and_names_each_defect() {
        let p = one_read();
        let zeros = vec![0; p.bounds.len()];
        let good = json!([read(&zeros, 0)]);
        assert_eq!(check_iqm_solutions(&adv(), &p, &ok_run(&good)), Ok(()));

        let at_bounds = p.bounds.clone();
        let energy = p.energy(&at_bounds);
        let wrong = json!([read(&at_bounds, energy + 1)]);
        let err = check_iqm_solutions(&adv(), &p, &ok_run(&wrong)).expect_err("energy");
        assert!(err.contains(&format!("score {energy}")), "{err}");

        let mut over = p.bounds.clone();
        if let Some(first) = over.first_mut() {
            *first += 1;
        }
        let err = check_iqm_solutions(&adv(), &p, &ok_run(&json!([read(&over, p.energy(&over))])))
            .expect_err("bound");
        assert!(err.contains("bound"), "{err}");

        let short = json!([read(&[0], 0)]);
        assert!(check_iqm_solutions(&adv(), &p, &ok_run(&short)).is_err());

        let negative = json!([{"values": [-1, 0, 0, 0], "energy_milli": 0}]);
        assert!(check_iqm_solutions(&adv(), &p, &ok_run(&negative)).is_err());

        let two = json!([read(&zeros, 0), read(&zeros, 0)]);
        assert!(check_iqm_solutions(&adv(), &p, &ok_run(&two)).is_err());

        let mut other_seed = read(&zeros, 0);
        if let Some(seed) = other_seed.pointer_mut("/sampler/seed") {
            *seed = json!(1);
        }
        let err =
            check_iqm_solutions(&adv(), &p, &ok_run(&json!([other_seed]))).expect_err("sampler");
        assert!(err.contains("sampler"), "{err}");

        let failed = Run {
            code: Some(64),
            ..ok_run(&good)
        };
        assert!(check_iqm_solutions(&adv(), &p, &failed).is_err());
    }

    #[test]
    fn capabilities_parse_reads_supported_kinds() {
        let parsed = Advertised::parse(&json!({
            "backend": "cpu", "algorithm": "sa", "supportedKinds": ["ISING_SAMPLE", "IQM_SAMPLE"],
        }))
        .expect("parses");
        assert!(parsed.lists_kind(IQM_KIND));
        let minimal =
            Advertised::parse(&json!({"backend": "cpu", "algorithm": "sa"})).expect("defaults");
        assert!(!minimal.lists_kind(IQM_KIND));
        assert!(Advertised::parse(
            &json!({"backend": "cpu", "algorithm": "sa", "supportedKinds": "IQM_SAMPLE"})
        )
        .is_err());
    }
}
