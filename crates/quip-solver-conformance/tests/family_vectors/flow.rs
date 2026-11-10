//! Static flow vectors (`flow.json`). Every field of every section is compared with
//! a value that the Rust library or a hand replay of the `flow` stream rebuilds.

use super::{
    built_once, case_name, check_layered_against_rust, check_layered_case, check_names_unique,
    codec, compare, digests, encoded_energy, entries_of, expect_err, expected_code, find_named,
    flip_last_hex_digit, le_u16, le_u32, lease_nonce, mutate, object_field, schema, section,
    str_field, topology_inputs, topology_max_degree, u32_field, u64_array, u64_decimal, validate,
    with_ice_instance,
};
use ising_generators::descriptor::{admit, params_digest, AdvertisedLimits, Params};
use ising_generators::family::{admit_topology, feasible, BaseTopology};
use ising_generators::flow::{flow_network, flow_observables, FLOW, FLOW_RECIPES, FLOW_V1};
use ising_generators::instance::Instance;
use serde_json::{json, Value};

/// The evidence id of every entry of the file and of the registry entry.
const EVIDENCE: &str = "SC-17:flow-vectors-frozen";
/// The coefficient bound of the contract, computed for side 64 and cost 1000.
const COEFFICIENT_BOUND: i64 = 8_320_504;

fn flow_doc() -> Result<Value, String> {
    serde_json::from_str(include_str!("../../vectors/families/flow.json"))
        .map_err(|e| e.to_string())
}

/// `side` and `cost_half_max` of canonical flow params: the two last fields,
/// which have fixed offsets.
fn flow_shape(params: &[u8]) -> Result<(u64, u64), String> {
    if params.len() != 32 {
        return Err(format!("flow params have {} bytes", params.len()));
    }
    Ok((
        u64::from(le_u16(params, 22)?),
        u64::from(le_u16(params, 30)?),
    ))
}

/// The bounds of `quip/flow-static`: the links, six couplers for each vertex
/// of the `(L + 1)` by `(L + 1)` grid, and degree six.
fn flow_size_bounds(
    _: &Value,
    params: &[u8],
    nodes: &[u32],
    edges: &[(u32, u32)],
) -> Result<Option<(u64, u64, u64)>, String> {
    let (side, _) = flow_shape(params)?;
    let n = u64::try_from(nodes.len())
        .map_err(|e| e.to_string())?
        .min(2 * side * (side + 1));
    let e = u64::try_from(edges.len())
        .map_err(|e| e.to_string())?
        .min(6 * (side + 1) * (side + 1));
    let d = topology_max_degree(nodes, edges)
        .min(6)
        .min(n.saturating_sub(1));
    Ok(Some((n, e, d)))
}

/// What the three metadata sections of a built flow instance say, decoded by
/// hand from the layouts of the contract.
struct FlowMeta {
    side: u16,
    labels: Vec<i8>,
    window: (u16, u16),
    positions: Vec<u32>,
    weight: u64,
    targets: Vec<i8>,
    costs: Vec<u32>,
}

fn flow_meta(built: &Instance) -> Result<FlowMeta, String> {
    let region = built.metadata();
    if le_u16(region, 0)? != 3 {
        return Err("the region does not hold three sections".to_owned());
    }
    let mut at = 2;
    let mut payloads = Vec::new();
    for kind in [4u16, 5, 9] {
        if le_u16(region, at)? != kind {
            return Err(format!("the section at {at} is not kind {kind}"));
        }
        let len = usize::try_from(le_u32(region, at + 2)?).map_err(|e| e.to_string())?;
        payloads.push(
            region
                .get(at + 6..at + 6 + len)
                .ok_or("section past the region")?,
        );
        at += 6 + len;
    }
    if at != region.len() {
        return Err("trailing bytes in the region".to_owned());
    }
    let [charges, incidence, flow] = payloads[..] else {
        return Err("three payloads".to_owned());
    };
    let weight = u64::from_le_bytes(
        flow.get(0..8)
            .ok_or("short kind 9")?
            .try_into()
            .map_err(|_| "short")?,
    );
    let links = flow.get(8..).ok_or("short kind 9")?;
    if links.len() % 5 != 0 {
        return Err("kind 9 is not whole links".to_owned());
    }
    let mut targets = Vec::new();
    let mut costs = Vec::new();
    for link in links.chunks_exact(5) {
        let [target, c0, c1, c2, c3] = link else {
            return Err("a link is not 5 bytes".to_owned());
        };
        targets.push(i8::from_le_bytes([*target]));
        costs.push(u32::from_le_bytes([*c0, *c1, *c2, *c3]));
    }
    Ok(FlowMeta {
        side: le_u16(charges, 0)?,
        labels: charges
            .get(2..)
            .ok_or("short charges")?
            .iter()
            .map(|&b| i8::from_le_bytes([b]))
            .collect(),
        window: (le_u16(incidence, 0)?, le_u16(incidence, 2)?),
        positions: incidence
            .get(4..)
            .ok_or("short incidence")?
            .chunks_exact(4)
            .map(|c| <[u8; 4]>::try_from(c).map(u32::from_le_bytes))
            .collect::<Result<_, _>>()
            .map_err(|_| "short position".to_owned())?,
        weight,
        targets,
        costs,
    })
}

