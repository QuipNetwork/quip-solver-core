//! Shared compact codec API checks.
use ising_generators::codec::CodecError;

#[test]
fn stable_error_names() {
    assert_eq!(CodecError::DescriptorMismatch.name(), "descriptor_mismatch");
    assert_eq!(CodecError::NonCanonical.to_string(), "non_canonical");
}

use ising_generators::{
    descriptor::{admit, params_digest},
    instance::{Instance, InstanceLimits, InstanceParts},
    registry,
};

#[test]
#[expect(
    clippy::panic_in_result_fn,
    reason = "test assertions must fail independently of setup errors"
)]
fn registry_identity_errors() -> Result<(), Box<dyn std::error::Error>> {
    let instance = Instance::new(
        InstanceParts {
            descriptor_digest: [0; 32],
            topology_order_digest: [0; 32],
            nonce: None,
            node_ids: vec![],
            edges: vec![],
            h: vec![],
            j: vec![],
            offset_milli: 0,
            metadata: vec![0, 0],
        },
        &InstanceLimits::FAM_37,
        &[],
        None,
    )?;
    for canonical in registry::CONTRACTS {
        let mut contract = *canonical;
        contract.schema = &[];
        // Caller-supplied contract pointers must not select or override the codec.
        contract.construction = None;
        let contracts = [contract];
        let admitted = admit(
            &contracts,
            contract.family_id.as_bytes(),
            1,
            &[1],
            &params_digest(&[1]),
        )?;
        assert!(matches!(
            registry::prepare_witness_codec(&admitted, &instance),
            Err(CodecError::DescriptorMismatch)
        ));
        assert_eq!(
            registry::encode_instance(&admitted, &instance),
            Err(CodecError::DescriptorMismatch)
        );
        for (id, revision) in [("quip/unknown", 1), (contract.family_id, 2)] {
            contract.family_id = id;
            contract.revision = revision;
            let contracts = [contract];
            let admitted = admit(
                &contracts,
                id.as_bytes(),
                revision,
                &[1],
                &params_digest(&[1]),
            )?;
            assert!(matches!(
                registry::prepare_witness_codec(&admitted, &instance),
                Err(CodecError::UnsupportedFamily)
            ));
            assert_eq!(
                registry::encode_instance(&admitted, &instance),
                Err(CodecError::UnsupportedFamily)
            );
            assert_eq!(
                registry::decode_instance(&admitted, &[]),
                Err(CodecError::UnsupportedFamily)
            );
        }
    }
    Ok(())
}

#[path = "common/codec.rs"]
#[expect(
    dead_code,
    reason = "the roundtrip helper is compiled here and used by the forthcoming family tests"
)]
mod codec_common;
