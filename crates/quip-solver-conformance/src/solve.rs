//! The `--solve` conformance gate: runs a solver binary in one-shot mode and
//! grades it against SPEC section 2.
//!
//! The gate first reads `--capabilities`, then sends one problem per case on
//! stdin. Base cases apply to every solver. A case for optional behaviour runs
//! only when the solver advertises it: `linear-schedule` and
//! `exact-coefficients` in `features`, and the model limits in
//! `maxDegree`, `maxLocalFieldMilli` and `exactResolutionMilli`. A solver that
//! advertises nothing optional skips those cases rather than failing them.

use quip_protocol::scoring::energy_milli;
use serde_json::{json, Value};
use std::fmt::Write as _;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt as _;
use tokio::process::Command;

/// How long one `--solve` or `--capabilities` run may take. Generous because a
/// GPU solver may compile its kernels on first launch.
const RUN_TIMEOUT: Duration = Duration::from_mins(2);

/// Exit code for a malformed `--solve` problem.
pub const EXIT_CONFIG_INVALID: i32 = 64;

/// Exit code for a `--solve` model beyond what the solver solves.
pub const EXIT_CAPACITY: i32 = 65;

/// Feature string for a solver whose ladder honours `"linear"`. Mirrors
/// `quip_solver_core::LINEAR_SCHEDULE_FEATURE`, which this crate cannot depend
/// on without a publish cycle.
pub const LINEAR_SCHEDULE_FEATURE: &str = "linear-schedule";

/// Feature string for the refuse-not-quantize policy. Mirrors
/// `quip_solver_core::EXACT_COEFFICIENTS_FEATURE`.
pub const EXACT_COEFFICIENTS_FEATURE: &str = "exact-coefficients";

/// Seed of the base problem. Above `u32::MAX`, so it reaches the high half of
/// the seed, and below 2^53, so a JavaScript solver reads it exactly.
const BASE_SEED: u64 = 0x0001_0000_0000_3039;

/// Nodes in the base ring.
const BASE_NODES: usize = 8;

/// How one case ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// The solver did what the specification requires.
    Passed,
    /// The solver did something else. Carries what.
    Failed(String),
    /// The case does not apply to this solver. Carries why.
    Skipped(String),
}

/// One graded case.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolveCase {
    /// Stable case name, for example `self-loop`.
    pub name: &'static str,
    /// How it ended.
    pub outcome: CaseOutcome,
}

/// Every case the gate ran against one binary.
#[derive(Debug, Clone, Default)]
pub struct SolveReport {
    /// The cases, in the order they ran.
    pub cases: Vec<SolveCase>,
}

impl SolveReport {
    /// True when no case failed. Skipped cases do not count against a solver.
    #[must_use]
    pub fn is_conformant(&self) -> bool {
        !self.cases.is_empty()
            && self
                .cases
                .iter()
                .all(|c| !matches!(c.outcome, CaseOutcome::Failed(_)))
    }

    /// The outcome of the case called `name`, when it ran.
    #[must_use]
    pub fn outcome(&self, name: &str) -> Option<&CaseOutcome> {
        self.cases
            .iter()
            .find(|c| c.name == name)
            .map(|c| &c.outcome)
    }

    /// One line per case.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut out = String::new();
        for case in &self.cases {
            let _ = match &case.outcome {
                CaseOutcome::Passed => writeln!(out, "  [pass] {}", case.name),
                CaseOutcome::Failed(why) => writeln!(out, "  [FAIL] {}: {why}", case.name),
                CaseOutcome::Skipped(why) => {
                    writeln!(out, "  [skip] {} (not advertised: {why})", case.name)
                }
            };
        }
        out
    }

    fn push(&mut self, name: &'static str, outcome: CaseOutcome) {
        self.cases.push(SolveCase { name, outcome });
    }

    fn grade(&mut self, name: &'static str, result: Result<(), String>) {
        self.push(
            name,
            result.map_or_else(CaseOutcome::Failed, |()| CaseOutcome::Passed),
        );
    }
}