/// The `flow` stream of one salt, replayed with the FAM-23 draw and the FAM-24
/// shuffle: the try that passed, the labels, the targets, and the costs.
type FlowReplay = (u64, Vec<i8>, Vec<i8>, Vec<u32>);

/// `None` when all four tries are refused by the request check of `square`.
fn replay_flow(case: &Value, slot: u64) -> Result<Option<FlowReplay>, String> {
    use core::num::NonZeroU32;
    use ising_generators::family::Streams;
    use ising_generators::rng::{shuffle, uniform};
    use ising_generators::square::{check_request, Boundary, Request, SquareLattice};
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let (side, half) = flow_shape(&params)?;
    let side = u16::try_from(side).map_err(|e| e.to_string())?;
    let half = u32::try_from(half).map_err(|e| e.to_string())?;
    let (nodes, edges) = topology_inputs(object_field(case, "topology")?)?;
    let mut streams = Streams::new(
        codec::hex32(&lease_nonce(case, slot)?)?,
        digests::descriptor_digest(
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &digests::params_digest(&params),
        ),
        digests::topology_order_digest(&nodes, &edges),
        FLOW.labels,
    );
    let mut stream = streams.open("flow").ok_or("no flow stream")?;
    let lattice = SquareLattice::new(side, Boundary::Open).map_err(|e| e.to_string())?;
    let width = u32::from(side) + 1;
    let last = u32::from(side);
    let on_border = |v: u32| {
        v.is_multiple_of(width) || v / width == 0 || v % width == last || v / width == last
    };
    let corner = |v: u32| {
        (v.is_multiple_of(width) || v % width == last) && (v / width == 0 || v / width == last)
    };
    let edge: Vec<u32> = (0..lattice.vertex_count())
        .filter(|&v| on_border(v) && !corner(v))
        .collect();
    let vertices = usize::try_from(lattice.vertex_count()).map_err(|e| e.to_string())?;
    for attempt in 1..=4u64 {
        let mut order = edge.clone();
        shuffle(&mut stream, &mut order).map_err(|e| format!("{e:?}"))?;
        let mut labels = vec![0i8; vertices];
        for (rank, &v) in order.iter().enumerate() {
            let slot = labels
                .get_mut(usize::try_from(v).map_err(|e| e.to_string())?)
                .ok_or("a vertex outside the lattice")?;
            *slot = if rank < order.len() / 2 { 1 } else { -1 };
        }
        let request = Request {
            charges: labels.iter().map(|&q| Some(q)).collect(),
            fixed: Vec::new(),
            fields: Vec::new(),
        };
        if check_request(&lattice, &request).is_err() {
            continue;
        }
        let two = NonZeroU32::new(2).ok_or("two")?;
        let bound = NonZeroU32::new(half + 1).ok_or("cost bound")?;
        let mut targets = Vec::new();
        let mut costs = Vec::new();
        for _ in 0..lattice.link_count() {
            targets.push(if uniform(&mut stream, two) == 1 {
                1
            } else {
                -1
            });
            costs.push(2 * uniform(&mut stream, bound));
        }
        return Ok(Some((attempt, labels, targets, costs)));
    }
    Ok(None)
}

/// The Rust metadata check must accept every built instance.
fn check_flow_metadata(params: &Params<'_>, built: &Instance) -> Result<FlowMeta, String> {
    if !(FLOW.check_metadata)(params, built) {
        return Err("check_metadata rejected a built instance".to_owned());
    }
    flow_meta(built)
}

/// The penalty weight of the contract: `R / 2 + 1`, raised to even.
fn expected_weight(costs: &[u32]) -> u64 {
    let weight = costs.iter().map(|&c| u64::from(c / 2)).sum::<u64>() + 1;
    weight + weight % 2
}

/// Check the evidence id and the name of one entry of a section.
fn check_entry(entry: &Value, name: &str) -> Result<(), String> {
    compare(
        entry,
        "name",
        &case_name(entry).to_owned(),
        &name.to_owned(),
    )?;
    compare(
        entry,
        "evidence",
        &entry.get("evidence").cloned(),
        &Some(json!([EVIDENCE])),
    )
}

