//! Family fields on the wire: lengths survive encode and decode (FAM-46, transport
//! only), and a decoded binding still commits to the same instance (FAM-68).

use ising_generators::descriptor::{params_digest, Descriptor, IdContext};
use ising_generators::instance::{Instance, InstanceLimits, InstanceParts};
use prost::Message;
use quip_proto::v1::{
    ising_problem::Graph, CoefficientEncoding, EdgeList, FamilyBinding, FamilyDescriptor,
    IsingProblem, IsingProblemGenerator, Job, JobKind,
};
use quip_protocol::wire::encode_i32_le;
use serde_json::Value;

fn core() -> Result<Value, String> {
    serde_json::from_str(quip_solver_conformance::FAMILY_CORE).map_err(|e| e.to_string())
}

fn section(doc: &Value, name: &str) -> Result<Vec<Value>, String> {
    doc.get(name)
        .and_then(Value::as_array)
        .cloned()
        .ok_or_else(|| format!("section {name} is missing"))
}

fn str_of<'a>(case: &'a Value, key: &str) -> Result<&'a str, String> {
    case.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string {key}"))
}

fn hex_of(case: &Value, key: &str) -> Result<Vec<u8>, String> {
    let text = str_of(case, key)?;
    if !text.len().is_multiple_of(2) {
        return Err(format!("{key}: odd-length hex"));
    }
    (0..text.len())
        .step_by(2)
        .map(|i| {
            text.get(i..i + 2)
                .and_then(|pair| u8::from_str_radix(pair, 16).ok())
                .ok_or_else(|| format!("{key}: bad hex"))
        })
        .collect()
}

fn array32(bytes: &[u8]) -> Result<[u8; 32], String> {
    bytes
        .try_into()
        .map_err(|_| format!("expected 32 bytes, got {}", bytes.len()))
}

fn u32s(case: &Value, key: &str) -> Result<Vec<u32>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing array {key}"))?
        .iter()
        .map(|v| {
            v.as_u64()
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| format!("{key}: bad u32"))
        })
        .collect()
}

fn i32s(case: &Value, key: &str) -> Result<Vec<i32>, String> {
    case.get(key)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("missing array {key}"))?
        .iter()
        .map(|v| {
            v.as_i64()
                .and_then(|n| i32::try_from(n).ok())
                .ok_or_else(|| format!("{key}: bad i32"))
        })
        .collect()
}

fn edge_pairs(case: &Value) -> Result<Vec<(u32, u32)>, String> {
    case.get("edges")
        .and_then(Value::as_array)
        .ok_or_else(|| "missing edges".to_owned())?
        .iter()
        .map(|e| {
            let pair = e.as_array().ok_or_else(|| "bad edge".to_owned())?;
            let at = |i: usize| {
                pair.get(i)
                    .and_then(Value::as_u64)
                    .and_then(|n| u32::try_from(n).ok())
                    .ok_or_else(|| "bad edge position".to_owned())
            };
            Ok((at(0)?, at(1)?))
        })
        .collect()
}

fn round_trip<M: Message + Default>(message: &M) -> Result<M, String> {
    M::decode(message.encode_to_vec().as_slice()).map_err(|e| e.to_string())
}

fn bytes_of(length: usize) -> Vec<u8> {
    (0..length)
        .map(|i| u8::try_from(i % 251).unwrap_or(0))
        .collect()
}

fn check_same(name: &str, sent: &[u8], got: &[u8]) -> Result<(), String> {
    if sent == got {
        Ok(())
    } else {
        Err(format!("{name}: bytes changed on the wire"))
    }
}

#[test]
fn field_lengths_cross_the_wire_unchanged() -> Result<(), String> {
    let doc = core()?;
    for case in section(&doc, "field_lengths")? {
        let name = str_of(&case, "name")?;
        let field = str_of(&case, "field")?;
        let length = case
            .get("length")
            .and_then(Value::as_u64)
            .and_then(|n| usize::try_from(n).ok())
            .ok_or_else(|| format!("{name}: bad length"))?;
        let data = bytes_of(length);
        let (sent, got): (Vec<u8>, Vec<u8>) = match field {
            "FamilyDescriptor.params_digest" => {
                let m = FamilyDescriptor {
                    params_digest: data.clone(),
                    ..Default::default()
                };
                (data, round_trip(&m)?.params_digest)
            }
            "IsingProblemGenerator.topology_order_digest" => {
                let m = IsingProblemGenerator {
                    topology_order_digest: data.clone(),
                    ..Default::default()
                };
                (data, round_trip(&m)?.topology_order_digest)
            }
            "FamilyBinding.nonce" => {
                let m = FamilyBinding {
                    nonce: data.clone(),
                    ..Default::default()
                };
                (data, round_trip(&m)?.nonce)
            }
            "FamilyBinding.instance_digest" => {
                let m = FamilyBinding {
                    instance_digest: data.clone(),
                    ..Default::default()
                };
                (data, round_trip(&m)?.instance_digest)
            }
            "FamilyBinding.topology_order_digest" => {
                let m = FamilyBinding {
                    topology_order_digest: data.clone(),
                    ..Default::default()
                };
                (data, round_trip(&m)?.topology_order_digest)
            }
            "FamilyBinding.node_ids" => {
                let ids: Vec<u32> = (0..length)
                    .map(|i| u32::try_from(i).map(|n| n * 7 + 1))
                    .collect::<Result<_, _>>()
                    .map_err(|e| e.to_string())?;
                let m = FamilyBinding {
                    node_ids: ids.clone(),
                    ..Default::default()
                };
                if round_trip(&m)?.node_ids != ids {
                    return Err(format!("{name}: node ids changed on the wire"));
                }
                continue;
            }
            other => return Err(format!("{name}: unknown field {other}")),
        };
        check_same(name, &sent, &got)?;
    }
    Ok(())
}