/// What the solver advertised in `--capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct Advertised {
    backend: String,
    algorithm: String,
    max_nodes: u64,
    max_edges: u64,
    features: Vec<String>,
    max_degree: Option<u64>,
    max_local_field_milli: Option<u64>,
    exact_resolution_milli: Option<u64>,
}

impl Advertised {
    /// Read the `--capabilities` JSON. The protobuf JSON mapping omits a
    /// zero, an empty list, and an unset optional, so each reads as its
    /// default here.
    fn parse(json: &Value) -> Result<Self, String> {
        let text = |key: &str| {
            json.get(key)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or_else(|| format!("--capabilities has no string {key}"))
        };
        let count = |key: &str| match json.get(key) {
            None => Ok(0),
            Some(v) => v
                .as_u64()
                .ok_or_else(|| format!("--capabilities {key} is not an integer")),
        };
        let optional = |key: &str| match json.get(key) {
            None => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| format!("--capabilities {key} is not an integer")),
        };
        let features = match json.get("features") {
            None => &[][..],
            Some(v) => v
                .as_array()
                .ok_or("--capabilities features is not an array")?,
        }
        .iter()
        .map(|f| {
            f.as_str()
                .map(str::to_owned)
                .ok_or("a feature is not a string")
        })
        .collect::<Result<Vec<_>, _>>()?;
        Ok(Self {
            backend: text("backend")?,
            algorithm: text("algorithm")?,
            max_nodes: count("maxNodes")?,
            max_edges: count("maxEdges")?,
            features,
            max_degree: optional("maxDegree")?,
            max_local_field_milli: optional("maxLocalFieldMilli")?,
            exact_resolution_milli: optional("exactResolutionMilli")?,
        })
    }

    fn lists(&self, feature: &str) -> bool {
        self.features.iter().any(|f| f == feature)
    }

    /// A bounded limit: `Some(n)` with `n > 0`.
    fn bound(limit: Option<u64>) -> Option<u64> {
        limit.filter(|&n| n != 0)
    }

    /// The advertised exact step in milli, when there is one.
    fn step(&self) -> Option<u64> {
        Self::bound(self.exact_resolution_milli)
    }

    /// The coefficient the gate builds models from, in milli: one unit, or
    /// the exact step when one unit is off it.
    fn unit(&self) -> u64 {
        match self.step() {
            Some(step) if 1000 % step != 0 => step,
            _ => 1000,
        }
    }

    /// Why a model of `nodes`, `edges`, highest degree `degree` and highest
    /// local field `field` (milli) is outside the advertised limits, if it is.
    fn excess(&self, nodes: usize, edges: usize, degree: u64, field: f64) -> Option<String> {
        let as_u64 = |n: usize| u64::try_from(n).unwrap_or(u64::MAX);
        if self.max_nodes != 0 && as_u64(nodes) > self.max_nodes {
            return Some(format!("{nodes} nodes exceed maxNodes {}", self.max_nodes));
        }
        if self.max_edges != 0 && as_u64(edges) > self.max_edges {
            return Some(format!("{edges} edges exceed maxEdges {}", self.max_edges));
        }
        if let Some(limit) = Self::bound(self.max_degree) {
            if degree > limit {
                return Some(format!("degree {degree} exceeds maxDegree {limit}"));
            }
        }
        if let Some(limit) = Self::bound(self.max_local_field_milli) {
            #[expect(
                clippy::cast_precision_loss,
                reason = "an advertised u32 field bound is exact in f64"
            )]
            let limit_f = limit as f64;
            if field > limit_f {
                return Some(format!(
                    "local field {field} exceeds maxLocalFieldMilli {limit}"
                ));
            }
        }
        None
    }
}

/// One `--solve` problem, coefficients in milli.
#[derive(Debug, Clone, PartialEq)]
struct Problem {
    h_milli: Vec<f64>,
    j_milli: Vec<f64>,
    edges: Vec<(usize, usize)>,
    num_reads: usize,
    schedule: Option<&'static str>,
}