/// The index of the first instance of a case, the one that the witnesses and the
/// network describe.
fn first_index(case: &Value) -> Result<u64, String> {
    let first = entries_of(case, "instances")?
        .first()
        .ok_or("a case builds no instance")?;
    u64_decimal(first, "index")
}

/// Recompute the digests, nonces, instance bytes, and the contract fields of every
/// case with Rust code that does not call the construction.
fn check_flow(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    check_names_unique(doc)?;
    for case in section(doc, "cases")? {
        check_layered_case(case, flow_size_bounds)?;
        compare(
            case,
            "metadata_kinds",
            &entries_of(case, "metadata_kinds")?.to_vec(),
            &vec![json!(4), json!(5), json!(9)],
        )?;
        compare(
            case,
            "coefficient_bound_milli",
            &case.get("coefficient_bound_milli").and_then(Value::as_i64),
            &Some(COEFFICIENT_BOUND),
        )?;
        compare(
            case,
            "evidence",
            &case.get("evidence").cloned(),
            &Some(json!([EVIDENCE])),
        )?;
    }
    Ok(())
}

#[test]
fn flow_fixture_matches_schema() -> Result<(), String> {
    let doc = flow_doc()?;
    validate(&schema()?, &doc)?;
    match doc.get("provenance").and_then(Value::as_str) {
        Some("specified") => {}
        other => return Err(format!("provenance is {other:?}, expected specified")),
    }
    check_names_unique(&doc)?;
    for name in [
        "cases",
        "flow_witnesses",
        "networks",
        "flow_recipes",
        "admission",
    ] {
        section(&doc, name).map(|_| ())?;
    }
    Ok(())
}

#[test]
fn flow_cases_recompute() -> Result<(), String> {
    check_flow(&flow_doc()?)
}

#[test]
fn flow_cases_match_the_rust_construction() -> Result<(), String> {
    for case in section(&flow_doc()?, "cases")? {
        check_layered_against_rust(case)?;
    }
    Ok(())
}

#[test]
fn flow_params_constants_match_the_vectors() -> Result<(), String> {
    use quip_solver_conformance::families::flow::{P_FLOW_SIDE1, P_FLOW_Z24_L2, P_FLOW_Z34_L3};
    let doc = flow_doc()?;
    for (section_name, name, want) in [
        ("cases", "flow-l2", &P_FLOW_Z24_L2[..]),
        ("cases", "flow-l3", &P_FLOW_Z34_L3[..]),
        ("admission", "side-1-is-out-of-bounds", &P_FLOW_SIDE1[..]),
    ] {
        let case = find_named(&doc, section_name, name)?;
        compare(
            case,
            "params_hex",
            &str_field(case, "params_hex")?.to_owned(),
            &codec::hex_encode(want),
        )?;
    }
    compare(&doc, "case count", &section(&doc, "cases")?.len(), &4)
}

/// Rebuild every salt of every case with the Rust construction, replay the
/// `flow` stream, and compare the labels, the targets, the costs, and the weight.
fn check_flow_draws(doc: &Value) -> Result<(), String> {
    for case in section(doc, "cases")? {
        for entry in entries_of(case, "skipped")? {
            let replay = replay_flow(case, u64_decimal(entry, "index")?)?;
            compare(case, "a skipped salt fails every try", &replay, &None)?;
        }
        for instance in entries_of(case, "instances")? {
            let slot = u64_decimal(instance, "index")?;
            let meta = with_ice_instance(FLOW_V1, case, slot, |params, built| {
                check_flow_metadata(params, built)
            })?;
            let (_, labels, targets, costs) =
                replay_flow(case, slot)?.ok_or("a salt that builds has a passing try")?;
            compare(case, "labels", &meta.labels, &labels)?;
            compare(case, "targets", &meta.targets, &targets)?;
            compare(case, "costs", &meta.costs, &costs)?;
            compare(case, "weight", &meta.weight, &expected_weight(&costs))?;
        }
    }
    Ok(())
}

#[test]
fn flow_draws_match_a_replay_of_the_flow_stream() -> Result<(), String> {
    check_flow_draws(&flow_doc()?)
}

/// The labels of every case are balanced, the corners and the interior vertices
/// hold 0, and the weight is even and beats the objective range.
#[test]
fn flow_requests_are_balanced_and_the_weight_beats_the_objective() -> Result<(), String> {
    let doc = flow_doc()?;
    for case in section(&doc, "cases")? {
        for instance in entries_of(case, "instances")? {
            let slot = u64_decimal(instance, "index")?;
            let meta = with_ice_instance(FLOW_V1, case, slot, check_flow_metadata)?;
            let total: i32 = meta.labels.iter().map(|&q| i32::from(q)).sum();
            compare(case, "labels sum to 0", &total, &0)?;
            let side = usize::from(meta.side);
            let plus = meta.labels.iter().filter(|&&q| q == 1).count();
            compare(case, "sinks", &plus, &(2 * (side - 1)))?;
            compare(case, "weight is even", &(meta.weight % 2), &0)?;
            let range: u64 = meta.costs.iter().map(|&c| u64::from(c)).sum();
            compare(case, "2 U beats R", &(2 * meta.weight > range), &true)?;
        }
    }
    Ok(())
}

