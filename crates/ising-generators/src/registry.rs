//! The family revisions this build implements.

use crate::defects::DEFECTS_V1;
use crate::descriptor::Contract;
use crate::flow::FLOW_V1;
use crate::ice::{CHARGED_ICE_V1, ICE_FIELDS_V1, ICE_GLASS_V1, TWISTED_DIMER_V1};
use crate::polariton::POLARITON_V1;
use crate::routing::ROUTING_V1;

/// Every contract with a construction in this crate, ordered by
/// `(family_id, revision)`. Pass it to [`crate::descriptor::admit`].
pub const CONTRACTS: &[Contract] = &[
    CHARGED_ICE_V1,
    DEFECTS_V1,
    FLOW_V1,
    ICE_FIELDS_V1,
    ICE_GLASS_V1,
    POLARITON_V1,
    TWISTED_DIMER_V1,
    ROUTING_V1,
];

use crate::{
    codec::{CodecError, WitnessCodec},
    descriptor::{Admitted, Params},
    instance::Instance,
};
use alloc::vec::Vec;

type Prepare = fn(&Params<'_>, &Instance) -> Result<WitnessCodec, CodecError>;
type Encode = fn(&Params<'_>, &Instance) -> Result<Vec<u8>, CodecError>;
type Decode = fn(&Params<'_>, &[u8; 32], &[u8]) -> Result<Instance, CodecError>;
struct Adapter {
    contract: Contract,
    prepare: Prepare,
    encode: Encode,
    decode: Decode,
}
fn adapter(admitted: &Admitted<'_>) -> Result<Adapter, CodecError> {
    match (
        admitted.descriptor().family_id(),
        admitted.descriptor().revision(),
    ) {
        ("quip/polariton-static", 1) => Ok(Adapter {
            contract: POLARITON_V1,
            prepare: crate::polariton::codec::prepare,
            encode: crate::polariton::codec::encode_instance,
            decode: crate::polariton::codec::decode_instance,
        }),
        ("quip/defects-static", 1) => Ok(Adapter {
            contract: DEFECTS_V1,
            prepare: crate::defects::codec::prepare,
            encode: crate::defects::codec::encode_instance,
            decode: crate::defects::codec::decode_instance,
        }),
        ("quip/flow-static", 1) => Ok(Adapter {
            contract: FLOW_V1,
            prepare: crate::flow::codec::prepare,
            encode: crate::flow::codec::encode_instance,
            decode: crate::flow::codec::decode_instance,
        }),
        ("quip/twisted-ice-dimer", 1) => Ok(Adapter {
            contract: TWISTED_DIMER_V1,
            prepare: crate::ice::twisted_dimer::codec::prepare,
            encode: crate::ice::twisted_dimer::codec::encode_instance,
            decode: crate::ice::twisted_dimer::codec::decode_instance,
        }),
        ("quip/charged-ice", 1) => Ok(Adapter {
            contract: CHARGED_ICE_V1,
            prepare: crate::ice::charge::codec::prepare,
            encode: crate::ice::charge::codec::encode_instance,
            decode: crate::ice::charge::codec::decode_instance,
        }),
        ("quip/ice-glass", 1) => Ok(Adapter {
            contract: ICE_GLASS_V1,
            prepare: crate::ice::glass::codec::prepare,
            encode: crate::ice::glass::codec::encode_instance,
            decode: crate::ice::glass::codec::decode_instance,
        }),
        ("quip/ice-fields", 1) => Ok(Adapter {
            contract: ICE_FIELDS_V1,
            prepare: crate::ice::fields::codec::prepare,
            encode: crate::ice::fields::codec::encode_instance,
            decode: crate::ice::fields::codec::decode_instance,
        }),
        ("quip/two-commodity-routing", 1) => Ok(Adapter {
            contract: ROUTING_V1,
            prepare: crate::routing::codec::prepare,
            encode: crate::routing::codec::encode_instance,
            decode: crate::routing::codec::decode_instance,
        }),
        _ => Err(CodecError::UnsupportedFamily),
    }
}
fn check_instance(
    admitted: &Admitted<'_>,
    instance: &Instance,
    contract: &Contract,
) -> Result<(), CodecError> {
    if instance.descriptor_digest() != &admitted.descriptor().digest() {
        return Err(CodecError::DescriptorMismatch);
    }
    let limits = &contract.limits;
    if instance.node_ids().len() > limits.max_nodes() as usize
        || instance.edges().len() > limits.max_edges() as usize
        || instance.metadata().len() > limits.max_metadata() as usize
        || instance.offset_milli().unsigned_abs() > limits.max_offset()
        || instance
            .h()
            .iter()
            .chain(instance.j())
            .any(|v| v.unsigned_abs() > limits.max_coefficient())
    {
        return Err(CodecError::BoundsExceeded);
    }
    Ok(())
}
fn canonical_params<'a>(
    admitted: &'a Admitted<'_>,
    contract: &Contract,
) -> Result<Params<'a>, CodecError> {
    let params = Params::parse(contract.schema, admitted.descriptor().params())
        .map_err(|_| CodecError::InvalidInstance)?;
    params
        .check_bounds()
        .map_err(|_| CodecError::BoundsExceeded)?;
    Ok(params)
}
/// Validate canonical family metadata once and prepare owned witness state.
///
/// Preparation can rebuild coefficients. Witness processing does not run this validation.
///
/// # Errors
/// Returns unsupported-family, descriptor, bounds, metadata, or allocation errors.
pub fn prepare_witness_codec(
    admitted: &Admitted<'_>,
    instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    let adapter = adapter(admitted)?;
    check_instance(admitted, instance, &adapter.contract)?;
    let params = canonical_params(admitted, &adapter.contract)?;
    let construction = adapter
        .contract
        .construction
        .ok_or(CodecError::UnsupportedFamily)?;
    if !(construction.check_metadata)(&params, instance) {
        return Err(CodecError::InvalidInstance);
    }
    (adapter.prepare)(&params, instance)
}
/// Encode an instance with the exact registered family revision.
///
/// # Errors
/// Rejects unsupported families, descriptor mismatches, exceeded limits, and schema violations.
pub fn encode_instance(
    admitted: &Admitted<'_>,
    instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    let adapter = adapter(admitted)?;
    check_instance(admitted, instance, &adapter.contract)?;
    (adapter.encode)(&canonical_params(admitted, &adapter.contract)?, instance)
}
/// Decode a compact instance with the admitted descriptor as external context.
///
/// # Errors
/// Rejects malformed, noncanonical, over-limit, or structurally invalid payloads.
pub fn decode_instance(admitted: &Admitted<'_>, bytes: &[u8]) -> Result<Instance, CodecError> {
    let adapter = adapter(admitted)?;
    let instance = (adapter.decode)(
        &canonical_params(admitted, &adapter.contract)?,
        &admitted.descriptor().digest(),
        bytes,
    )?;
    check_instance(admitted, &instance, &adapter.contract)?;
    Ok(instance)
}
