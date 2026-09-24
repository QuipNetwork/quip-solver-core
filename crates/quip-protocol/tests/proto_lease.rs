//! Wire lease validation and topology registration order.
#![cfg(feature = "session")]
use quip_proto::v1::{
    EdgeList, GeneratorAlgorithm, IsingProblemGenerator, Result as JobResult, SetTarget, Solution,
    Topology,
};
use quip_protocol::lease::{
    verify_lease_result, LeaseError, LeaseSpec, TopologyError, TopologyView, VerifyError,
};
use quip_protocol::target::Target;
use quip_protocol::wire::encode_spins_packed;

fn generator() -> IsingProblemGenerator {
    IsingProblemGenerator {
        algorithm: GeneratorAlgorithm::Blake3Chacha8V1 as i32,
        last_proof_block_hash: vec![1; 32],
        miner_account: vec![2; 32],
        base_salt: vec![3; 32],
        salt_start: 5,
        salt_count: 4,
        ..Default::default()
    }
}

#[test]
fn topology_preserves_registration_order_and_rejects_invalid_edges() {
    let mut t = Topology {
        nodes: vec![90, 2, 41],
        edges: Some(EdgeList {
            u: vec![41],
            v: vec![90],
        }),
        allowed_h_milli: vec![1000],
        allowed_j_milli: vec![-1000],
        ..Default::default()
    };
    let view = TopologyView::from_proto(&t).unwrap();
    assert_eq!(view.edges, vec![(2, 0)]);
    assert_eq!(view.allowed_j_milli, vec![-1000]);
    t.nodes.push(90);
    assert_eq!(
        TopologyView::from_proto(&t),
        Err(TopologyError::DuplicateNode(90))
    );
    let _ = t.nodes.pop();
    t.edges.as_mut().unwrap().v = vec![7];
    assert_eq!(
        TopologyView::from_proto(&t),
        Err(TopologyError::UnknownNode(7))
    );
    t.edges.as_mut().unwrap().v.clear();
    assert_eq!(
        TopologyView::from_proto(&t),
        Err(TopologyError::UnequalEdgeLists)
    );
}

#[test]
fn proto_result_verifies_and_rejects_malformed_fields() {
    let mut g = generator();
    let lease = LeaseSpec::from_proto(&g).unwrap();
    let topology = TopologyView {
        num_nodes: 3,
        edges: vec![(0, 2)],
        allowed_h_milli: vec![1000],
        allowed_j_milli: vec![-1000],
    };
    let target = Target::from_proto(&SetTarget {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        min_diversity_milli: 0,
        max_proof_solutions: 32,
        ..Default::default()
    });
    let spins = vec![1, -1, 1];
    let nonce = lease.nonce(1).unwrap();
    let (h, j) = topology.draw(nonce).unwrap();
    let energy_milli = quip_protocol::scoring::energy_from_milli(&spins, &h, &j, &topology.edges);
    let mut result = JobResult {
        salt: lease.salt(1).unwrap().to_vec(),
        nonce: nonce.to_vec(),
        solutions: vec![Solution {
            spins: encode_spins_packed(&spins),
            energy_milli,
        }],
        ..Default::default()
    };
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result)
            .unwrap()
            .stats
            .valid_solution_count,
        1
    );
    result.solutions.first_mut().unwrap().spins = vec![0x80];
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result),
        Err(VerifyError::MalformedSpins { index: 0 })
    );
    result.nonce.clear();
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result),
        Err(VerifyError::NonceMismatch)
    );
    result.salt.clear();
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result),
        Err(VerifyError::SaltOutsideLease)
    );
    g.algorithm = 99;
    assert_eq!(
        LeaseSpec::from_proto(&g),
        Err(LeaseError::UnknownGenerator(99))
    );
    g = generator();
    g.base_salt.clear();
    assert_eq!(
        LeaseSpec::from_proto(&g),
        Err(LeaseError::BadLength { field: "base_salt" })
    );
}