/// Admit `params` for `family_id` and `revision` on a topology and give the code
/// of the first refusal, or `None` when the request is admitted.
fn flow_admission_code(
    case: &Value,
    params: &[u8],
    nodes: Vec<u32>,
    edges: &[(u32, u32)],
) -> Result<Option<u8>, String> {
    let contracts = [FLOW_V1];
    let topology = BaseTopology::new(nodes, edges).map_err(|e| format!("{e:?}"))?;
    let got = admit(
        &contracts,
        str_field(case, "family_id")?.as_bytes(),
        u32_field(case, "revision")?,
        params,
        &params_digest(params),
    )
    .and_then(|admitted| {
        admit_topology(
            &admitted,
            &topology,
            topology.order_digest(),
            &AdvertisedLimits {
                max_nodes: 0,
                max_edges: 0,
                max_degree: 0,
            },
        )
    });
    Ok(got.err().map(ising_generators::Code::value))
}

/// Compare the digest, the family, the revision, the code, and the topology
/// order digest of one admission entry with the Rust admission.
fn check_flow_admission_case(case: &Value) -> Result<Option<u8>, String> {
    let params = codec::hex_decode(str_field(case, "params_hex")?)?;
    let topology = object_field(case, "topology")?;
    let (nodes, edges) = topology_inputs(topology)?;
    compare(
        case,
        "order_digest",
        &codec::hex_encode(&digests::topology_order_digest(&nodes, &edges)),
        &str_field(topology, "order_digest")?.to_owned(),
    )?;
    compare(
        case,
        "evidence",
        &case.get("evidence").cloned(),
        &Some(json!([EVIDENCE])),
    )?;
    let got = flow_admission_code(case, &params, nodes, &edges)?;
    compare(case, "admission code", &got, &expected_code(case)?)?;
    Ok(got)
}

fn check_flow_admission(doc: &Value) -> Result<(), String> {
    let mut codes = Vec::new();
    for case in section(doc, "admission")? {
        codes.push(check_flow_admission_case(case)?);
    }
    compare(doc, "codes 9 present", &codes.contains(&Some(9)), &true)?;
    compare(doc, "code 12 present", &codes.contains(&Some(12)), &true)?;
    compare(doc, "an admitted request", &codes.contains(&None), &true)
}

#[test]
fn flow_admission_codes_match_the_rust_admission() -> Result<(), String> {
    check_flow_admission(&flow_doc()?)
}

/// Recompute the feasible result, the observables, and the encoded energy of one
/// witness through the Rust construction. Returns the objective and the energy.
fn check_flow_witness(doc: &Value, witness: &Value) -> Result<(u64, i128), String> {
    let case = find_named(doc, "cases", str_field(witness, "case")?)?;
    let slot = u64_decimal(witness, "index")?;
    compare(witness, "index", &slot, &first_index(case)?)?;
    let spins: Vec<i8> = entries_of(witness, "spins")?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i8::try_from(n).ok())
                .ok_or_else(|| "a spin is not an i8".to_owned())
        })
        .collect::<Result<_, _>>()?;
    let want_feasible = witness
        .get("feasible")
        .and_then(Value::as_bool)
        .ok_or("no feasible flag")?;
    let kind = if want_feasible {
        "feasible"
    } else {
        "infeasible"
    };
    check_entry(witness, &format!("{}-{kind}", case_name(case)))?;
    let int = |key: &str| {
        witness
            .get(key)
            .and_then(Value::as_i64)
            .ok_or_else(|| format!("no {key}"))
    };
    with_ice_instance(FLOW_V1, case, slot, |params, built| {
        let contracts = [FLOW_V1];
        let bytes = codec::hex_decode(str_field(case, "params_hex")?)?;
        let admitted = admit(
            &contracts,
            str_field(case, "family_id")?.as_bytes(),
            u32_field(case, "revision")?,
            &bytes,
            &params_digest(&bytes),
        )
        .map_err(|c| format!("admit failed with {c}"))?;
        compare(
            witness,
            "feasible",
            &feasible(&admitted, built, &spins),
            &want_feasible,
        )?;
        let seen = flow_observables(params, built, &spins).ok_or("no observables")?;
        compare(
            witness,
            "objective",
            &i64::try_from(seen.objective).map_err(|e| e.to_string())?,
            &int("objective")?,
        )?;
        compare(
            witness,
            "charges",
            &seen
                .charges
                .iter()
                .map(|&q| json!(q))
                .collect::<Vec<Value>>(),
            &entries_of(witness, "charges")?.to_vec(),
        )?;
        compare(
            witness,
            "violations",
            &i64::from(seen.violations),
            &int("violations")?,
        )?;
        compare(
            witness,
            "reversed",
            &i64::from(seen.reversed),
            &int("reversed")?,
        )?;
        let energy = encoded_energy(built, &spins)?;
        compare(
            witness,
            "energy_milli",
            &energy,
            &i128::from(int("energy_milli")?),
        )?;
        // The flag is the violation count, so the oracle and the Rust observables
        // must agree on it.
        compare(
            witness,
            "feasible is no violation",
            &want_feasible,
            &(seen.violations == 0),
        )?;
        Ok((seen.objective, energy))
    })
}

