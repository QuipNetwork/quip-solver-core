//! Family generation: substreams, base topologies, admission, and the one
//! canonical instance builder, checked against `vectors/families/core.json`.

use core::num::NonZeroU32;

use ising_generators::descriptor::{
    admit, params_digest, topology_order_digest, Admitted, AdvertisedLimits, Contract, Descriptor,
    FieldSpec, IdContext, Params, SizeBounds, TopologyStats, Value as ParamValue, ValueType,
};
use ising_generators::family::{
    admit_topology, generate_instance, BaseTopology, Construction, Defect, InstanceBody, SaltFault,
    Streams, TopologyFault,
};
use ising_generators::instance::{Instance, InstanceLimits};
use ising_generators::rng::uniform;
use ising_generators::Code;
use serde_json::Value;

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

const P_SPEC: &[u8] = &[
    0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0x24, 0xFA, 0xFF, 0xFF,
];
const P_L2_F0: &[u8] = &[0x01, 1, 0, 2, 0, 0, 0, 2, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];
const P_L3_F0: &[u8] = &[0x01, 1, 0, 2, 0, 0, 0, 3, 0, 2, 0, 4, 0, 0, 0, 0, 0, 0, 0];

const TIGHT_LIMITS: InstanceLimits = match InstanceLimits::new(
    16_777_216,
    67_108_864,
    2000,
    72_057_594_037_927_936,
    67_108_864,
) {
    Some(limits) => limits,
    None => InstanceLimits::FAM_37,
};

const SPARSE_NODES: [u32; 6] = [907, 41, 0, 4_294_967_295, 12, 3];
const SPARSE_EDGES: [(u32, u32); 7] = [
    (907, 41),
    (41, 0),
    (0, 4_294_967_295),
    (4_294_967_295, 12),
    (12, 3),
    (3, 907),
    (41, 12),
];

macro_rules! ensure {
    ($cond:expr) => {
        if !$cond {
            return Err(format!("check failed: {}", stringify!($cond)));
        }
    };
}

macro_rules! ensure_eq {
    ($left:expr, $right:expr, $note:expr) => {
        ensure_eq!(@go $left, $right, $note)
    };
    ($left:expr, $right:expr) => {
        ensure_eq!(@go $left, $right, "")
    };
    (@go $left:expr, $right:expr, $note:expr) => {
        match (&$left, &$right) {
            (left, right) => {
                if left != right {
                    return Err(format!("{}: left {left:?}, right {right:?}", $note));
                }
            }
        }
    };
}

// ---- helpers ----

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn section<'a>(doc: &'a Value, name: &str) -> Result<&'a [Value], String> {
    doc.get(name)
        .and_then(Value::as_array)
        .map(Vec::as_slice)
        .ok_or_else(|| format!("missing array section {name}"))
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err(format!("odd hex length {}", s.len()));
    }
    (0..s.len())
        .step_by(2)
        .map(|i| {
            s.get(i..i + 2)
                .ok_or_else(|| "short hex".to_string())
                .and_then(|p| u8::from_str_radix(p, 16).map_err(|e| e.to_string()))
        })
        .collect()
}

fn hex32(s: &str) -> Result<[u8; 32], String> {
    hex(s)?
        .try_into()
        .map_err(|v: Vec<u8>| format!("expected 32 bytes, got {}", v.len()))
}

fn str_field<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string {key}"))
}

fn u32_of(v: &Value) -> Result<u32, String> {
    v.as_u64()
        .and_then(|n| u32::try_from(n).ok())
        .ok_or_else(|| format!("not a u32: {v}"))
}

fn pairs_of(case: &Value, key: &str) -> Result<Vec<(u32, u32)>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing {key}"))?
        .iter()
        .map(|e| {
            let pair = e.as_array().ok_or("edge is not an array")?;
            match pair.as_slice() {
                [u, v] => Ok((u32_of(u)?, u32_of(v)?)),
                _ => Err("edge is not a pair".to_string()),
            }
        })
        .collect()
}

