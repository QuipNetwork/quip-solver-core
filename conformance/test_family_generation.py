"""The quip_solver_core.families bindings against the frozen family vectors.

The vectors come from the independent Python oracle. The bindings call the Rust
generator, so every case checks that a Python miner builds the Rust bytes. The
topology blob decoder is the oracle's. The wheel has no decoder of its own.
"""

import json
import subprocess
import sys
import textwrap
from pathlib import Path

import pytest
from family_oracle import topology_blob
from family_oracle.digests import instance_digest
from family_oracle.instance import canonical_bytes
from quip_solver_core import (
    AdmissionError,
    FamilyError,
    GeneratorDefect,
    InstanceInvalid,
    SaltSkipped,
    families,
)

VECTORS = (
    Path(__file__).resolve().parents[1]
    / "crates/quip-solver-conformance/vectors/families"
)

# The vector files of the implemented families. layered.json holds the test
# construction of the vectors/* ids, which the wheel does not implement.
FILES = [
    "defects",
    "flow",
    "ice_charge",
    "ice_fields",
    "ice_structure",
    "polariton",
    "twisted_dimer",
    "two_commodity",
]
HARD = {"quip/defects-static", "quip/flow-static", "quip/polariton-static"}


def _load(name: str) -> dict:
    return json.loads((VECTORS / f"{name}.json").read_text())


def _topology(ref: dict) -> tuple[list[int], list[tuple[int, int]]]:
    body = topology_blob.expand(
        topology_blob.decode((VECTORS / ref["topology"]).read_bytes())
    )
    return list(body["nodes"]), [(a, b) for a, b in body["edges"]]


def _cases(hard: bool | None = None) -> list:
    out = []
    for name in FILES:
        for case in _load(name)["cases"]:
            if hard is not None and (
                (case["family_id"] in HARD) != hard or not case["instances"]
            ):
                continue
            out.append(pytest.param(case, id=f"{name}:{case['name']}"))
    return out


def _instances() -> list:
    out = []
    for name in FILES:
        for case in _load(name)["cases"]:
            for inst in case["instances"]:
                out.append(
                    pytest.param(
                        case, inst, id=f"{name}:{case['name']}:{inst['index']}"
                    )
                )
    return out


def _rows(section: str) -> list:
    out = []
    for name in FILES:
        for row in _load(name).get(section, []):
            out.append(pytest.param(row, id=f"{name}:{row['name']}"))
    return out


def _generate(case: dict, nonce: bytes, **limits):
    nodes, edges = _topology(case["topology"])
    return families.generate(
        case["family_id"],
        case["revision"],
        bytes.fromhex(case["params_hex"]),
        bytes.fromhex(case["params_digest"]),
        nodes,
        edges,
        nonce,
        **limits,
    )


def _validate_args(case: dict, inst: dict, **override):
    args = {
        "family_id": case["family_id"],
        "revision": case["revision"],
        "params": bytes.fromhex(case["params_hex"]),
        "params_digest": bytes.fromhex(case["params_digest"]),
        "descriptor_digest": bytes.fromhex(case["descriptor_digest"]),
        "topology_order_digest": bytes.fromhex(case["topology"]["order_digest"]),
        "nonce": bytes.fromhex(inst["nonce"]),
        "node_ids": inst["node_ids"],
        "edges": [tuple(e) for e in inst["edges"]],
        "h": inst["h"],
        "j": inst["j"],
        "offset_milli": int(inst["offset_milli"]),
        "metadata": bytes.fromhex(inst["metadata_hex"]),
        "instance_digest": bytes.fromhex(inst["instance_digest"]),
    }
    args.update(override)
    return args


def test_supported_matches_the_registry():
    registry = json.loads((VECTORS / "registry.json").read_text())["registry"]
    # vectors/* ids are the layered test construction, not implemented families.
    expected = sorted(
        (e["family_id"], e["revision"])
        for e in registry
        if e["status"] != "archived" and not e["family_id"].startswith("vectors/")
    )
    assert families.supported() == expected
    assert len(expected) == 8


@pytest.mark.parametrize(("case", "inst"), _instances())
def test_generate_matches_every_vector(case, inst):
    instance = _generate(case, bytes.fromhex(inst["nonce"]))
    assert instance.canonical_bytes().hex() == inst["canonical_hex"]
    assert instance.digest.hex() == inst["instance_digest"]
    assert instance.descriptor_digest.hex() == case["descriptor_digest"]
    assert instance.topology_order_digest.hex() == case["topology"]["order_digest"]
    assert instance.nonce.hex() == inst["nonce"]
    assert instance.node_ids == inst["node_ids"]
    assert instance.edges == [tuple(e) for e in inst["edges"]]
    assert (instance.h, instance.j) == (inst["h"], inst["j"])
    assert instance.offset_milli == int(inst["offset_milli"])
    assert instance.metadata.hex() == inst["metadata_hex"]