/// Recompute every witness. Each case has one feasible and one infeasible
/// witness, and the infeasible one has a lower objective and a higher energy.
fn check_flow_witnesses(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    let witnesses = section(doc, "flow_witnesses")?;
    let cases = section(doc, "cases")?;
    compare(doc, "witness count", &witnesses.len(), &(2 * cases.len()))?;
    for case in cases {
        let at = |kind: &str| -> Result<(u64, i128), String> {
            let witness = find_named(
                doc,
                "flow_witnesses",
                &format!("{}-{kind}", case_name(case)),
            )?;
            compare(
                witness,
                "case",
                &str_field(witness, "case")?,
                &case_name(case),
            )?;
            check_flow_witness(doc, witness)
        };
        let (good_objective, good_energy) = at("feasible")?;
        let (bad_objective, bad_energy) = at("infeasible")?;
        compare(
            case,
            "a lower objective",
            &(bad_objective < good_objective),
            &true,
        )?;
        compare(case, "a higher energy", &(bad_energy > good_energy), &true)?;
    }
    Ok(())
}

#[test]
fn flow_witnesses_match_the_rust_feasibility_and_observables() -> Result<(), String> {
    check_flow_witnesses(&flow_doc()?)
}

/// The network of the first instance of a case, compared field by field with
/// `flow_network`.
fn check_flow_network(doc: &Value, entry: &Value) -> Result<(), String> {
    let case = find_named(doc, "cases", str_field(entry, "case")?)?;
    check_entry(entry, &format!("{}-network", case_name(case)))?;
    let slot = u64_decimal(entry, "index")?;
    compare(entry, "index", &slot, &first_index(case)?)?;
    with_ice_instance(FLOW_V1, case, slot, |params, built| {
        let net = flow_network(params, built).ok_or("no network")?;
        let supplies: Vec<Value> = net.supplies.iter().map(|&s| json!(s)).collect();
        compare(
            entry,
            "supplies",
            &supplies,
            &entries_of(entry, "supplies")?.to_vec(),
        )?;
        let tails: Vec<u64> = net.arcs.iter().map(|a| u64::from(a.tail)).collect();
        compare(entry, "tails", &tails, &u64_array(entry, "tails")?)?;
        let heads: Vec<u64> = net.arcs.iter().map(|a| u64::from(a.head)).collect();
        compare(entry, "heads", &heads, &u64_array(entry, "heads")?)?;
        let targets: Vec<Value> = net.arcs.iter().map(|a| json!(a.target)).collect();
        compare(
            entry,
            "targets",
            &targets,
            &entries_of(entry, "targets")?.to_vec(),
        )?;
        let costs: Vec<u64> = net.arcs.iter().map(|a| u64::from(a.cost)).collect();
        compare(entry, "costs", &costs, &u64_array(entry, "costs")?)?;
        let total: i32 = net.supplies.iter().map(|&s| i32::from(s)).sum();
        compare(entry, "supplies sum to 0", &total, &0)
    })
}

/// One network for each case, in the order of the cases.
fn check_flow_networks(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    let networks = section(doc, "networks")?;
    let cases = section(doc, "cases")?;
    compare(doc, "network count", &networks.len(), &cases.len())?;
    for (entry, case) in networks.iter().zip(cases) {
        compare(entry, "case", &str_field(entry, "case")?, &case_name(case))?;
        check_flow_network(doc, entry)?;
    }
    Ok(())
}

#[test]
fn flow_networks_match_the_rust_network() -> Result<(), String> {
    check_flow_networks(&flow_doc()?)
}

/// The hash and the decoded metadata of one built salt of a recipe.
type FlowSalt = (String, FlowMeta);