fn nodes_of(case: &Value) -> Result<Vec<u32>, String> {
    case.get("nodes")
        .and_then(Value::as_array)
        .ok_or("missing nodes")?
        .iter()
        .map(u32_of)
        .collect()
}

fn nz(n: u32) -> Result<NonZeroU32, String> {
    NonZeroU32::new(n).ok_or_else(|| "zero".to_string())
}

fn sparse() -> Result<BaseTopology, String> {
    BaseTopology::new(SPARSE_NODES.to_vec(), &SPARSE_EDGES).map_err(|e| format!("{e:?}"))
}

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

fn overflowing_size_bounds(
    _params: &Params<'_>,
    _stats: &TopologyStats,
) -> Result<SizeBounds, Code> {
    Err(Code::BoundsExceeded)
}

// ---- the layered construction ----

const fn layered_construction() -> Construction {
    Construction {
        labels: &["salt", "couplings"],
        check_topology: layered_check_topology,
        generate: layered_generate,
        check_metadata: layered_check_metadata,
    }
}

static LAYERED: Construction = layered_construction();

fn layered_check_topology(_params: &Params<'_>, topology: &BaseTopology) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    topology
        .edges()
        .iter()
        .all(|&(u, v)| seen.insert((u.min(v), u.max(v))))
}

fn layered_params(params: &Params<'_>) -> Result<(u64, i32), Code> {
    let Some(ParamValue::Unsigned(layers)) = params.get(1) else {
        return Err(Code::ParamsMalformed);
    };
    let Some(ParamValue::Signed(field)) = params.get(2) else {
        return Err(Code::ParamsMalformed);
    };
    let field = i32::try_from(field).map_err(|_| Code::ParamsMalformed)?;
    Ok((layers, field))
}

fn layered_generate(
    params: &Params<'_>,
    topology: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let overflow = Code::BoundsExceeded;
    let (layers, field) = layered_params(params)?;
    let layers = u32::try_from(layers).map_err(|_| overflow)?;
    let n = u32::try_from(topology.node_ids().len()).map_err(|_| overflow)?;
    let mut salt = streams.open("salt").ok_or(Code::NoInstance)?;
    let w = uniform(&mut salt, nz(8).map_err(|_| overflow)?);
    if w == 0 {
        return Err(Code::NoInstance);
    }
    if w == 2 {
        let _wide = field.checked_mul(1 << 21).ok_or(overflow)?;
    }
    let scale = if w == 1 { 2 } else { 1 };
    let h_value = field.checked_mul(scale).ok_or(overflow)?;
    let mut node_ids = Vec::new();
    for k in 0..layers {
        for pos in 0..n {
            node_ids.push(
                k.checked_mul(n)
                    .and_then(|b| b.checked_add(pos))
                    .ok_or(overflow)?,
            );
        }
    }
    let mut edges = Vec::new();
    for k in 0..layers {
        let base = k.checked_mul(n).ok_or(overflow)?;
        for &(a, b) in topology.edges() {
            edges.push((
                base.checked_add(a.min(b)).ok_or(overflow)?,
                base.checked_add(a.max(b)).ok_or(overflow)?,
            ));
        }
        if k + 1 < layers {
            let next = base.checked_add(n).ok_or(overflow)?;
            for pos in 0..n {
                edges.push((base + pos, next.checked_add(pos).ok_or(overflow)?));
            }
        }
    }
    edges.sort_unstable();
    let mut couplings = streams.open("couplings").ok_or(Code::NoInstance)?;
    let three = nz(3).map_err(|_| overflow)?;
    let j = edges
        .iter()
        .map(|_| {
            i32::try_from(uniform(&mut couplings, three))
                .ok()
                .and_then(|d| d.checked_sub(1))
                .and_then(|d| d.checked_mul(1000))
                .ok_or(overflow)
        })
        .collect::<Result<Vec<_>, _>>()?;
    let h = vec![h_value; node_ids.len()];
    let offset_milli = 250_i64.checked_mul(i64::from(layers)).ok_or(overflow)?;
    Ok(InstanceBody {
        node_ids,
        edges,
        h,
        j,
        offset_milli,
        metadata: layered_region(layers, n),
    })
}