impl Problem {
    fn new(h_milli: Vec<f64>, j_milli: Vec<f64>, edges: Vec<(usize, usize)>) -> Self {
        Self {
            h_milli,
            j_milli,
            edges,
            num_reads: 4,
            schedule: None,
        }
    }

    fn units(milli: &[f64]) -> Vec<f64> {
        milli.iter().map(|m| m / 1000.0).collect()
    }

    fn to_json(&self) -> Value {
        let mut v = json!({
            "h": Self::units(&self.h_milli),
            "j": Self::units(&self.j_milli),
            "edges": self.edges,
            "num_reads": self.num_reads,
            "num_sweeps": 64,
            "sweeps_per_beta": 1,
            "beta_range": null,
            "seed": BASE_SEED,
        });
        if let (Some(schedule), Some(object)) = (self.schedule, v.as_object_mut()) {
            let _ = object.insert("beta_schedule_type".into(), json!(schedule));
        }
        v
    }

    fn bytes(&self) -> Vec<u8> {
        self.to_json().to_string().into_bytes()
    }

    /// Highest degree and highest local field (milli) over all nodes.
    fn degree_and_field(&self) -> (u64, f64) {
        let mut degree = vec![0_u64; self.h_milli.len()];
        let mut field: Vec<f64> = self.h_milli.iter().map(|v| v.abs()).collect();
        for (&(u, v), j) in self.edges.iter().zip(&self.j_milli) {
            for node in [u, v] {
                if let (Some(d), Some(f)) = (degree.get_mut(node), field.get_mut(node)) {
                    *d += 1;
                    *f += j.abs();
                }
            }
        }
        (
            degree.into_iter().max().unwrap_or(0),
            field.into_iter().fold(0.0, f64::max),
        )
    }

    fn excess(&self, adv: &Advertised) -> Option<String> {
        let (degree, field) = self.degree_and_field();
        adv.excess(self.h_milli.len(), self.edges.len(), degree, field)
    }
}

/// An eight-node ring with one biased node, built on the solver's unit.
fn base_problem(adv: &Advertised) -> Problem {
    #[expect(
        clippy::cast_precision_loss,
        reason = "the unit is at most an advertised u32 step"
    )]
    let unit = adv.unit() as f64;
    let mut h = vec![0.0; BASE_NODES];
    if let Some(first) = h.first_mut() {
        *first = unit;
    }
    let edges: Vec<(usize, usize)> = (0..BASE_NODES).map(|i| (i, (i + 1) % BASE_NODES)).collect();
    let j = (0..BASE_NODES)
        .map(|i| if i % 2 == 0 { unit } else { -unit })
        .collect();
    Problem::new(h, j, edges)
}

/// A star: node 0 joined to `leaves` others by couplings of `coupling` milli.
fn star(leaves: usize, coupling: f64) -> Problem {
    Problem::new(
        vec![0.0; leaves + 1],
        vec![coupling; leaves],
        (1..=leaves).map(|leaf| (0, leaf)).collect(),
    )
}

/// Two nodes and one coupling: bias `h0` on node 0, coupling `coupling`.
fn pair(h0: f64, coupling: f64) -> Problem {
    Problem::new(vec![h0, 0.0], vec![coupling], vec![(0, 1)])
}

/// What one process run produced.
#[derive(Debug)]
struct Run {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: String,
}

impl Run {
    /// The tail of stderr, for a failure message.
    fn stderr_tail(&self) -> String {
        let tail: String = self
            .stderr
            .chars()
            .rev()
            .take(400)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect();
        tail.trim().to_owned()
    }
}