/// Every salt of recipe `index`, built with the Rust construction on the live
/// topology. A recipe that gives admission code 12 has none.
fn built_flow_salts(index: usize) -> Result<&'static [FlowSalt], String> {
    static BUILT: [std::sync::OnceLock<Result<Vec<FlowSalt>, String>>; 4] =
        [const { std::sync::OnceLock::new() }; 4];
    built_once(&BUILT, index, flow_doc, "flow_recipes", |recipe| {
        entries_of(recipe, "salts")?
            .iter()
            .map(|salt| {
                let slot = u64_decimal(salt, "index")?;
                with_ice_instance(FLOW_V1, recipe, slot, |params, built| {
                    let meta = check_flow_metadata(params, built)?;
                    let canonical = built.canonical_bytes().map_err(|e| e.to_string())?;
                    Ok((blake3::hash(&canonical).to_hex().to_string(), meta))
                })
            })
            .collect()
    })
}

/// Compare every field of recipe `index` with the Rust contract and construction.
fn check_flow_recipe(doc: &Value, recipe: &Value, index: usize) -> Result<(), String> {
    let want = FLOW_RECIPES.get(index).ok_or("no recipe in the contract")?;
    check_entry(recipe, &format!("flow-recipe-{}", want.alias))?;
    compare(recipe, "alias", &str_field(recipe, "alias")?, &want.alias)?;
    compare(recipe, "cell", &str_field(recipe, "cell")?, &want.cell)?;
    compare(
        recipe,
        "family_id",
        &str_field(recipe, "family_id")?,
        &"quip/flow-static",
    )?;
    compare(recipe, "revision", &u32_field(recipe, "revision")?, &1)?;
    let params = codec::hex_decode(str_field(recipe, "params_hex")?)?;
    compare(recipe, "params", &params.as_slice(), &want.params)?;
    compare(
        recipe,
        "params_digest",
        &codec::hex_encode(&params_digest(&params)),
        &str_field(recipe, "params_digest")?.to_owned(),
    )?;
    let topology = object_field(recipe, "topology")?;
    let (nodes, edges) = topology_inputs(topology)?;
    compare(
        recipe,
        "topology order_digest",
        &codec::hex_encode(&digests::topology_order_digest(&nodes, &edges)),
        &str_field(topology, "order_digest")?.to_owned(),
    )?;
    compare(
        recipe,
        "topology name",
        &str_field(topology, "name")?,
        &"zephyr-z12t4-live",
    )?;
    // The lease is the same for every recipe, and the nonces below fix it for the
    // recipes that build.
    let first = section(doc, "flow_recipes")?.first().ok_or("no recipe")?;
    compare(recipe, "lease", &recipe.get("lease"), &first.get("lease"))?;
    let lease = object_field(recipe, "lease")?;
    compare(recipe, "salt_count", &u64_decimal(lease, "salt_count")?, &2)?;
    let code = flow_admission_code(recipe, &params, nodes, &edges)?;
    compare(
        recipe,
        "admission_code",
        &code,
        &recipe
            .get("admission_code")
            .and_then(Value::as_u64)
            .map(|n| u8::try_from(n).map_err(|e| e.to_string()))
            .transpose()?,
    )?;
    let salts = entries_of(recipe, "salts")?;
    let built = built_flow_salts(index)?;
    compare(
        recipe,
        "salts",
        &salts.len(),
        &if code.is_none() { 2 } else { 0 },
    )?;
    compare(recipe, "built salts", &built.len(), &salts.len())?;
    for (position, (salt, (hash, meta))) in salts.iter().zip(built).enumerate() {
        let slot = u64_decimal(salt, "index")?;
        compare(
            recipe,
            "index",
            &slot,
            &u64::try_from(position).map_err(|e| e.to_string())?,
        )?;
        compare(
            recipe,
            "nonce",
            &lease_nonce(recipe, slot)?,
            &str_field(salt, "nonce")?.to_owned(),
        )?;
        compare(
            recipe,
            "canonical_blake3",
            hash,
            &str_field(salt, "canonical_blake3")?.to_owned(),
        )?;
        compare(
            recipe,
            "window",
            &u64_array(salt, "window")?,
            &vec![u64::from(meta.window.0), u64::from(meta.window.1)],
        )?;
        let (attempt, _, _, _) =
            replay_flow(recipe, slot)?.ok_or("a salt that builds has a passing try")?;
        compare(
            recipe,
            "try",
            &attempt,
            &salt.get("try").and_then(Value::as_u64).ok_or("no try")?,
        )?;
    }
    Ok(())
}

