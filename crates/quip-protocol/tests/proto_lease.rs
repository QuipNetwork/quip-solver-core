//! Wire lease validation and topology registration order.
#![cfg(feature = "session")]
use quip_proto::v1::{
    EdgeList, FamilyDescriptor, GeneratorAlgorithm, IsingProblemGenerator, Result as JobResult,
    SetTarget, Solution, Topology,
};
use quip_protocol::lease::{
    verify_lease_result, verify_lease_solutions, Generator, LeaseError, LeaseSpec, TopologyError, TopologyView, VerifyError,
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

fn with_family(mut g: IsingProblemGenerator) -> IsingProblemGenerator {
    g.family = Some(FamilyDescriptor {
        family_id: "quip/x".into(),
        revision: 1,
        ..Default::default()
    });
    g
}

#[test]
fn a_legacy_lease_with_a_family_is_unexpected() {
    assert_eq!(
        LeaseSpec::from_proto(&with_family(generator())),
        Err(LeaseError::DescriptorUnexpected)
    );
}

#[test]
fn an_empty_present_family_is_unexpected() {
    let g = IsingProblemGenerator {
        family: Some(FamilyDescriptor::default()),
        ..generator()
    };
    assert_eq!(
        LeaseSpec::from_proto(&g),
        Err(LeaseError::DescriptorUnexpected)
    );
}

#[test]
fn a_legacy_lease_with_a_topology_order_digest_is_unexpected() {
    for len in [32, 31] {
        let g = IsingProblemGenerator {
            topology_order_digest: vec![9; len],
            ..generator()
        };
        assert_eq!(
            LeaseSpec::from_proto(&g),
            Err(LeaseError::DescriptorUnexpected),
            "digest length {len}"
        );
    }
}

#[test]
fn field_checks_win_over_descriptor_unexpected() {
    let mut zero = with_family(generator());
    zero.salt_count = 0;
    assert_eq!(LeaseSpec::from_proto(&zero), Err(LeaseError::ZeroCount));
    let mut overflow = with_family(generator());
    overflow.salt_start = u64::MAX;
    assert_eq!(
        LeaseSpec::from_proto(&overflow),
        Err(LeaseError::CounterOverflow)
    );
    let mut short = with_family(generator());
    short.base_salt = vec![3; 31];
    assert_eq!(
        LeaseSpec::from_proto(&short),
        Err(LeaseError::BadLength { field: "base_salt" })
    );
}

fn family_lease() -> IsingProblemGenerator {
    IsingProblemGenerator {
        algorithm: GeneratorAlgorithm::FamilyChacha8V1 as i32,
        topology_order_digest: vec![9; 32],
        ..with_family(generator())
    }
}

#[test]
fn generator_value_two_is_a_family_lease() {
    let family = LeaseSpec::from_proto(&family_lease()).unwrap();
    let legacy = LeaseSpec::from_proto(&generator()).unwrap();
    assert_eq!(family.generator, Generator::FamilyChacha8V1);
    for index in 0..4 {
        assert_eq!(family.salt(index), legacy.salt(index));
        assert_eq!(family.nonce(index), legacy.nonce(index));
    }
}

#[test]
fn a_family_lease_without_a_descriptor_is_code_1() {
    for len in [0, 31, 33] {
        let g = IsingProblemGenerator {
            family: None,
            topology_order_digest: vec![9; len],
            ..family_lease()
        };
        assert_eq!(
            LeaseSpec::from_proto(&g),
            Err(LeaseError::DescriptorMissing)
        );
    }
    let g = IsingProblemGenerator {
        topology_order_digest: vec![9; 31],
        ..family_lease()
    };
    assert_eq!(
        LeaseSpec::from_proto(&g),
        Err(LeaseError::DescriptorMissing)
    );
}

#[test]
fn field_checks_come_before_code_1() {
    let g = IsingProblemGenerator {
        family: None,
        salt_count: 0,
        ..family_lease()
    };
    assert_eq!(LeaseSpec::from_proto(&g), Err(LeaseError::ZeroCount));
    let g = IsingProblemGenerator {
        family: None,
        base_salt: vec![3; 31],
        ..family_lease()
    };
    assert_eq!(
        LeaseSpec::from_proto(&g),
        Err(LeaseError::BadLength { field: "base_salt" })
    );
}

#[test]
fn other_generator_values_stay_unknown() {
    for id in [0, 3, 7] {
        let g = IsingProblemGenerator {
            algorithm: id,
            ..family_lease()
        };
        assert_eq!(
            LeaseSpec::from_proto(&g),
            Err(LeaseError::UnknownGenerator(id))
        );
    }
}

#[test]
fn verify_refuses_a_family_lease() {
    let g = family_lease();
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
    let result = JobResult {
        salt: lease.salt(1).unwrap().to_vec(),
        nonce: lease.nonce(1).unwrap().to_vec(),
        ..Default::default()
    };
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result),
        Err(VerifyError::FamilyLease)
    );
    let no_descriptor = IsingProblemGenerator { family: None, ..g };
    assert_eq!(
        verify_lease_result(&no_descriptor, &topology, &target, &result),
        Err(VerifyError::FamilyLease)
    );
}

#[test]
fn verify_solutions_refuses_a_family_spec() {
    let lease = LeaseSpec::from_proto(&family_lease()).unwrap();
    assert_eq!(lease.generator, Generator::FamilyChacha8V1);
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
    let salt = lease.salt(1).unwrap();
    let nonce = lease.nonce(1).unwrap();
    assert_eq!(
        verify_lease_solutions(&lease, &topology, &target, &salt, &nonce, &[]),
        Err(VerifyError::FamilyLease)
    );
}

#[test]
fn verify_refuses_a_legacy_lease_with_a_family() {
    let g = with_family(generator());
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
    let lease = LeaseSpec::from_proto(&generator()).unwrap();
    let result = JobResult {
        salt: lease.salt(1).unwrap().to_vec(),
        nonce: lease.nonce(1).unwrap().to_vec(),
        ..Default::default()
    };
    assert_eq!(
        verify_lease_result(&g, &topology, &target, &result),
        Err(VerifyError::SaltOutsideLease)
    );
}