async fn run(bin_path: &str, flag: &str, stdin: &[u8]) -> Result<Run, String> {
    let mut child = Command::new(bin_path)
        .arg(flag)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| format!("cannot spawn {bin_path}: {e}"))?;
    if let Some(mut pipe) = child.stdin.take() {
        // A solver that exits before reading all of stdin closes the pipe;
        // its exit code, not the write, is what the case grades.
        let _ = pipe.write_all(stdin).await;
        drop(pipe);
    }
    let output = tokio::time::timeout(RUN_TIMEOUT, child.wait_with_output())
        .await
        .map_err(|_| format!("{flag} did not exit within {RUN_TIMEOUT:?}"))?
        .map_err(|e| format!("cannot wait for {bin_path}: {e}"))?;
    Ok(Run {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
    })
}

/// The `(spins, energy_milli)` of each read, after checking every output field
/// against the problem and the advertised identity.
type Reads = Vec<(Vec<i8>, i64)>;

fn check_solutions(adv: &Advertised, problem: &Problem, run: &Run) -> Result<Reads, String> {
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
    let h = Problem::units(&problem.h_milli);
    let j = Problem::units(&problem.j_milli);
    let expected_sampler = json!({
        "backend": adv.backend,
        "algorithm": adv.algorithm,
        "num_reads": problem.num_reads,
        "num_sweeps": 64,
        "sweeps_per_beta": 1,
        "beta_range": null,
        "beta_schedule_type": problem.schedule.unwrap_or("geometric"),
        "seed": BASE_SEED,
    });
    let mut graded = Vec::with_capacity(reads.len());
    for (index, read) in reads.iter().enumerate() {
        let spins: Vec<i8> = read
            .get("spins")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("solution {index} has no spins array"))?
            .iter()
            .map(|s| match s.as_i64() {
                Some(1) => Ok(1),
                Some(-1) => Ok(-1),
                _ => Err(format!(
                    "solution {index} has a spin other than -1 or +1: {s}"
                )),
            })
            .collect::<Result<_, _>>()?;
        if spins.len() != problem.h_milli.len() {
            return Err(format!(
                "solution {index} has {} spins for {} nodes",
                spins.len(),
                problem.h_milli.len()
            ));
        }
        let reported = read
            .get("energy_milli")
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("solution {index} has no integer energy_milli"))?;
        let rescored = energy_milli(&spins, &h, &j, &problem.edges);
        if reported != rescored {
            return Err(format!(
                "solution {index} reports energy_milli {reported}, its spins score {rescored}"
            ));
        }
        let timing = read
            .get("timing")
            .ok_or_else(|| format!("solution {index} has no timing"))?;
        let micros = |key: &str| {
            timing
                .get(key)
                .and_then(Value::as_u64)
                .ok_or_else(|| format!("solution {index} timing has no integer {key}"))
        };
        if micros("sample_us")? > micros("total_us")? {
            return Err(format!("solution {index} sample_us exceeds total_us"));
        }
        if read.get("sampler") != Some(&expected_sampler) {
            return Err(format!(
                "solution {index} sampler is {}, expected {expected_sampler}",
                read.get("sampler").unwrap_or(&Value::Null)
            ));
        }
        graded.push((spins, reported));
    }
    Ok(graded)
}

fn check_refused(run: &Run, code: i32) -> Result<(), String> {
    if run.code != Some(code) {
        return Err(format!(
            "exited {:?}, expected {code}; stderr: {}",
            run.code,
            run.stderr_tail()
        ));
    }
    if !String::from_utf8_lossy(&run.stdout).trim().is_empty() {
        return Err("a refused problem wrote solutions to stdout".into());
    }
    Ok(())
}

/// Solve `problem` and require valid solutions.
async fn expect_solved(bin: &str, adv: &Advertised, problem: &Problem) -> Result<Reads, String> {
    let run = run(bin, "--solve", &problem.bytes()).await?;
    check_solutions(adv, problem, &run)
}

/// Send `input` and require exit `code` with nothing on stdout.
async fn expect_refused(bin: &str, input: &[u8], code: i32) -> Result<(), String> {
    let run = run(bin, "--solve", input).await?;
    check_refused(&run, code)
}

