//! Shared checks for family compact instance adapters.
use ising_generators::{
    descriptor::Admitted,
    instance::Instance,
    registry::{decode_instance, encode_instance},
};

#[expect(clippy::expect_used, reason = "a codec failure is a test failure")]
pub(crate) fn assert_instance_roundtrip(admitted: &Admitted<'_>, instance: &Instance) {
    let compact = encode_instance(admitted, instance).expect("encode compact instance");
    let decoded = decode_instance(admitted, &compact).expect("decode compact instance");
    assert_eq!(
        instance.canonical_bytes().expect("original bytes"),
        decoded.canonical_bytes().expect("decoded bytes")
    );
    assert_eq!(instance.digest(), decoded.digest());
    assert_eq!(
        encode_instance(admitted, &decoded).expect("re-encode instance"),
        compact
    );
}
