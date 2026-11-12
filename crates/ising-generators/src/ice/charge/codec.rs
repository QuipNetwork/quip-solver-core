//! Compact codec adapter, implemented by the family task.
use crate::{
    codec::{CodecError, WitnessCodec},
    descriptor::Params,
    instance::Instance,
};
use alloc::vec::Vec;
pub(crate) fn prepare(
    _params: &Params<'_>,
    _instance: &Instance,
) -> Result<WitnessCodec, CodecError> {
    Err(CodecError::UnsupportedFamily)
}
pub(crate) fn encode_instance(
    _params: &Params<'_>,
    _instance: &Instance,
) -> Result<Vec<u8>, CodecError> {
    Err(CodecError::UnsupportedFamily)
}
pub(crate) fn decode_instance(
    _params: &Params<'_>,
    _descriptor_digest: &[u8; 32],
    _bytes: &[u8],
) -> Result<Instance, CodecError> {
    Err(CodecError::UnsupportedFamily)
}
