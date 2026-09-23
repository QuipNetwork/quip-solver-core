//! Public graph literals, trait defaults, entry-point inference, and warm streams.
use quip_solver_core::coefficient::{Fixed, Milli};
use quip_solver_core::{
    run, run_code, BackendIdentity, CancelToken, CommonArgs, IsingGraph, OpenError, SampleError,
    SampleParams, Sampler, SamplerResult, StreamJob, StreamResult, WarmStreamJob,
};

struct DefaultSampler;
impl Sampler for DefaultSampler {
    #[expect(
        clippy::panic_in_result_fn,
        reason = "test sampler asserts that graph coefficients preserve their exact bits"
    )]
    fn sample(
        &self,
        graph: &IsingGraph,
        _: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        assert_eq!(
            graph.h.first().copied().map(f64::to_bits),
            Some(0.0004_f64.to_bits())
        );
        Ok(Vec::new())
    }
}

struct NarrowSampler;
impl Sampler<Fixed<i8, 1>> for NarrowSampler {
    fn sample(
        &self,
        _: &IsingGraph<Fixed<i8, 1>>,
        _: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        Ok(Vec::new())
    }
    fn accepts_warm_start() -> bool {
        true
    }
}

// Taking these function items type-checks the real entry points without I/O.
#[test]
#[expect(
    clippy::no_effect_underscore_binding,
    reason = "closures type-check entry-point inference without starting I/O"
)]
fn entry_points_infer_the_coefficient() {
    type OpenDefault = fn() -> Result<DefaultSampler, OpenError>;
    type OpenNarrow = fn() -> Result<NarrowSampler, OpenError>;
    let _default_run =
        |id: BackendIdentity, common: &CommonArgs, open: OpenDefault| run(id, common, open);
    let _default_code =
        |id: BackendIdentity, common: &CommonArgs, open: OpenDefault| run_code(id, common, open);
    let _narrow_run =
        |id: BackendIdentity, common: &CommonArgs, open: OpenNarrow| run(id, common, open);
    let _narrow_code =
        |id: BackendIdentity, common: &CommonArgs, open: OpenNarrow| run_code(id, common, open);
}

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions check behavior while Result propagates unexpected errors"
)]
fn default_graph_and_json_keep_sub_milli_values() -> Result<(), Box<dyn std::error::Error>> {
    let graph = IsingGraph {
        h: vec![0.0004],
        j: vec![],
        edges: vec![],
    };
    assert!(DefaultSampler
        .sample(&graph, &SampleParams::default())
        .map_err(quip_solver_core::driver::SolveError::from)?
        .is_empty());
    let bytes = br#"{"h":[0.0004],"j":[],"edges":[],"num_reads":1,
        "num_sweeps":1,"sweeps_per_beta":1,"beta_range":null,"seed":0}"#;
    assert_eq!(
        quip_solver_core::driver::solve(&DefaultSampler, bytes)?,
        b"[]"
    );
    let milli = IsingGraph::<Milli> {
        h: vec![Fixed(1)],
        j: vec![],
        edges: vec![],
    };
    assert_eq!(milli.num_nodes(), 1);
    Ok(())
}

#[test]
fn main_constructor_still_infers_f64() {
    // Written exactly as main writes it: no annotation, empty vectors.
    let empty = IsingGraph::new(vec![], vec![], vec![]);
    let typed: &IsingGraph<f64> = &empty;
    assert_eq!(typed.num_nodes(), 0);
    let graph = IsingGraph::new(vec![1.0, -1.0], vec![0.5], vec![(0, 1)]);
    assert_eq!(graph.h, vec![1.0, -1.0]);
}

#[test]
fn narrow_stream_signatures_are_callable() {
    let (plain_tx, plain_rx) = tokio::sync::mpsc::channel::<StreamJob<Fixed<i8, 1>>>(1);
    let (warm_tx, warm_rx) = tokio::sync::mpsc::channel::<WarmStreamJob<Fixed<i8, 1>>>(1);
    let (out, _received) = tokio::sync::mpsc::channel::<StreamResult>(1);
    drop(plain_tx);
    drop(warm_tx);
    NarrowSampler.sample_stream(plain_rx, out.clone(), CancelToken::default());
    NarrowSampler.sample_stream_warm(warm_rx, out, CancelToken::default());
}