fn layered_region(layers: u32, n: u32) -> Vec<u8> {
    let mut region = vec![1, 0, 1, 0, 6, 0, 0, 0];
    region.extend_from_slice(&u16::try_from(layers).unwrap_or(u16::MAX).to_le_bytes());
    region.extend_from_slice(&n.to_le_bytes());
    region
}

fn layered_check_metadata(params: &Params<'_>, instance: &Instance) -> bool {
    let Ok((layers, _)) = layered_params(params) else {
        return false;
    };
    let (Ok(layers32), Ok(len)) = (
        u32::try_from(layers),
        u32::try_from(instance.node_ids().len()),
    ) else {
        return false;
    };
    let Some(n) = len.checked_div(layers32) else {
        return false;
    };
    instance.metadata() == layered_region(layers32, n) && n.checked_mul(layers32) == Some(len)
}

// ---- small local constructions ----

fn small_body() -> InstanceBody {
    InstanceBody {
        node_ids: vec![0, 1],
        edges: vec![(0, 1)],
        h: vec![0, 0],
        j: vec![1000],
        offset_milli: 0,
        metadata: vec![0, 0],
    }
}

fn accept_topology(_: &Params<'_>, _: &BaseTopology) -> bool {
    true
}

fn accept_metadata(_: &Params<'_>, _: &Instance) -> bool {
    true
}

fn reject_metadata(_: &Params<'_>, _: &Instance) -> bool {
    false
}

const fn small(
    generate: fn(&Params<'_>, &BaseTopology, &mut Streams<'_>) -> Result<InstanceBody, Code>,
) -> Construction {
    Construction {
        labels: &["salt"],
        check_topology: accept_topology,
        generate,
        check_metadata: accept_metadata,
    }
}

fn gen_honest(
    _: &Params<'_>,
    _: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let _stream = streams.open("salt").ok_or(Code::NoInstance)?;
    Ok(small_body())
}

#[expect(clippy::unnecessary_wraps, reason = "must match GenerateFn")]
fn gen_unlisted(
    _: &Params<'_>,
    _: &BaseTopology,
    streams: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    let _stream = streams.open("other");
    Ok(small_body())
}

fn gen_overflow(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Err(Code::BoundsExceeded)
}

fn gen_exhausted(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Err(Code::NoInstance)
}

fn gen_other_code(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Err(Code::ParamsMalformed)
}

#[expect(clippy::unnecessary_wraps, reason = "must match GenerateFn")]
fn gen_big_h(_: &Params<'_>, _: &BaseTopology, _: &mut Streams<'_>) -> Result<InstanceBody, Code> {
    Ok(InstanceBody {
        h: vec![2001, 0],
        ..small_body()
    })
}

#[expect(clippy::unnecessary_wraps, reason = "must match GenerateFn")]
fn gen_unsorted(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Ok(InstanceBody {
        node_ids: vec![0, 1, 2],
        edges: vec![(1, 2), (0, 1)],
        h: vec![0; 3],
        j: vec![0; 2],
        ..small_body()
    })
}

#[expect(clippy::unnecessary_wraps, reason = "must match GenerateFn")]
fn gen_bad_region(
    _: &Params<'_>,
    _: &BaseTopology,
    _: &mut Streams<'_>,
) -> Result<InstanceBody, Code> {
    Ok(InstanceBody {
        metadata: vec![1, 0],
        ..small_body()
    })
}

static HONEST: Construction = small(gen_honest);
static UNLISTED: Construction = small(gen_unlisted);
static OVERFLOW: Construction = small(gen_overflow);
static EXHAUSTED: Construction = small(gen_exhausted);
static OTHER_CODE: Construction = small(gen_other_code);
static BIG_H: Construction = small(gen_big_h);
static UNSORTED: Construction = small(gen_unsorted);
static BAD_REGION: Construction = small(gen_bad_region);
static REJECTS_METADATA: Construction = Construction {
    check_metadata: reject_metadata,
    ..small(gen_honest)
};

