//! SC-7 conformance of generated instances and family sessions. Library level, no binary.

#[path = "family_conformance/faulty.rs"]
mod faulty;

use faulty::{FAULTY_J_V1, FAULTY_METADATA_V1, FAULTY_OVERSIZE_V1};
use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits, Contract};
use ising_generators::family::{
    admit_topology, generate_instance, BaseTopology, Defect, SaltFault,
};
use ising_generators::instance::Instance;
use ising_generators::random::derive_nonce;
use quip_protocol::lease::salt_with_counter;
use quip_solver_conformance::families::layered::UNIFORM_V1;

/// 2 layers, field -1500.
const P_SPEC: &str = "01010002000000020002000400000024faffff";
/// Lease slot 1 uses counter `5 + 1`. Slot 0 is skipped.
const SLOT_ONE_COUNTER: u64 = 6;

fn hex_decode(hex: &str) -> Result<Vec<u8>, String> {
    (0..hex.len())
        .step_by(2)
        .map(|i| {
            hex.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("bad hex at {i}"))
        })
        .collect()
}

/// The topology `T_SPARSE`.
fn t_sparse() -> Result<BaseTopology, String> {
    let nodes = vec![907, 41, 0, 4_294_967_295, 12, 3];
    let edges = [
        (907, 41),
        (41, 0),
        (0, 4_294_967_295),
        (4_294_967_295, 12),
        (12, 3),
        (3, 907),
        (41, 12),
    ];
    BaseTopology::new(nodes, &edges).map_err(|e| format!("{e:?}"))
}

/// Generate the instance of lease slot 1 for `P_SPEC` on `T_SPARSE` under `contract`.
fn generate_slot_one(contract: Contract) -> Result<Result<Instance, SaltFault>, String> {
    let params = hex_decode(P_SPEC)?;
    let contracts = [contract];
    let admitted = admit(
        &contracts,
        contract.family_id.as_bytes(),
        contract.revision,
        &params,
        &params_digest(&params),
    )
    .map_err(|c| format!("admit failed with {c}"))?;
    let topology = t_sparse()?;
    let bounds = admit_topology(
        &admitted,
        &topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
    .map_err(|c| format!("admit_topology failed with {c}"))?;
    let base_salt = {
        let mut salt = [0x33_u8; 32];
        salt[..8].fill(0);
        salt
    };
    let salt = salt_with_counter(&base_salt, SLOT_ONE_COUNTER);
    let nonce = derive_nonce([0x11; 32], [0x22; 32], salt);
    Ok(generate_instance(&admitted, &topology, &bounds, nonce))
}

fn honest_and_faulty(faulty: Contract) -> Result<(Instance, Instance), String> {
    let honest = generate_slot_one(UNIFORM_V1)?.map_err(|f| format!("honest: {f:?}"))?;
    let broken = generate_slot_one(faulty)?.map_err(|f| format!("faulty: {f:?}"))?;
    Ok((honest, broken))
}

/// The all-+1 energy: offset plus the sum of h plus the sum of J, in milli.
fn all_up_energy(instance: &Instance) -> i64 {
    let h: i64 = instance.h().iter().copied().map(i64::from).sum();
    let j: i64 = instance.j().iter().copied().map(i64::from).sum();
    instance.offset_milli() + h + j
}

fn canonical(instance: &Instance) -> Result<Vec<u8>, String> {
    instance.canonical_bytes().map_err(|e| e.to_string())
}

#[test]
fn faulty_j_keeps_the_all_up_energy_and_changes_the_couplings() -> Result<(), String> {
    let (honest, broken) = honest_and_faulty(FAULTY_J_V1)?;
    let (a, b) = (all_up_energy(&honest), all_up_energy(&broken));
    if a != b {
        return Err(format!("energies differ: {a} and {b}"));
    }
    let sum = |i: &Instance| i.j().iter().copied().map(i64::from).sum::<i64>();
    if sum(&honest) != sum(&broken) {
        return Err("sum of J differs".to_owned());
    }
    if honest.j() == broken.j() {
        return Err("the couplings are equal at slot 1".to_owned());
    }
    if canonical(&honest)? == canonical(&broken)? {
        return Err("canonical bytes are equal".to_owned());
    }
    Ok(())
}

#[test]
fn faulty_metadata_keeps_the_energy_and_changes_the_region() -> Result<(), String> {
    let (honest, broken) = honest_and_faulty(FAULTY_METADATA_V1)?;
    let (a, b) = (all_up_energy(&honest), all_up_energy(&broken));
    if a != b {
        return Err(format!("energies differ: {a} and {b}"));
    }
    if honest.metadata() == broken.metadata() {
        return Err("the metadata regions are equal".to_owned());
    }
    if canonical(&honest)? == canonical(&broken)? {
        return Err("canonical bytes are equal".to_owned());
    }
    Ok(())
}

#[test]
fn faulty_oversize_is_a_size_bound_defect() -> Result<(), String> {
    match generate_slot_one(FAULTY_OVERSIZE_V1)? {
        Err(SaltFault::Defect(Defect::SizeBound)) => Ok(()),
        other => Err(format!("expected a size bound defect, got {other:?}")),
    }
}