@pytest.mark.parametrize("case", _cases())
def test_skipped_salts_raise_salt_skipped(case):
    for skip in case["skipped"]:
        with pytest.raises(SaltSkipped) as caught:
            _generate(case, bytes.fromhex(skip["nonce"]))
        assert caught.value.code == skip["skip_code"]
        assert (
            caught.value.name
            == {13: "BoundsExceeded", 14: "NoInstance"}[skip["skip_code"]]
        )


@pytest.mark.parametrize("row", _rows("admission"))
def test_admission_codes_match_the_vectors(row):
    # Code 11 cannot occur: generate sends the topology's own order digest.
    # No admission row has an advertised limit, so every limit stays 0.
    case = {
        "family_id": row["family_id"],
        "revision": row["revision"],
        "params_hex": row["params_hex"],
        "params_digest": families.params_digest(bytes.fromhex(row["params_hex"])).hex(),
        "topology": row["topology"],
    }
    expected = row["expected_code"]
    try:
        _generate(case, bytes(range(32)))
    except AdmissionError as error:
        assert expected == error.code
        assert error.name
    except SaltSkipped:
        assert expected is None
    else:
        assert expected is None


def test_admit_reports_descriptor_codes():
    params = bytes.fromhex(_load("ice_charge")["cases"][0]["params_hex"])
    digest = families.params_digest(params)
    assert families.admit("quip/charged-ice", 1, params, digest) is None
    for args, code, name in [
        (("Bad Id", 1, params, digest), 3, "FamilyIdInvalid"),
        (("quip/charged-ice", 0, params, digest), 4, "RevisionZero"),
        (("quip/unknown", 1, params, digest), 5, "FamilyUnsupported"),
        (("quip/charged-ice", 2, params, digest), 6, "RevisionUnsupported"),
        (("quip/charged-ice", 1, params[:5], digest), 7, "ParamsMalformed"),
        (("quip/charged-ice", 1, params, bytes(32)), 8, "ParamsDigestMismatch"),
    ]:
        with pytest.raises(AdmissionError) as caught:
            families.admit(*args)
        assert (caught.value.code, caught.value.name) == (code, name)


def test_advertised_limits_give_code_13():
    case = _load("ice_charge")["cases"][0]
    inst = case["instances"][0]
    nonce = bytes.fromhex(inst["nonce"])
    for limit in ({"max_nodes": 1}, {"max_edges": 1}, {"max_degree": 1}):
        with pytest.raises(AdmissionError) as caught:
            _generate(case, nonce, **limit)
        assert (caught.value.code, caught.value.name) == (13, "BoundsExceeded")
    assert _generate(case, nonce, max_nodes=10**6, max_edges=10**6, max_degree=10**6)


@pytest.mark.parametrize(("case", "inst"), _instances())
def test_validate_instance_round_trips(case, inst):
    generated = _generate(case, bytes.fromhex(inst["nonce"]))
    checked = families.validate_instance(**_validate_args(case, inst))
    assert checked.canonical_bytes() == generated.canonical_bytes()
    assert checked.digest == generated.digest


def test_validate_instance_rejects_a_bad_digest():
    case = _load("ice_charge")["cases"][0]
    inst = case["instances"][0]
    flipped = bytearray.fromhex(inst["instance_digest"])
    flipped[0] ^= 1
    with pytest.raises(InstanceInvalid) as caught:
        families.validate_instance(
            **_validate_args(case, inst, instance_digest=bytes(flipped))
        )
    assert (caught.value.code, caught.value.name) == (15, "InstanceDigestMismatch")


def _with_metadata(case: dict, inst: dict, metadata: bytes) -> dict:
    """Validate arguments with a new metadata region and a matching digest."""
    raw = canonical_bytes(
        bytes.fromhex(case["descriptor_digest"]),
        bytes.fromhex(case["topology"]["order_digest"]),
        bytes.fromhex(inst["nonce"]),
        inst["node_ids"],
        [tuple(e) for e in inst["edges"]],
        inst["h"],
        inst["j"],
        int(inst["offset_milli"]),
        metadata,
    )
    return _validate_args(
        case, inst, metadata=metadata, instance_digest=instance_digest(raw)
    )


@pytest.mark.parametrize(
    "metadata", [b"\xff\xff", b"\x01\x00"], ids=["layout", "truncated"]
)
def test_validate_instance_rejects_bad_metadata(metadata):
    case = _load("ice_charge")["cases"][0]
    inst = case["instances"][0]
    with pytest.raises(InstanceInvalid) as caught:
        families.validate_instance(**_with_metadata(case, inst, metadata))
    assert (caught.value.code, caught.value.name) == (16, "MetadataInvalid")