fn contract(
    construction: Option<&'static Construction>,
    size_bounds: ising_generators::descriptor::SizeBoundsFn,
) -> Contract {
    Contract {
        family_id: "vectors/uniform-field",
        revision: 1,
        schema: UNIFORM_FIELD,
        metadata_kinds: &[1],
        limits: TIGHT_LIMITS,
        size_bounds,
        construction,
    }
}

fn admitted<'a>(contracts: &'a [Contract], params: &'a [u8]) -> Result<Admitted<'a>, String> {
    admit(
        contracts,
        b"vectors/uniform-field",
        1,
        params,
        &params_digest(params),
    )
    .map_err(|c| format!("admit failed with {c}"))
}

fn descriptor_digest(params: &[u8]) -> Result<[u8; 32], String> {
    Descriptor::new(b"vectors/uniform-field", 1, params, IdContext::Lease)
        .map(|d| d.digest())
        .map_err(|c| format!("descriptor failed with {c}"))
}

fn nonce_of(i: u32) -> [u8; 32] {
    let mut nonce = [7u8; 32];
    for (dst, src) in nonce.iter_mut().zip(i.to_le_bytes()) {
        *dst = src;
    }
    nonce
}

/// The first `salt` draw of `nonce` under `P_SPEC` on the sparse topology.
fn salt_draw(nonce: [u8; 32], topology: &BaseTopology) -> Result<u32, String> {
    let mut streams = Streams::new(
        nonce,
        descriptor_digest(P_SPEC)?,
        *topology.order_digest(),
        &["salt"],
    );
    let mut rng = streams.open("salt").ok_or("salt not listed")?;
    Ok(uniform(&mut rng, nz(8)?))
}

fn nonce_where(topology: &BaseTopology, want: impl Fn(u32) -> bool) -> Result<[u8; 32], String> {
    for i in 0..10_000 {
        let nonce = nonce_of(i);
        if want(salt_draw(nonce, topology)?) {
            return Ok(nonce);
        }
    }
    Err("no nonce found".into())
}

fn bounds_of(admitted: &Admitted<'_>, topology: &BaseTopology) -> Result<SizeBounds, String> {
    admit_topology(
        admitted,
        topology,
        topology.order_digest(),
        &AdvertisedLimits::default(),
    )
    .map_err(|c| format!("admit_topology failed with {c}"))
}

// ---- tests ----

#[test]
fn streams_match_the_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "streams")? {
        let label =
            String::from_utf8(hex(str_field(case, "label_hex")?)?).map_err(|e| e.to_string())?;
        let labels = [label.as_str()];
        let mut streams = Streams::new(
            hex32(str_field(case, "nonce")?)?,
            hex32(str_field(case, "descriptor_digest")?)?,
            hex32(str_field(case, "topology_order_digest")?)?,
            &labels,
        );
        for checkpoint in case
            .get("checkpoints")
            .and_then(Value::as_array)
            .ok_or("no checkpoints")?
        {
            let start = checkpoint
                .get("start")
                .and_then(Value::as_u64)
                .ok_or("no start")?;
            let mut rng = streams.open(&label).ok_or("label not listed")?;
            for _ in 0..start {
                let _skipped = rng.next_u32();
            }
            for want in checkpoint
                .get("words")
                .and_then(Value::as_array)
                .ok_or("no words")?
            {
                let want = want.as_u64().ok_or("word is not a number")?;
                if u64::from(rng.next_u32()) != want {
                    return Err(format!("stream {label} differs after word {start}"));
                }
            }
        }
    }
    Ok(())
}

#[test]
fn an_unlisted_label_is_a_defect() -> Result<(), String> {
    let contracts = [contract(Some(&UNLISTED), example_size_bounds)];
    let admitted = admitted(&contracts, P_L2_F0)?;
    let topology = sparse()?;
    let bounds = bounds_of(&admitted, &topology)?;
    ensure_eq!(
        generate_instance(&admitted, &topology, &bounds, nonce_of(0)).err(),
        Some(SaltFault::Defect(Defect::UnlistedLabel))
    );
    Ok(())
}

