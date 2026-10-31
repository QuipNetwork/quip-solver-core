//! Test-support miner that declares family support: the same trivial sampler
//! as `mock_sampler_miner`, with `solver_id` `quip-mock/families`, one
//! restricted generated claim and one unrestricted accepted claim. Used by
//! `cli_prints_the_family_fields` and `cli_and_session_answers_agree` in
//! `tests/family_capabilities.rs`.

use clap::Parser;
use quip_protocol::scoring::energy_milli;
use quip_solver_core::families::{Declaration, FamilyClaim};
use quip_solver_core::ising_generators::descriptor::{
    Contract, FieldSpec, Params, SizeBounds, TopologyStats, Value as ParamValue, ValueType,
};
use quip_solver_core::ising_generators::instance::InstanceLimits;
use quip_solver_core::ising_generators::Code;
use quip_solver_core::{
    run, BackendIdentity, CommonArgs, IsingGraph, SampleError, SampleParams, Sampler, SamplerResult,
};
use std::process::ExitCode;

const UNIFORM_FIELD: &[FieldSpec] = &[
    FieldSpec {
        tag: 1,
        kind: ValueType::U16,
        min: 1,
        max: 16,
    },
    FieldSpec {
        tag: 2,
        kind: ValueType::I32,
        min: -4000,
        max: 4000,
    },
];

const ZERO_FIELD_PARAMS: [u8; 19] = [0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];

fn example_size_bounds(params: &Params<'_>, stats: &TopologyStats) -> Result<SizeBounds, Code> {
    let Some(ParamValue::Unsigned(layers)) = params.get(1) else {
        return Err(Code::ParamsMalformed);
    };
    let overflow = || Code::BoundsExceeded;
    let nodes = layers.checked_mul(stats.n_nodes).ok_or_else(overflow)?;
    let edges = layers
        .checked_mul(stats.n_edges)
        .and_then(|e| e.checked_add(layers.saturating_sub(1).checked_mul(stats.n_nodes)?))
        .ok_or_else(overflow)?;
    let degree = stats
        .max_degree
        .checked_add(layers.saturating_sub(1).min(2))
        .ok_or_else(overflow)?;
    Ok(SizeBounds {
        nodes,
        edges,
        degree,
    })
}

const UNIFORM_V1: Contract = Contract {
    family_id: "vectors/uniform-field",
    revision: 1,
    schema: UNIFORM_FIELD,
    metadata_kinds: &[],
    limits: InstanceLimits::FAM_37,
    size_bounds: example_size_bounds,
};

struct MockSampler;

impl Sampler for MockSampler {
    fn declaration() -> Declaration {
        Declaration {
            solver_id: "quip-mock/families",
            generated: &[FamilyClaim {
                revisions: &[UNIFORM_V1],
                only_params: &[&ZERO_FIELD_PARAMS],
            }],
            accepted: &[FamilyClaim {
                revisions: &[UNIFORM_V1],
                only_params: &[],
            }],
        }
    }

    fn sample(
        &self,
        graph: &IsingGraph,
        params: &SampleParams,
    ) -> Result<Vec<SamplerResult>, SampleError> {
        let spins = vec![1i8; graph.num_nodes()];
        let energy = energy_milli(&spins, &graph.h, &graph.j, &graph.edges);
        Ok((0..params.num_reads)
            .map(|_| SamplerResult {
                spins: spins.clone(),
                energy_milli: energy,
            })
            .collect())
    }
}

static CLI_VERSION: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
    format!(
        "{} protocol {}",
        env!("CARGO_PKG_VERSION"),
        quip_protocol::session::PROTOCOL_VERSION
    )
});

#[derive(Parser)]
#[command(version = CLI_VERSION.as_str())]
struct Cli {
    #[command(flatten)]
    common: CommonArgs,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    run(
        BackendIdentity {
            backend: quip_proto::v1::Backend::Mock,
            algorithm: quip_proto::v1::Algorithm::Sa,
            max_nodes: 100_000,
            max_edges: 1_000_000,
            features: &[],
            adapt: quip_solver_core::adapt::AdaptBounds {
                min_sweeps: 64,
                max_sweeps: 4096,
                min_reads: 64,
                max_reads: 512,
                reads_solution_min_factor: 4,
                reads_solution_max_factor: 8,
                reads_solution_floor_factor: 0,
            },
            limits: quip_solver_core::ModelLimits::UNSTATED,
        },
        &cli.common,
        || Ok(MockSampler),
    )
}
