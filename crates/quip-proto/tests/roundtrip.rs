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
    };
    let bytes = c.encode_to_vec();
    assert_eq!(Capabilities::decode(&bytes[..]).expect("decode"), c);
}