#[test]
fn base_topologies_match_the_topology_vectors() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "topologies")? {
        let nodes = nodes_of(case)?;
        let edges = pairs_of(case, "edges")?;
        let topology = BaseTopology::new(nodes.clone(), &edges).map_err(|e| format!("{e:?}"))?;
        let want = hex32(str_field(case, "order_digest")?)?;
        ensure_eq!(topology.order_digest(), &want, str_field(case, "name")?);
        ensure_eq!(topology_order_digest(&nodes, &edges), Some(want));
        ensure_eq!(topology.node_ids(), nodes.as_slice());
    }
    let topology = sparse()?;
    let stats = topology.stats();
    ensure_eq!((stats.n_nodes, stats.n_edges, stats.max_degree), (6, 7, 3));
    // Dense positions, in registration order and not normalized.
    ensure_eq!(
        topology.edges(),
        &[(0, 1), (1, 2), (2, 3), (3, 4), (4, 5), (5, 0), (1, 4)]
    );
    Ok(())
}

#[test]
fn a_base_topology_refuses_bad_input() -> Result<(), String> {
    ensure_eq!(
        BaseTopology::new(vec![1, 2, 1], &[]).err(),
        Some(TopologyFault::DuplicateNode(1))
    );
    ensure_eq!(
        BaseTopology::new(vec![1, 2], &[(1, 9)]).err(),
        Some(TopologyFault::UnknownNode(9))
    );
    ensure_eq!(
        BaseTopology::new(vec![1, 2], &[(8, 2)]).err(),
        Some(TopologyFault::UnknownNode(8))
    );
    Ok(())
}

#[test]
fn generation_fills_the_digests_and_the_nonce() -> Result<(), String> {
    let contracts = [contract(Some(&LAYERED), example_size_bounds)];
    let admitted = admitted(&contracts, P_SPEC)?;
    let topology = sparse()?;
    let bounds = bounds_of(&admitted, &topology)?;
    let nonce = nonce_where(&topology, |w| w >= 3)?;
    let instance =
        generate_instance(&admitted, &topology, &bounds, nonce).map_err(|f| format!("{f:?}"))?;
    ensure_eq!(instance.descriptor_digest(), &descriptor_digest(P_SPEC)?);
    ensure_eq!(instance.topology_order_digest(), topology.order_digest());
    ensure_eq!(instance.nonce(), Some(&nonce));
    ensure_eq!(instance.node_ids().len(), 12);
    ensure_eq!(instance.edges().len(), 20);
    ensure!(instance.h().iter().all(|&h| h == -1500));
    ensure_eq!(instance.offset_milli(), 500);
    let bytes = instance.canonical_bytes().map_err(|e| e.to_string())?;
    let mut hasher = blake3::Hasher::new_derive_key("quip family instance v1");
    let _updated = hasher.update(&bytes);
    ensure_eq!(instance.digest(), *hasher.finalize().as_bytes());
    Ok(())
}

#[test]
fn generation_is_deterministic() -> Result<(), String> {
    let contracts = [contract(Some(&LAYERED), example_size_bounds)];
    let admitted = admitted(&contracts, P_SPEC)?;
    let topology = sparse()?;
    let bounds = bounds_of(&admitted, &topology)?;
    let nonce = nonce_where(&topology, |w| w >= 3)?;
    let build = || {
        generate_instance(&admitted, &topology, &bounds, nonce)
            .map_err(|f| format!("{f:?}"))
            .and_then(|i| i.canonical_bytes().map_err(|e| e.to_string()))
    };
    ensure_eq!(build()?, build()?);
    Ok(())
}

