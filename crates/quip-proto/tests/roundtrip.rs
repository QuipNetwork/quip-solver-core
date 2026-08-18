use prost::Message;
use quip_proto::v1::{miner_msg, Hello, JobKind, MinerMsg};

#[test]
fn hello_roundtrips_through_prost() {
    let hello = Hello {
        miner_id: "cpu-0".into(),
        session_token: "tok".into(),
        protocol_version: 1,
        backend: "cpu".into(),
        algorithm: "sa".into(),
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4600,
        max_edges: 40000,
        native_topology_hash: None,
        features: vec![],
    };
    let msg = MinerMsg {
        msg: Some(miner_msg::Msg::Hello(hello.clone())),
    };
    let bytes = msg.encode_to_vec();
    let decoded = MinerMsg::decode(&bytes[..]).unwrap();
    match decoded.msg.unwrap() {
        miner_msg::Msg::Hello(h) => assert_eq!(h.protocol_version, 1),
        _ => panic!("wrong variant"),
    }
}

#[test]
fn capabilities_roundtrips() {
    use prost::Message as _;
    use quip_proto::v1::{Capabilities, JobKind};

    let c = Capabilities {
        backend: "cuda".to_owned(),
        algorithm: "sa".to_owned(),
        supported_kinds: vec![JobKind::IsingSample as i32],
        max_nodes: 4096,
        max_edges: 32768,
        features: vec!["streaming".to_owned(), "governor".to_owned()],
        protocol_version: 1,
        stream_width: 8,
        native_topology_hash: None,
    };
    let bytes = c.encode_to_vec();
    assert_eq!(Capabilities::decode(&bytes[..]).expect("decode"), c);
}