def test_validate_instance_rejects_metadata_that_check_metadata_refuses():
    case = _load("polariton")["cases"][0]
    inst = case["instances"][0]
    metadata = bytearray.fromhex(inst["metadata_hex"])
    metadata[-1] ^= 0x01
    with pytest.raises(InstanceInvalid) as caught:
        families.validate_instance(**_with_metadata(case, inst, bytes(metadata)))
    assert caught.value.code == 16


def test_validate_instance_checks_the_descriptor_first():
    case = _load("ice_charge")["cases"][0]
    inst = case["instances"][0]
    with pytest.raises(AdmissionError) as caught:
        families.validate_instance(
            **_validate_args(case, inst, params_digest=bytes(32))
        )
    assert caught.value.code == 8
    with pytest.raises(InstanceInvalid) as caught:
        families.validate_instance(
            **_validate_args(case, inst, descriptor_digest=bytes(32))
        )
    assert caught.value.code == 15


def _witness_rows() -> list:
    out = []
    for name, key in [
        ("polariton", "witnesses"),
        ("defects", "defect_witnesses"),
        ("flow", "flow_witnesses"),
    ]:
        for w in _load(name)[key]:
            out.append(pytest.param(name, w, id=f"{name}:{w['name']}"))
    return out


@pytest.mark.parametrize(("file", "witness"), _witness_rows())
def test_feasible_matches_the_vector_witnesses(file, witness):
    case = next(c for c in _load(file)["cases"] if c["name"] == witness["case"])
    inst = next(i for i in case["instances"] if i["index"] == witness["index"])
    instance = _generate(case, bytes.fromhex(inst["nonce"]))
    assert case["family_id"] in HARD
    result = families.feasible(
        case["family_id"],
        case["revision"],
        bytes.fromhex(case["params_hex"]),
        bytes.fromhex(case["params_digest"]),
        instance,
        witness["spins"],
    )
    assert result is witness["feasible"]


@pytest.mark.parametrize("case", _cases(hard=False))
def test_feasible_for_a_soft_family_checks_the_spin_shape(case):
    instance = _generate(case, bytes.fromhex(case["instances"][0]["nonce"]))
    args = (
        case["family_id"],
        case["revision"],
        bytes.fromhex(case["params_hex"]),
        bytes.fromhex(case["params_digest"]),
        instance,
    )
    n = len(instance.node_ids)
    assert families.feasible(*args, [1] * n) is True
    assert families.feasible(*args, [1] * (n - 1)) is False
    assert families.feasible(*args, [0] * n) is False


def test_import_is_offline():
    code = textwrap.dedent(
        """
        import sys
        opened = []
        sys.addaudithook(lambda e, a: opened.append(e) if e in ("open", "socket.connect") else None)
        import quip_solver_core
        quip_solver_core.families.supported()
        # Only the interpreter's own import machinery may open files.
        print(sorted(set(opened) - {"open"}))
        """
    )
    result = subprocess.run(
        [sys.executable, "-I", "-c", code],
        env={},
        capture_output=True,
        text=True,
        check=True,
    )
    assert result.stdout.strip() == "[]"


def test_errors_carry_code_and_name():
    for cls in (AdmissionError, SaltSkipped, InstanceInvalid, GeneratorDefect):
        assert issubclass(cls, FamilyError)
    assert issubclass(FamilyError, ValueError)
    with pytest.raises(FamilyError) as caught:
        families.admit("quip/unknown", 1, b"", families.params_digest(b""))
    assert isinstance(caught.value, AdmissionError)
    assert isinstance(caught.value.code, int)
    assert isinstance(caught.value.name, str)
    assert (caught.value.code, caught.value.name) == (5, "FamilyUnsupported")


def test_wrong_argument_types_stay_type_errors():
    with pytest.raises(TypeError):
        families.params_digest("text")
    with pytest.raises(TypeError):
        families.admit(1, 1, b"", b"")


def test_a_nonce_of_the_wrong_length_is_a_value_error():
    case = _load("ice_charge")["cases"][0]
    with pytest.raises(ValueError, match="32 bytes") as caught:
        _generate(case, b"short")
    assert not isinstance(caught.value, FamilyError)


def test_a_topology_with_an_unknown_node_is_code_12():
    case = _load("ice_charge")["cases"][0]
    with pytest.raises(AdmissionError) as caught:
        families.generate(
            case["family_id"],
            case["revision"],
            bytes.fromhex(case["params_hex"]),
            bytes.fromhex(case["params_digest"]),
            [0, 1],
            [(0, 9)],
            bytes(32),
        )
    assert caught.value.code == 12