fn check_flow_recipes(doc: &Value) -> Result<(), String> {
    validate(&schema()?, doc)?;
    let recipes = section(doc, "flow_recipes")?;
    compare(doc, "recipe count", &recipes.len(), &FLOW_RECIPES.len())?;
    for (index, recipe) in recipes.iter().enumerate() {
        check_flow_recipe(doc, recipe, index)?;
    }
    Ok(())
}

#[test]
fn flow_recipes_match_the_rust_construction() -> Result<(), String> {
    check_flow_recipes(&flow_doc()?)
}

#[test]
fn l12_has_no_window_on_the_live_topology_and_the_others_build() -> Result<(), String> {
    let doc = flow_doc()?;
    let codes: Vec<(String, Option<u64>)> = section(&doc, "flow_recipes")?
        .iter()
        .map(|r| {
            Ok((
                str_field(r, "alias")?.to_owned(),
                r.get("admission_code").and_then(Value::as_u64),
            ))
        })
        .collect::<Result<_, String>>()?;
    compare(
        &doc,
        "recipe codes",
        &codes,
        &vec![
            ("l4".to_owned(), None),
            ("l6".to_owned(), None),
            ("l8".to_owned(), None),
            ("l12".to_owned(), Some(12)),
        ],
    )
}

#[test]
fn flow_windows_are_inside_the_cell_grid_and_the_dead_cell_moves_it() -> Result<(), String> {
    let doc = flow_doc()?;
    for case in section(&doc, "cases")? {
        let params = codec::hex_decode(str_field(case, "params_hex")?)?;
        let (side, _) = flow_shape(&params)?;
        let m = u64::from(le_u16(&params, 7)?);
        let meta = with_ice_instance(FLOW_V1, case, first_index(case)?, check_flow_metadata)?;
        let (c0, r0) = (u64::from(meta.window.0), u64::from(meta.window.1));
        compare(case, "side", &u64::from(meta.side), &side)?;
        compare(
            case,
            "the window fits the cell grid",
            &(c0 >= 1 && r0 >= 1 && c0 + side < 2 * m && r0 + side < 2 * m),
            &true,
        )?;
        let expected = match case_name(case) {
            "flow-dead-cell" => (1, 2),
            _ => (1, 1),
        };
        compare(case, "window", &(c0, r0), &expected)?;
        compare(
            case,
            "the link positions are every instance node",
            &{
                let mut sorted = meta.positions.clone();
                sorted.sort_unstable();
                sorted
            },
            &(0..u32::try_from(meta.positions.len()).map_err(|e| e.to_string())?).collect(),
        )?;
    }
    Ok(())
}

/// A copy of `value` with one leaf changed so that no check can read it as the
/// same: a flag flips, a spin or a target changes sign, another integer grows by
/// one, a hex string changes its last digit, a decimal string grows by one, and
/// any other string gains a letter.
fn changed_leaf(value: &Value) -> Result<Value, String> {
    Ok(match value {
        Value::Bool(flag) => json!(!flag),
        Value::Null => json!(12),
        Value::Number(n) => {
            let n = n.as_i64().ok_or("not an integer")?;
            json!(if n == 1 || n == -1 { -n } else { n + 1 })
        }
        Value::String(text) if text.parse::<u64>().is_ok() => {
            json!((text.parse::<u64>().map_err(|e| e.to_string())? + 1).to_string())
        }
        Value::String(text) if text.chars().all(|c| c.is_ascii_hexdigit()) => {
            json!(flip_last_hex_digit(text))
        }
        Value::String(text) => json!(format!("{text}x")),
        _ => return Err("not a leaf".to_owned()),
    })
}

/// The pointers of the leaves of `value` under `prefix`. A long array gives its
/// first, middle, and last element, because one changed element is as strong a
/// test as any other.
fn leaf_pointers(value: &Value, prefix: &str, out: &mut Vec<String>) {
    match value {
        Value::Object(map) => {
            for (key, inner) in map {
                leaf_pointers(inner, &format!("{prefix}/{key}"), out);
            }
        }
        Value::Array(items) => {
            let last = items.len().saturating_sub(1);
            for (at, inner) in items.iter().enumerate() {
                if items.len() <= 8 || at == 0 || at == last || at == items.len() / 2 {
                    leaf_pointers(inner, &format!("{prefix}/{at}"), out);
                }
            }
        }
        _ => out.push(prefix.to_owned()),
    }
}

/// Change every leaf of every entry of `section_name`, one at a time, and expect
/// `check` to refuse each changed document.
fn every_changed_field_fails(
    doc: &Value,
    section_name: &str,
    check: fn(&Value, &Value, usize) -> Result<(), String>,
) -> Result<(), String> {
    for (at, entry) in section(doc, section_name)?.iter().enumerate() {
        check(doc, entry, at)?;
        let mut pointers = Vec::new();
        leaf_pointers(entry, &format!("/{section_name}/{at}"), &mut pointers);
        for pointer in pointers {
            let was = doc.pointer(&pointer).ok_or("no value")?;
            let broken = mutate(doc, &pointer, changed_leaf(was)?);
            let changed = section(&broken, section_name)?.get(at).ok_or("no entry")?;
            expect_err(&check(&broken, changed, at), &pointer)?;
        }
    }
    Ok(())
}

