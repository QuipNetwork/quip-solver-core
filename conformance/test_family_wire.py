"""Wire-level checks for the problem-family extension (FAM-44, FAM-45).

The reference bytes are the plan's reference table, produced by Python
protobuf 7.35.1 from the edited proto. Each one is parsed and re-serialized
to pin tags, presence, and open-enum behavior.
"""

import json
from pathlib import Path

import pytest
from google.protobuf import json_format
from quip.v1 import miner_pb2

DESCRIPTOR_HEX = "0a06717569702f7810011a01012220" + "aa" * 32
FAMILY_LEASE_HEX = "0802422f" + DESCRIPTOR_HEX + "4a20" + "bb" * 32
BINDING_HEX = "8a010f3202050738fdffffffffffffffff01"
EMPTY_BINDING_HEX = "8a0100"
CAPS_HEX = (
    "8a010361626392010c0a06717569702f78120201029a012d0a06717569702f78"
    "1201011a20" + "cc" * 32
)
REJECT_10_HEX = "100a"

CORE_VECTORS = (
    Path(__file__).resolve().parent.parent
    / "crates/quip-solver-conformance/vectors/families/core.json"
)

REFERENCES = [
    ("descriptor", miner_pb2.FamilyDescriptor, DESCRIPTOR_HEX),
    ("lease", miner_pb2.IsingProblemGenerator, FAMILY_LEASE_HEX),
    ("binding", miner_pb2.IsingProblem, BINDING_HEX),
    ("empty-binding", miner_pb2.IsingProblem, EMPTY_BINDING_HEX),
    ("capabilities", miner_pb2.Capabilities, CAPS_HEX),
    ("reject", miner_pb2.Reject, REJECT_10_HEX),
]


@pytest.mark.parametrize(
    ("name", "cls", "hex_"), REFERENCES, ids=[r[0] for r in REFERENCES]
)
def test_reference_bytes(name, cls, hex_):
    raw = bytes.fromhex(hex_)
    parsed = cls.FromString(raw)
    assert parsed.SerializeToString(deterministic=True) == raw, name


def test_reference_values():
    lease = miner_pb2.IsingProblemGenerator()
    lease.algorithm = miner_pb2.GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1
    lease.family.family_id = "quip/x"
    lease.family.revision = 1
    lease.family.params = b"\x01"
    lease.family.params_digest = b"\xaa" * 32
    lease.topology_order_digest = b"\xbb" * 32
    assert lease.SerializeToString(deterministic=True).hex() == FAMILY_LEASE_HEX

    problem = miner_pb2.IsingProblem()
    problem.family.node_ids.extend([5, 7])
    problem.family.offset_milli = -3
    assert problem.SerializeToString(deterministic=True).hex() == BINDING_HEX


def test_presence():
    empty = miner_pb2.IsingProblem.FromString(bytes.fromhex(EMPTY_BINDING_HEX))
    assert empty.HasField("family")
    assert not miner_pb2.IsingProblem().HasField("family")

    legacy = miner_pb2.IsingProblemGenerator(
        algorithm=miner_pb2.GENERATOR_ALGORITHM_BLAKE3_CHACHA8_V1
    )
    legacy.family.SetInParent()
    assert legacy.SerializeToString(deterministic=True).hex() == "0801" + "4200"


def test_unknown_enum_values():
    lease = miner_pb2.IsingProblemGenerator.FromString(bytes.fromhex("0807"))
    assert lease.algorithm == 7
    with pytest.raises(ValueError, match="7"):
        miner_pb2.GeneratorAlgorithm.Name(7)
    assert lease.SerializeToString(deterministic=True).hex() == "0807"

    caps = miner_pb2.Capabilities.FromString(bytes.fromhex("5a03010207"))
    assert list(caps.generators) == [1, 2, 7]
    assert caps.SerializeToString(deterministic=True).hex() == "5a03010207"


def test_new_enum_numbers():
    assert miner_pb2.GENERATOR_ALGORITHM_FAMILY_CHACHA8_V1 == 2
    assert miner_pb2.FAMILY_UNSUPPORTED == 10


def test_wire_reasons_match_vectors():
    rows = json.loads(CORE_VECTORS.read_text())["wire_reasons"]
    assert len(rows) == 11
    for row in rows:
        assert miner_pb2.RejectReason.Value(row["reason"]) == row["value"]
    names = {row["reason"] for row in rows}
    assert set(miner_pb2.RejectReason.keys()) == names


def test_capabilities_json_names():
    caps = miner_pb2.Capabilities.FromString(bytes.fromhex(CAPS_HEX))
    as_dict = json_format.MessageToDict(caps)
    assert as_dict["solverId"] == "abc"
    generated = as_dict["generatedFamilies"][0]
    assert generated["familyId"] == "quip/x"
    assert generated["revisions"] == [1, 2]
    accepted = as_dict["acceptedFamilies"][0]
    assert accepted["paramsDigests"] == ["zMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMzMw="]
    assert json_format.MessageToDict(miner_pb2.Capabilities()) == {}