/// Rebuilds the instance from decoded wire fields only (FAM-68 table).
fn parts_from_job(name: &str, decoded: &Job) -> Result<InstanceParts, String> {
    let problem = decoded
        .ising
        .clone()
        .ok_or_else(|| format!("{name}: no ising"))?;
    let family = problem.family.ok_or_else(|| format!("{name}: no family"))?;
    let desc = family
        .family
        .ok_or_else(|| format!("{name}: no descriptor"))?;
    let Some(Graph::Edges(list)) = problem.graph else {
        return Err(format!("{name}: no edges"));
    };
    let descriptor = Descriptor::new(
        desc.family_id.as_bytes(),
        desc.revision,
        &desc.params,
        IdContext::Direct,
    )
    .map_err(|c| format!("{name}: descriptor {c}"))?;
    let le32 = |bytes: &[u8]| -> Vec<i32> {
        bytes
            .chunks_exact(4)
            .filter_map(|c| <[u8; 4]>::try_from(c).ok().map(i32::from_le_bytes))
            .collect()
    };
    Ok(InstanceParts {
        descriptor_digest: descriptor.digest(),
        topology_order_digest: array32(&family.topology_order_digest)?,
        nonce: if family.nonce.is_empty() {
            None
        } else {
            Some(array32(&family.nonce)?)
        },
        node_ids: family.node_ids,
        edges: list.u.into_iter().zip(list.v).collect(),
        h: le32(&problem.h),
        j: le32(&problem.j),
        offset_milli: family.offset_milli,
        metadata: family.metadata,
    })
}

#[test]
fn binding_commits_to_the_instance_after_the_wire() -> Result<(), String> {
    let doc = core()?;
    let descriptors = section(&doc, "descriptors")?;
    let instances = section(&doc, "instances")?;
    for case in &instances {
        let name = str_of(case, "name")?;
        let family_id = str_of(case, "family_id")?;
        let params = hex_of(case, "params_hex")?;
        let revision = case
            .get("revision")
            .and_then(Value::as_u64)
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| format!("{name}: bad revision"))?;
        // The descriptor row with the same family id and params carries the digest.
        let row = descriptors
            .iter()
            .find(|d| {
                d.get("family_id").and_then(Value::as_str) == Some(family_id)
                    && d.get("params_hex") == case.get("params_hex")
            })
            .ok_or_else(|| format!("{name}: no descriptor row"))?;
        let digest = hex_of(row, "params_digest")?;
        if digest != params_digest(&params) {
            return Err(format!("{name}: fixture params digest differs"));
        }
        let canonical = hex_of(case, "canonical_hex")?;
        let nonce = hex_of(case, "nonce")?;
        let h = i32s(case, "h")?;
        let j = i32s(case, "j")?;
        let edges = edge_pairs(case)?;
        let offset_milli: i64 = str_of(case, "offset_milli")?
            .parse()
            .map_err(|_| format!("{name}: bad offset"))?;
        let binding = FamilyBinding {
            family: Some(FamilyDescriptor {
                family_id: family_id.to_owned(),
                revision,
                params,
                params_digest: digest,
            }),
            nonce,
            instance_digest: hex_of(case, "instance_digest")?,
            metadata: hex_of(case, "metadata_hex")?,
            // FAM-30 canonical layout: format byte, descriptor digest, topology digest.
            topology_order_digest: canonical
                .get(33..65)
                .ok_or_else(|| format!("{name}: canonical bytes too short"))?
                .to_vec(),
            node_ids: u32s(case, "node_ids")?,
            offset_milli,
        };
        let job = Job {
            job_id: vec![1],
            kind: JobKind::IsingSample as i32,
            ising: Some(IsingProblem {
                graph: Some(Graph::Edges(EdgeList {
                    u: edges.iter().map(|e| e.0).collect(),
                    v: edges.iter().map(|e| e.1).collect(),
                })),
                encoding: CoefficientEncoding::I32 as i32,
                scale: 1000,
                h: encode_i32_le(&h),
                j: encode_i32_le(&j),
                family: Some(binding),
                ..Default::default()
            }),
            ..Default::default()
        };
        let parts = parts_from_job(name, &round_trip(&job)?)?;
        let kinds: Vec<u16> = u32s(case, "metadata_kinds")?
            .into_iter()
            .map(|k| u16::try_from(k).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        let instance = Instance::new(parts, &InstanceLimits::FAM_37, &kinds, None)
            .map_err(|c| format!("{name}: instance {c}"))?;
        check_same(name, &hex_of(case, "instance_digest")?, &instance.digest())?;
        let bytes = instance
            .canonical_bytes()
            .map_err(|e| format!("{name}: {e}"))?;
        check_same(name, &canonical, &bytes)?;
    }
    Ok(())
}