#[test]
fn the_layered_construction_skips_the_salts_it_should() -> Result<(), String> {
    let contracts = [contract(Some(&LAYERED), example_size_bounds)];
    let admitted = admitted(&contracts, P_SPEC)?;
    let topology = sparse()?;
    let bounds = bounds_of(&admitted, &topology)?;
    for (w, want) in [
        (0, Code::NoInstance),
        (1, Code::BoundsExceeded),
        (2, Code::BoundsExceeded),
    ] {
        let nonce = nonce_where(&topology, |d| d == w)?;
        ensure_eq!(
            generate_instance(&admitted, &topology, &bounds, nonce).err(),
            Some(SaltFault::Skip(want)),
            format!("draw {w}")
        );
    }
    Ok(())
}

#[test]
fn codes_13_and_14_from_a_construction_skip_the_salt() -> Result<(), String> {
    let topology = sparse()?;
    for (construction, code) in [
        (&OVERFLOW, Code::BoundsExceeded),
        (&EXHAUSTED, Code::NoInstance),
    ] {
        let contracts = [contract(Some(construction), example_size_bounds)];
        let admitted = admitted(&contracts, P_L2_F0)?;
        let bounds = bounds_of(&admitted, &topology)?;
        ensure_eq!(
            generate_instance(&admitted, &topology, &bounds, nonce_of(0)).err(),
            Some(SaltFault::Skip(code))
        );
    }
    Ok(())
}

#[test]
fn an_instance_over_the_bounds_item_skips_the_salt() -> Result<(), String> {
    let contracts = [contract(Some(&BIG_H), example_size_bounds)];
    let admitted = admitted(&contracts, P_L2_F0)?;
    let topology = sparse()?;
    let bounds = bounds_of(&admitted, &topology)?;
    ensure_eq!(
        generate_instance(&admitted, &topology, &bounds, nonce_of(0)).err(),
        Some(SaltFault::Skip(Code::BoundsExceeded))
    );
    Ok(())
}

#[test]
fn construction_defects_are_reported() -> Result<(), String> {
    let topology = sparse()?;
    let run = |construction: Option<&'static Construction>,
               override_bounds: Option<SizeBounds>|
     -> Result<Option<SaltFault>, String> {
        let contracts = [contract(construction, example_size_bounds)];
        let admitted = admitted(&contracts, P_L2_F0)?;
        let bounds = match override_bounds {
            Some(bounds) => bounds,
            None => bounds_of(&admitted, &topology)?,
        };
        Ok(generate_instance(&admitted, &topology, &bounds, nonce_of(0)).err())
    };
    let roomy = SizeBounds {
        nodes: 100,
        edges: 100,
        degree: 100,
    };
    let defect = |d| Some(SaltFault::Defect(d));
    ensure_eq!(
        run(Some(&OTHER_CODE), None)?,
        defect(Defect::ConstructionCode(Code::ParamsMalformed))
    );
    ensure_eq!(
        run(Some(&UNSORTED), Some(roomy))?,
        defect(Defect::InstanceInvalid(Code::InstanceDigestMismatch))
    );
    ensure_eq!(
        run(Some(&BAD_REGION), Some(roomy))?,
        defect(Defect::InstanceInvalid(Code::MetadataInvalid))
    );
    ensure_eq!(
        run(Some(&HONEST), Some(SizeBounds { nodes: 1, ..roomy }))?,
        defect(Defect::SizeBound)
    );
    ensure_eq!(
        run(Some(&HONEST), Some(SizeBounds { edges: 0, ..roomy }))?,
        defect(Defect::SizeBound)
    );
    ensure_eq!(
        run(Some(&HONEST), Some(SizeBounds { degree: 0, ..roomy }))?,
        defect(Defect::SizeBound)
    );
    ensure_eq!(
        run(Some(&REJECTS_METADATA), Some(roomy))?,
        defect(Defect::MetadataRejected)
    );
    ensure_eq!(run(Some(&HONEST), Some(roomy))?, None);
    // A contract without a construction cannot pass admission, but a caller
    // that skips it still gets a defect, not a panic.
    let with_construction = [contract(Some(&HONEST), example_size_bounds)];
    let admitted_ok = admitted(&with_construction, P_L2_F0)?;
    let bare = [contract(None, example_size_bounds)];
    let admitted_bare = admitted(&bare, P_L2_F0)?;
    ensure_eq!(
        generate_instance(&admitted_bare, &topology, &roomy, nonce_of(0)).err(),
        defect(Defect::NoConstruction)
    );
    ensure!(generate_instance(&admitted_ok, &topology, &roomy, nonce_of(0)).is_ok());
    Ok(())
}