/// The base problem with its JSON edited by `edit`.
fn edited(base: &Problem, edit: impl FnOnce(&mut serde_json::Map<String, Value>)) -> Vec<u8> {
    let mut v = base.to_json();
    if let Some(object) = v.as_object_mut() {
        edit(object);
    }
    v.to_string().into_bytes()
}

/// Run the `--solve` gate against `bin_path`.
pub async fn drive_solve(bin_path: &str) -> SolveReport {
    let mut report = SolveReport::default();
    let adv = match run(bin_path, "--capabilities", b"").await.and_then(|run| {
        if run.code != Some(0) {
            return Err(format!("--capabilities exited {:?}", run.code));
        }
        let json: Value = serde_json::from_slice(&run.stdout)
            .map_err(|e| format!("--capabilities is not JSON: {e}"))?;
        Advertised::parse(&json)
    }) {
        Ok(adv) => {
            report.push("capabilities", CaseOutcome::Passed);
            adv
        }
        Err(why) => {
            report.push("capabilities", CaseOutcome::Failed(why));
            return report;
        }
    };

    let base = base_problem(&adv);
    if let Some(why) = base.excess(&adv) {
        report.push(
            "solve",
            CaseOutcome::Failed(format!(
                "the gate's {BASE_NODES}-node ring is outside the advertised limits: {why}"
            )),
        );
        return report;
    }

    base_cases(bin_path, &adv, &base, &mut report).await;
    schedule_case(bin_path, &adv, &base, &mut report).await;
    degree_cases(bin_path, &adv, &mut report).await;
    field_cases(bin_path, &adv, &mut report).await;
    resolution_cases(bin_path, &adv, &mut report).await;
    report
}

