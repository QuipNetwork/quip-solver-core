//! The `feasible` helper: a soft family, a hard family with a hook, and a
//! contract without a construction.

#![expect(
    clippy::panic_in_result_fn,
    reason = "test code: a failed assertion is a test failure"
)]

use ising_generators::descriptor::{
    admit, params_digest, Admitted, AdvertisedLimits, Contract, Params, SizeBounds, TopologyStats,
};
use ising_generators::family::{
    admit_topology, feasible, generate_instance, BaseTopology, Construction, InstanceBody, Streams,
};
use ising_generators::instance::{Instance, InstanceLimits};
use ising_generators::Code;

const PARAMS: &[u8] = &[0x01];

fn accept_topology(_: &Params<'_>, _: &BaseTopology) -> bool {
    true
}

fn accept_metadata(_: &Params<'_>, _: &Instance) -> bool {
    true
}

#[expect(clippy::unnecessary_wraps, reason = "must match GenerateFn")]
fn generate(_: &Params<'_>, _: &BaseTopology, _: &mut Streams<'_>) -> Result<InstanceBody, Code> {
    Ok(InstanceBody {
        node_ids: vec![0, 1],
        edges: vec![(0, 1)],
        h: vec![0, 0],
        j: vec![1000],
        offset_milli: 0,
        metadata: vec![0, 0],
    })
}

#[expect(clippy::unnecessary_wraps, reason = "must match SizeBoundsFn")]
fn bounds(_: &Params<'_>, _: &TopologyStats) -> Result<SizeBounds, Code> {
    Ok(SizeBounds {
        nodes: 2,
        edges: 1,
        degree: 1,
    })
}

/// A hook that accepts only a witness whose two spins are equal.
fn equal_spins(_: &Params<'_>, _: &Instance, spins: &[i8]) -> bool {
    spins.first() == spins.get(1)
}

const fn construction(feasible: Option<ising_generators::family::FeasibleFn>) -> Construction {
    Construction {
        labels: &[],
        check_topology: accept_topology,
        generate,
        check_metadata: accept_metadata,
        feasible,
    }
}

static SOFT: Construction = construction(None);
static HARD: Construction = construction(Some(equal_spins));

const fn contract(construction: Option<&'static Construction>) -> Contract {
    Contract {
        family_id: "vectors/feasible",
        revision: 1,
        schema: &[],
        metadata_kinds: &[],
        limits: InstanceLimits::FAM_37,
        size_bounds: bounds,
        construction,
    }
}

fn admitted(contracts: &[Contract]) -> Result<Admitted<'_>, String> {
    admit(
        contracts,
        b"vectors/feasible",
        1,
        PARAMS,
        &params_digest(PARAMS),
    )
    .map_err(|c| format!("admit failed with {c}"))
}

fn instance_of(admitted: &Admitted<'_>) -> Result<Instance, String> {
    let topology = BaseTopology::new(vec![0, 1], &[(0, 1)]).map_err(|e| format!("{e:?}"))?;
    let bounds = admit_topology(
        admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
    .map_err(|c| format!("admit_topology failed with {c}"))?;
    generate_instance(admitted, &topology, &bounds, [3; 32]).map_err(|e| format!("{e:?}"))
}

#[test]
fn a_soft_family_accepts_any_witness_of_the_right_length() -> Result<(), String> {
    let contracts = [contract(Some(&SOFT))];
    let admitted = admitted(&contracts)?;
    let instance = instance_of(&admitted)?;
    for spins in [[1i8, 1], [1, -1], [-1, 1], [-1, -1]] {
        assert!(feasible(&admitted, &instance, &spins));
    }
    Ok(())
}

#[test]
fn a_soft_family_rejects_a_witness_that_is_not_one_spin_per_node() -> Result<(), String> {
    let contracts = [contract(Some(&SOFT))];
    let admitted = admitted(&contracts)?;
    let instance = instance_of(&admitted)?;
    assert!(!feasible(&admitted, &instance, &[1]));
    assert!(!feasible(&admitted, &instance, &[1, 1, 1]));
    assert!(!feasible(&admitted, &instance, &[1, 0]));
    assert!(!feasible(&admitted, &instance, &[2, 1]));
    Ok(())
}

#[test]
fn a_hard_family_returns_the_hook_result() -> Result<(), String> {
    let contracts = [contract(Some(&HARD))];
    let admitted = admitted(&contracts)?;
    let instance = instance_of(&admitted)?;
    assert!(feasible(&admitted, &instance, &[1, 1]));
    assert!(feasible(&admitted, &instance, &[-1, -1]));
    assert!(!feasible(&admitted, &instance, &[1, -1]));
    Ok(())
}

#[test]
fn a_contract_without_a_construction_is_never_feasible() -> Result<(), String> {
    let built = [contract(Some(&SOFT))];
    let built = admitted(&built)?;
    let instance = instance_of(&built)?;
    let none = [contract(None)];
    let none = admitted(&none)?;
    assert!(!feasible(&none, &instance, &[1, 1]));
    Ok(())
}