#[test]
fn admission_runs_11_then_12_then_13() -> Result<(), String> {
    let contracts = [contract(Some(&LAYERED), example_size_bounds)];
    let admitted_l2 = admitted(&contracts, P_L2_F0)?;
    let topology = sparse()?;
    let tiny = AdvertisedLimits {
        max_nodes: 1,
        ..AdvertisedLimits::default()
    };

    // 11 beats 13.
    ensure_eq!(
        admit_topology(&admitted_l2, &topology, &[0; 32], &tiny),
        Err(Code::TopologyOrderMismatch)
    );
    ensure_eq!(
        admit_topology(&admitted_l2, &topology, &[0; 31], &tiny),
        Err(Code::TopologyOrderMismatch)
    );

    // 12 beats 13: a repeated edge, then a self-loop.
    let mut doubled_edges = SPARSE_EDGES.to_vec();
    doubled_edges.push((41, 907));
    let doubled =
        BaseTopology::new(SPARSE_NODES.to_vec(), &doubled_edges).map_err(|e| format!("{e:?}"))?;
    ensure_eq!(
        admit_topology(&admitted_l2, &doubled, doubled.order_digest(), &tiny),
        Err(Code::TopologyIncompatible)
    );
    let mut looped_edges = SPARSE_EDGES.to_vec();
    looped_edges.push((907, 907));
    let looped =
        BaseTopology::new(SPARSE_NODES.to_vec(), &looped_edges).map_err(|e| format!("{e:?}"))?;
    let accepting = [contract(Some(&HONEST), example_size_bounds)];
    let admitted_accepting = admitted(&accepting, P_L2_F0)?;
    ensure_eq!(
        admit_topology(&admitted_accepting, &looped, looped.order_digest(), &tiny),
        Err(Code::TopologyIncompatible)
    );

    // A contract without a construction is code 12.
    let bare = [contract(None, example_size_bounds)];
    let admitted_bare = admitted(&bare, P_L2_F0)?;
    ensure_eq!(
        admit_topology(&admitted_bare, &topology, topology.order_digest(), &tiny),
        Err(Code::TopologyIncompatible)
    );

    // 13 from the advertised limits and from an overflowing size function.
    let degree_three = AdvertisedLimits {
        max_degree: 3,
        ..AdvertisedLimits::default()
    };
    ensure_eq!(
        admit_topology(
            &admitted_l2,
            &topology,
            topology.order_digest(),
            &degree_three
        ),
        Err(Code::BoundsExceeded)
    );
    ensure_eq!(
        admit_topology(&admitted_l2, &topology, topology.order_digest(), &tiny),
        Err(Code::BoundsExceeded)
    );
    let overflowing = [contract(Some(&LAYERED), overflowing_size_bounds)];
    let admitted_overflow = admitted(&overflowing, P_L2_F0)?;
    ensure_eq!(
        admit_topology(
            &admitted_overflow,
            &topology,
            topology.order_digest(),
            &AdvertisedLimits::default()
        ),
        Err(Code::BoundsExceeded)
    );
    let admitted_l3 = admitted(&contracts, P_L3_F0)?;
    let degree_four = AdvertisedLimits {
        max_degree: 4,
        ..AdvertisedLimits::default()
    };
    ensure_eq!(
        admit_topology(
            &admitted_l3,
            &topology,
            topology.order_digest(),
            &degree_four
        ),
        Err(Code::BoundsExceeded)
    );

    // The admitted bounds.
    ensure_eq!(
        admit_topology(
            &admitted_l2,
            &topology,
            topology.order_digest(),
            &degree_four
        ),
        Ok(SizeBounds {
            nodes: 12,
            edges: 20,
            degree: 4
        })
    );
    Ok(())
}