async fn base_cases(bin: &str, adv: &Advertised, base: &Problem, report: &mut SolveReport) {
    let first = expect_solved(bin, adv, base).await;
    let same_seed = match &first {
        Ok(reads) => match expect_solved(bin, adv, base).await {
            Ok(again) if &again == reads => Ok(()),
            Ok(_) => Err("the same seed gave different samples".to_owned()),
            Err(why) => Err(why),
        },
        Err(_) => Err("the base problem did not solve".to_owned()),
    };
    report.grade("solve", first.map(|_| ()));
    report.grade("same-seed", same_seed);

    let malformed: [(&'static str, Vec<u8>); 9] = [
        ("malformed-json", b"not json".to_vec()),
        (
            "unknown-key",
            edited(base, |o| {
                let _ = o.insert("num_sweep".into(), json!(1));
            }),
        ),
        (
            "missing-key",
            edited(base, |o| {
                let _ = o.remove("seed");
            }),
        ),
        (
            "self-loop",
            edited(base, |o| {
                let _ = o.insert("edges".into(), json!([[0, 1], [1, 1]]));
                let _ = o.insert("j".into(), json!([1.0, 1.0]));
            }),
        ),
        (
            "endpoint-out-of-range",
            edited(base, |o| {
                let _ = o.insert("edges".into(), json!([[0, BASE_NODES]]));
                let _ = o.insert("j".into(), json!([1.0]));
            }),
        ),
        (
            "couplings-mismatch",
            edited(base, |o| {
                let _ = o.insert("j".into(), json!([1.0]));
            }),
        ),
        (
            "zero-reads",
            edited(base, |o| {
                let _ = o.insert("num_reads".into(), json!(0));
            }),
        ),
        (
            "zero-sweeps-per-beta",
            edited(base, |o| {
                let _ = o.insert("sweeps_per_beta".into(), json!(0));
            }),
        ),
        (
            "unknown-schedule",
            edited(base, |o| {
                let _ = o.insert("beta_schedule_type".into(), json!("cosine"));
            }),
        ),
    ];
    for (name, input) in malformed {
        report.grade(name, expect_refused(bin, &input, EXIT_CONFIG_INVALID).await);
    }
}

async fn schedule_case(bin: &str, adv: &Advertised, base: &Problem, report: &mut SolveReport) {
    let linear = Problem {
        schedule: Some("linear"),
        ..base.clone()
    };
    let result = if adv.lists(LINEAR_SCHEDULE_FEATURE) {
        expect_solved(bin, adv, &linear).await.map(|_| ())
    } else {
        // Without the feature the solver would anneal geometric and say
        // nothing, so it must refuse the key as an older binary does.
        expect_refused(bin, &linear.bytes(), EXIT_CONFIG_INVALID).await
    };
    report.grade("linear-schedule", result);
}

/// A model just inside the limits solves; one just outside is capacity.
async fn limit_pair(
    bin: &str,
    adv: &Advertised,
    inside: &Problem,
    outside: &Problem,
) -> (Result<(), String>, Result<(), String>) {
    let inside_result = match inside.excess(adv) {
        Some(why) => Err(format!("the gate built an inside model that is not: {why}")),
        None => expect_solved(bin, adv, inside).await.map(|_| ()),
    };
    let outside_result = expect_refused(bin, &outside.bytes(), EXIT_CAPACITY).await;
    (inside_result, outside_result)
}

async fn degree_cases(bin: &str, adv: &Advertised, report: &mut SolveReport) {
    const NAMES: (&str, &str) = ("degree-limit-inside", "degree-limit-outside");
    let Some(limit) = Advertised::bound(adv.max_degree) else {
        skip_pair(report, NAMES, "maxDegree is not advertised or is unbounded");
        return;
    };
    let Ok(leaves) = usize::try_from(limit) else {
        skip_pair(
            report,
            NAMES,
            "maxDegree does not fit this platform's usize",
        );
        return;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "the unit is at most an advertised u32 step"
    )]
    let unit = adv.unit() as f64;
    let inside = star(leaves, unit);
    let outside = star(leaves + 1, unit);
    if let Some(why) = inside.excess(adv) {
        skip_pair(
            report,
            NAMES,
            &format!("a star of degree {limit} is outside another limit: {why}"),
        );
        return;
    }
    let (inside, outside) = limit_pair(bin, adv, &inside, &outside).await;
    report.grade(NAMES.0, inside);
    report.grade(NAMES.1, outside);
}

async fn field_cases(bin: &str, adv: &Advertised, report: &mut SolveReport) {
    const NAMES: (&str, &str) = ("field-limit-inside", "field-limit-outside");
    let Some(limit) = Advertised::bound(adv.max_local_field_milli) else {
        skip_pair(
            report,
            NAMES,
            "maxLocalFieldMilli is not advertised or is unbounded",
        );
        return;
    };
    // Build on the exact step so an exact solver does not refuse the inside
    // model for its resolution: the largest field on the step that fits, and
    // one step more.
    let step = adv.step().unwrap_or(1);
    let fits = limit / step * step;
    if fits < step {
        skip_pair(
            report,
            NAMES,
            "no field on the exact step fits the field limit",
        );
        return;
    }
    if fits > u64::from(i32::MAX.unsigned_abs()) {
        skip_pair(
            report,
            NAMES,
            "the field limit is beyond one i32 milli coefficient",
        );
        return;
    }
    #[expect(
        clippy::cast_precision_loss,
        reason = "fits and step are at most i32::MAX here, exact in f64"
    )]
    let (fits, step) = (fits as f64, step as f64);
    let inside = pair(fits - step, step);
    let outside = pair(fits, step);
    let (inside, outside) = limit_pair(bin, adv, &inside, &outside).await;
    report.grade(NAMES.0, inside);
    report.grade(NAMES.1, outside);
}