#[test]
fn a_changed_flow_witness_field_fails_the_check() -> Result<(), String> {
    every_changed_field_fails(&flow_doc()?, "flow_witnesses", |doc, entry, _| {
        check_flow_witness(doc, entry).map(|_| ())
    })
}

#[test]
fn a_changed_flow_network_field_fails_the_check() -> Result<(), String> {
    every_changed_field_fails(&flow_doc()?, "networks", |doc, entry, _| {
        check_flow_network(doc, entry)
    })
}

#[test]
fn a_changed_flow_recipe_field_fails_the_check() -> Result<(), String> {
    every_changed_field_fails(&flow_doc()?, "flow_recipes", check_flow_recipe)
}

#[test]
fn a_changed_flow_admission_field_fails_the_check() -> Result<(), String> {
    let doc = flow_doc()?;
    for case in section(&doc, "admission")? {
        let _code = check_flow_admission_case(case)?;
        let fails = |pointer: &str, value: Value| {
            expect_err(
                &check_flow_admission_case(&mutate(case, pointer, value)).map(|_| ()),
                pointer,
            )
        };
        for pointer in [
            "/evidence/0",
            "/family_id",
            "/revision",
            "/topology/order_digest",
            "/topology/topology",
        ] {
            fails(
                pointer,
                changed_leaf(case.pointer(pointer).ok_or("no value")?)?,
            )?;
        }
        // A code that the request cannot give.
        fails(
            "/expected_code",
            match case.get("expected_code") {
                Some(Value::Null) => json!(9),
                _ => Value::Null,
            },
        )?;
        // Changed params give another code for some request. A request with a code that both keep has no changed field to
        // detect, so one of them must change the code.
        let params = str_field(case, "params_hex")?;
        let stem = params.get(..params.len() - 4).ok_or("short params")?;
        let (head, tail) = (
            params.get(..44).ok_or("short params")?,
            params.get(48..).ok_or("short params")?,
        );
        let mut detected = false;
        // `cost_half_max` 0, `cost_half_max` 65535, `side` 3, `zephyr_m` 2, and `zephyr_t` 4.
        for other in [
            format!("{stem}0000"),
            format!("{stem}ffff"),
            format!("{head}0300{tail}"),
            format!("{}0200{}", &params[..14], &params[18..]),
            format!("{}04{}", &params[..30], &params[32..]),
        ] {
            let changed = mutate(case, "/params_hex", json!(other));
            detected |= check_flow_admission_case(&changed).is_err();
        }
        compare(case, "a changed params field is detected", &detected, &true)?;
    }
    Ok(())
}

#[test]
fn a_changed_flow_case_field_fails_the_recompute() -> Result<(), String> {
    let doc = flow_doc()?;
    check_flow(&doc)?;
    for (at, case) in section(&doc, "cases")?.iter().enumerate() {
        let prefix = format!("/cases/{at}");
        let mut pointers = vec![
            format!("{prefix}/params_hex"),
            format!("{prefix}/params_digest"),
            format!("{prefix}/descriptor_digest"),
            format!("{prefix}/coefficient_bound_milli"),
            format!("{prefix}/metadata_kinds/2"),
            format!("{prefix}/topology/order_digest"),
            format!("{prefix}/size_bounds/nodes"),
            format!("{prefix}/size_bounds/edges"),
            format!("{prefix}/size_bounds/degree"),
            format!("{prefix}/lease/salt_start"),
            format!("{prefix}/evidence/0"),
        ];
        for list in ["instances", "skipped"] {
            for (index, _) in entries_of(case, list)?.iter().enumerate() {
                pointers.push(format!("{prefix}/{list}/{index}/nonce"));
            }
        }
        for key in ["h", "j", "node_ids"] {
            pointers.push(format!("{prefix}/instances/0/{key}/0"));
        }
        for key in [
            "offset_milli",
            "metadata_hex",
            "canonical_hex",
            "instance_digest",
            "energy_all_up_milli",
        ] {
            pointers.push(format!("{prefix}/instances/0/{key}"));
        }
        for pointer in pointers {
            let was = doc.pointer(&pointer).ok_or("no value")?;
            let broken = mutate(&doc, &pointer, changed_leaf(was)?);
            expect_err(&check_flow(&broken), &pointer)?;
        }
    }
    Ok(())
}
