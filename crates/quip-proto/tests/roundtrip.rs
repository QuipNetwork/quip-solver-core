use prost::Message;
use quip_proto::v1::{miner_msg, Hello, JobKind, MinerMsg};

#[test]
fn hello_roundtrips_through_prost() {
    let hello = Hello {
        miner_id: "cpu-0".into(),
        session_token: "tok".into(),
        capabilities: Some(quip_proto::v1::Capabilities {
            protocol_version: 2,
            backend: quip_proto::v1::Backend::Cpu as i32,
            algorithm: quip_proto::v1::Algorithm::Sa as i32,
            supported_kinds: vec![JobKind::IsingSample as i32],
            max_nodes: 4600,
            max_edges: 40000,
            native_topology_hash: None,
            features: vec![],
            stream_width: 4,
            encodings: vec![quip_proto::v1::CoefficientEncoding::I32 as i32],
            generators: vec![],
            max_degree: None,
            max_local_field_milli: None,
            exact_resolution_milli: None,
        }),
    };
    let msg = MinerMsg {
        msg: Some(miner_msg::Msg::Hello(hello.clone())),
    };
    let bytes = msg.encode_to_vec();
    let decoded = MinerMsg::decode(&bytes[..]).unwrap();
    match decoded.msg.unwrap() {
        miner_msg::Msg::Hello(h) => assert_eq!(h.capabilities.unwrap().protocol_version, 2),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn capabilities_roundtrips() {
    use prost::Message as _;
    use quip_proto::v1::{Capabilities, JobKind};

    let c = Capabilities {
        encodings: vec![quip_proto::v1::CoefficientEncoding::I32 as i32],
        generators: vec![],
        backend: quip_proto::v1::Backend::Cuda as i32,
        algorithm: quip_proto::v1::Algorithm::Sa as i32,
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4096,
        max_edges: 32768,
        features: vec!["streaming".to_owned(), "governor".to_owned()],
        protocol_version: 2,
        stream_width: 8,
        native_topology_hash: None,
        max_degree: Some(20),
        max_local_field_milli: Some(63_000),
        exact_resolution_milli: Some(1000),
    };
    let bytes = c.encode_to_vec();
    assert_eq!(Capabilities::decode(&bytes[..]).expect("decode"), c);
}

/// Optional model limits keep their presence across the wire: an explicit `0`
/// (unbounded) is distinct from an unset field (no claim).
#[test]
fn model_limits_keep_presence() {
    use prost::Message as _;
    use quip_proto::v1::Capabilities;

    let unbounded = Capabilities {
        max_degree: Some(0),
        max_local_field_milli: Some(0),
        ..Default::default()
    };
    let decoded = Capabilities::decode(&unbounded.encode_to_vec()[..]).expect("decode");
    assert_eq!(decoded.max_degree, Some(0));
    assert_eq!(decoded.max_local_field_milli, Some(0));
    assert_eq!(decoded.exact_resolution_milli, None);

    let unset = Capabilities::default();
    assert!(unset.encode_to_vec().is_empty());
    let decoded = Capabilities::decode(&[][..]).expect("decode");
    assert_eq!(decoded.max_degree, None);
}