async fn resolution_cases(bin: &str, adv: &Advertised, report: &mut SolveReport) {
    const NAMES: (&str, &str) = ("resolution-limit-inside", "resolution-limit-outside");
    if !adv.lists(EXACT_COEFFICIENTS_FEATURE) {
        skip_pair(
            report,
            NAMES,
            "exact-coefficients is not listed, so off-step values are quantized",
        );
        return;
    }
    let Some(step) = adv.step() else {
        skip_pair(report, NAMES, "exactResolutionMilli is not advertised");
        return;
    };
    #[expect(
        clippy::cast_precision_loss,
        reason = "an advertised u32 step is exact in f64"
    )]
    let step = step as f64;
    // One milli off the step, or half a milli when the step is one milli.
    let off = if step > 1.0 { 1.0 } else { 0.5 };
    let inside = pair(step, step);
    let outside = pair(step + off, step);
    if let Some(why) = outside.excess(adv) {
        skip_pair(
            report,
            NAMES,
            &format!("a one-step model is outside another limit: {why}"),
        );
        return;
    }
    let (inside, outside) = limit_pair(bin, adv, &inside, &outside).await;
    report.grade(NAMES.0, inside);
    report.grade(NAMES.1, outside);
}

fn skip_pair(report: &mut SolveReport, names: (&'static str, &'static str), why: &str) {
    report.push(names.0, CaseOutcome::Skipped(why.to_owned()));
    report.push(names.1, CaseOutcome::Skipped(why.to_owned()));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adv() -> Advertised {
        Advertised {
            backend: "mock".into(),
            algorithm: "sa".into(),
            max_nodes: 0,
            max_edges: 0,
            ..Advertised::default()
        }
    }

    #[test]
    fn capabilities_parse_reads_optional_limits() {
        let parsed = Advertised::parse(&json!({
            "backend": "cuda", "algorithm": "msa", "maxNodes": 5000, "maxEdges": 9,
            "features": ["linear-schedule"], "maxDegree": 20, "exactResolutionMilli": 1000,
        }))
        .expect("parses");
        assert_eq!(parsed.max_degree, Some(20));
        assert_eq!(parsed.max_local_field_milli, None);
        assert_eq!(parsed.exact_resolution_milli, Some(1000));
        assert!(parsed.lists(LINEAR_SCHEDULE_FEATURE));
        assert!(Advertised::parse(&json!({"algorithm": "sa"})).is_err());
        assert!(
            Advertised::parse(&json!({"backend": "cpu", "algorithm": "sa", "features": 1}))
                .is_err()
        );
        // The protobuf JSON mapping omits zeros and empty lists.
        let minimal =
            Advertised::parse(&json!({"backend": "cpu", "algorithm": "sa"})).expect("defaults");
        assert_eq!((minimal.max_nodes, minimal.features.len()), (0, 0));
    }

    #[test]
    fn the_base_ring_has_degree_two_and_field_three_units() {
        let ring = base_problem(&adv());
        assert_eq!(ring.degree_and_field(), (2, 3000.0));
        assert_eq!(ring.edges.len(), BASE_NODES);
    }

    #[test]
    fn the_unit_follows_a_step_that_does_not_divide_one_unit() {
        let mut a = adv();
        a.exact_resolution_milli = Some(250);
        assert_eq!(a.unit(), 1000);
        a.exact_resolution_milli = Some(3);
        assert_eq!(a.unit(), 3);
    }

    #[test]
    fn a_star_has_the_degree_of_its_leaf_count() {
        assert_eq!(star(20, 1000.0).degree_and_field(), (20, 20_000.0));
    }

    #[test]
    fn excess_names_the_limit() {
        let mut a = adv();
        a.max_degree = Some(2);
        a.max_local_field_milli = Some(2500);
        let ring = base_problem(&adv());
        assert!(ring
            .excess(&a)
            .is_some_and(|why| why.contains("local field")));
        a.max_local_field_milli = Some(0);
        assert!(ring.excess(&a).is_none());
        assert!(star(3, 1000.0)
            .excess(&a)
            .is_some_and(|why| why.contains("degree 3")));
    }

    #[test]
    fn problem_json_carries_units_and_an_optional_schedule() {
        let p = Problem {
            schedule: Some("linear"),
            ..pair(1500.0, -1000.0)
        };
        let v = p.to_json();
        assert_eq!(v.get("h"), Some(&json!([1.5, 0.0])));
        assert_eq!(v.get("j"), Some(&json!([-1.0])));
        assert_eq!(v.get("beta_schedule_type"), Some(&json!("linear")));
        assert!(pair(0.0, 0.0).to_json().get("beta_schedule_type").is_none());
    }

    fn ok_run(stdout: &Value) -> Run {
        Run {
            code: Some(0),
            stdout: stdout.to_string().into_bytes(),
            stderr: String::new(),
        }
    }

    fn solution(spins: &[i8], energy: i64, schedule: &str) -> Value {
        json!({
            "spins": spins,
            "energy_milli": energy,
            "timing": {"sample_us": 1, "total_us": 2},
            "sampler": {
                "backend": "mock", "algorithm": "sa", "num_reads": 1, "num_sweeps": 64,
                "sweeps_per_beta": 1, "beta_range": null, "beta_schedule_type": schedule,
                "seed": BASE_SEED,
            },
        })
    }

    #[test]
    fn check_solutions_accepts_a_correct_read_and_names_each_defect() {
        let p = Problem {
            num_reads: 1,
            ..pair(1000.0, 1000.0)
        };
        // All +1: h0 + J = 2 units.
        let good = json!([solution(&[1, 1], 2000, "geometric")]);
        assert!(check_solutions(&adv(), &p, &ok_run(&good)).is_ok());

        let wrong_energy = json!([solution(&[1, 1], 0, "geometric")]);
        let err = check_solutions(&adv(), &p, &ok_run(&wrong_energy)).expect_err("energy");
        assert!(err.contains("score 2000"), "{err}");

        let wrong_schedule = json!([solution(&[1, 1], 2000, "linear")]);
        let err = check_solutions(&adv(), &p, &ok_run(&wrong_schedule)).expect_err("schedule");
        assert!(err.contains("sampler"), "{err}");

        let bad_spin = json!([solution(&[1, 0], 1000, "geometric")]);
        assert!(check_solutions(&adv(), &p, &ok_run(&bad_spin)).is_err());

        let too_many = json!([
            solution(&[1, 1], 2000, "geometric"),
            solution(&[1, 1], 2000, "geometric")
        ]);
        assert!(check_solutions(&adv(), &p, &ok_run(&too_many)).is_err());

        let mut no_meta = solution(&[1, 1], 2000, "geometric");
        let _ = no_meta.as_object_mut().map(|o| o.remove("timing"));
        assert!(check_solutions(&adv(), &p, &ok_run(&json!([no_meta]))).is_err());
    }

    #[test]
    fn check_refused_requires_the_code_and_an_empty_stdout() {
        let refused = Run {
            code: Some(65),
            stdout: Vec::new(),
            stderr: String::new(),
        };
        assert!(check_refused(&refused, EXIT_CAPACITY).is_ok());
        assert!(check_refused(&refused, EXIT_CONFIG_INVALID).is_err());
        let chatty = Run {
            stdout: b"[]".to_vec(),
            ..refused
        };
        assert!(check_refused(&chatty, EXIT_CAPACITY).is_err());
    }

    #[test]
    fn a_report_with_a_failure_is_not_conformant_but_skips_are_fine() {
        let mut report = SolveReport::default();
        assert!(!report.is_conformant(), "an empty report proves nothing");
        report.push("solve", CaseOutcome::Passed);
        report.push("degree-limit-inside", CaseOutcome::Skipped("n/a".into()));
        assert!(report.is_conformant());
        report.push("self-loop", CaseOutcome::Failed("exited 0".into()));
        assert!(!report.is_conformant());
        assert!(report.summary().contains("[FAIL] self-loop: exited 0"));
        assert_eq!(
            report.outcome("solve"),
            Some(&CaseOutcome::Passed),
            "outcome looks a case up by name"
        );
    }
}
