//! Wire lease validation and topology registration order.
#![cfg(feature = "session")]
use quip_proto::v1::{
    EdgeList, FamilyDescriptor, GeneratorAlgorithm, IsingProblemGenerator, Result as JobResult,
    SetTarget, Solution, Topology,
};
use quip_protocol::lease::{
    verify_lease_result, verify_lease_solutions, Generator, LeaseError, LeaseSpec, TopologyError,
    TopologyView, VerifyError,
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

fn three_node_topology() -> TopologyView {
    TopologyView {
        num_nodes: 3,
        edges: vec![(0, 2)],
        allowed_h_milli: vec![1000],
        allowed_j_milli: vec![-1000],
    }
}

fn permissive_target() -> Target {
    Target::from_proto(&SetTarget {
        max_energy_milli: i64::MAX,
        min_solutions: 1,
        min_diversity_milli: 0,
        max_proof_solutions: 32,
        ..Default::default()
    })
}

#[test]
fn proto_result_verifies_and_rejects_malformed_fields() {
    let g = generator();
    let lease = LeaseSpec::from_proto(&g).unwrap();
    let topology = three_node_topology();
    let target = permissive_target();
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
fn a_legacy_lease_with_stray_or_invalid_fields_is_rejected() {
    let cases = [
        (
            "a family descriptor",
            with_family(generator()),
            LeaseError::DescriptorUnexpected,
        ),
        (
            "an empty present family descriptor",
            IsingProblemGenerator {
                family: Some(FamilyDescriptor::default()),
                ..generator()
            },
            LeaseError::DescriptorUnexpected,
        ),
        (
            "topology order digest length 32",
            IsingProblemGenerator {
                topology_order_digest: vec![9; 32],
                ..generator()
            },
            LeaseError::DescriptorUnexpected,
        ),
        (
            "topology order digest length 31",
            IsingProblemGenerator {
                topology_order_digest: vec![9; 31],
                ..generator()
            },
            LeaseError::DescriptorUnexpected,
        ),
        (
            "zero count beats the descriptor",
            IsingProblemGenerator {
                salt_count: 0,
                ..with_family(generator())
            },
            LeaseError::ZeroCount,
        ),
        (
            "counter overflow beats the descriptor",
            IsingProblemGenerator {
                salt_start: u64::MAX,
                ..with_family(generator())
            },
            LeaseError::CounterOverflow,
        ),
        (
            "short base salt beats the descriptor",
            IsingProblemGenerator {
                base_salt: vec![3; 31],
                ..with_family(generator())
            },
            LeaseError::BadLength { field: "base_salt" },
        ),
    ];
    for (label, g, expected) in cases {
        assert_eq!(LeaseSpec::from_proto(&g), Err(expected), "{label}");
    }
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
fn a_family_lease_is_validated_in_order() {
    let cases = [
        (
            "no descriptor, digest length 0",
            IsingProblemGenerator {
                family: None,
                topology_order_digest: vec![9; 0],
                ..family_lease()
            },
            LeaseError::DescriptorMissing,
        ),
        (
            "no descriptor, digest length 31",
            IsingProblemGenerator {
                family: None,
                topology_order_digest: vec![9; 31],
                ..family_lease()
            },
            LeaseError::DescriptorMissing,
        ),
        (
            "no descriptor, digest length 33",
            IsingProblemGenerator {
                family: None,
                topology_order_digest: vec![9; 33],
                ..family_lease()
            },
            LeaseError::DescriptorMissing,
        ),
        (
            "descriptor, bad digest length 31",
            IsingProblemGenerator {
                topology_order_digest: vec![9; 31],
                ..family_lease()
            },
            LeaseError::DescriptorMissing,
        ),
        (
            "zero count beats the missing descriptor",
            IsingProblemGenerator {
                family: None,
                salt_count: 0,
                ..family_lease()
            },
            LeaseError::ZeroCount,
        ),
        (
            "short base salt beats the missing descriptor",
            IsingProblemGenerator {
                family: None,
                base_salt: vec![3; 31],
                ..family_lease()
            },
            LeaseError::BadLength { field: "base_salt" },
        ),
        (
            "generator value 0",
            IsingProblemGenerator {
                algorithm: 0,
                ..family_lease()
            },
            LeaseError::UnknownGenerator(0),
        ),
        (
            "generator value 3",
            IsingProblemGenerator {
                algorithm: 3,
                ..family_lease()
            },
            LeaseError::UnknownGenerator(3),
        ),
        (
            "generator value 7",
            IsingProblemGenerator {
                algorithm: 7,
                ..family_lease()
            },
            LeaseError::UnknownGenerator(7),
        ),
    ];
    for (label, g, expected) in cases {
        assert_eq!(LeaseSpec::from_proto(&g), Err(expected), "{label}");
    }
}

#[test]
fn verify_refuses_lease_shapes_it_cannot_score() {
    let topology = three_node_topology();
    let target = permissive_target();
    let lease = LeaseSpec::from_proto(&generator()).unwrap();
    let result = JobResult {
        salt: lease.salt(1).unwrap().to_vec(),
        nonce: lease.nonce(1).unwrap().to_vec(),
        ..Default::default()
    };
    let cases = [
        ("a family lease", family_lease(), VerifyError::FamilyLease),
        (
            "a family lease without a descriptor",
            IsingProblemGenerator {
                family: None,
                ..family_lease()
            },
            VerifyError::FamilyLease,
        ),
        (
            "a legacy lease with a family",
            with_family(generator()),
            VerifyError::SaltOutsideLease,
        ),
    ];
    for (label, g, expected) in cases {
        assert_eq!(
            verify_lease_result(&g, &topology, &target, &result),
            Err(expected),
            "{label}"
        );
    }
}

#[test]
fn verify_solutions_refuses_a_family_spec() {
    let lease = LeaseSpec::from_proto(&family_lease()).unwrap();
    assert_eq!(lease.generator, Generator::FamilyChacha8V1);
    let topology = three_node_topology();
    let target = permissive_target();
    let salt = lease.salt(1).unwrap();
    let nonce = lease.nonce(1).unwrap();
    assert_eq!(
        verify_lease_solutions(&lease, &topology, &target, &salt, &nonce, &[]),
        Err(VerifyError::FamilyLease)
    );
}
