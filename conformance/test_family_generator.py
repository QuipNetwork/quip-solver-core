"""families.Generator against families.generate, for every registered family.

The prepared handle, its batch entry point, and the int64 buffers must give
the instance that the module-level generate gives, byte for byte, with the
same skips and the same admission errors.
"""

import json
from pathlib import Path

import numpy as np
import pytest
from family_oracle import ice, topology_blob
from family_oracle.params import (
    CHARGED_ICE,
    ICE_FIELDS,
    ICE_GLASS,
    TWISTED_DIMER,
    decode_params,
    encode_params,
)
from quip_solver_core import AdmissionError, FamilyError, SaltSkipped, families

VECTORS = (
    Path(__file__).resolve().parents[1]
    / "crates/quip-solver-conformance/vectors/families"
)
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
# Constant bytes and counters, plus the vector nonces of each case.
NONCES = [bytes([i]) * 32 for i in range(24)] + [
    i.to_bytes(4, "little") + bytes(28) for i in range(24)
]


def _load(name: str) -> dict:
    return json.loads((VECTORS / f"{name}.json").read_text())


def _topology(ref: str) -> tuple[list[int], list[tuple[int, int]]]:
    body = topology_blob.expand(topology_blob.decode((VECTORS / ref).read_bytes()))
    return list(body["nodes"]), [(a, b) for a, b in body["edges"]]


def _args(case: dict) -> tuple:
    nodes, edges = _topology(case["topology"]["topology"])
    return (
        case["family_id"],
        case["revision"],
        bytes.fromhex(case["params_hex"]),
        bytes.fromhex(case["params_digest"]),
        nodes,
        edges,
    )


def _nonces(case: dict) -> list[bytes]:
    own = [bytes.fromhex(i["nonce"]) for i in case["instances"]]
    own += [bytes.fromhex(s["nonce"]) for s in case.get("skipped", [])]
    return own + NONCES


def _cases() -> list:
    return [
        pytest.param(case, id=f"{name}:{case['name']}")
        for name in FILES
        for case in _load(name)["cases"]
    ]


def _zephyr_z12_cases() -> list:
    """The four ice families on the full Zephyr z(12, 4), 4800 nodes."""
    nodes, edges = ice.lattice(12, 4)
    edges = [tuple(e) for e in edges]
    out = []
    for schema, name, case_name in [
        (TWISTED_DIMER, "twisted_dimer", "twisted-dimer-zephyr-z22"),
        (CHARGED_ICE, "ice_charge", "charged-ice-zephyr-z22"),
        (ICE_FIELDS, "ice_fields", "ice-fields-zephyr-z22"),
        (ICE_GLASS, "ice_structure", "ice-glass-zephyr-z22-damaged"),
    ]:
        case = next(c for c in _load(name)["cases"] if c["name"] == case_name)
        _, values = decode_params(schema, bytes.fromhex(case["params_hex"]))
        params = encode_params(schema, dict(values, zephyr_m=12, zephyr_t=4))
        args = (
            case["family_id"],
            case["revision"],
            params,
            families.params_digest(params),
            nodes,
            edges,
        )
        out.append(pytest.param(args, id=f"z12t4:{case['family_id']}"))
    return out


def _outcome(call):
    """The instance of `call()`, or the class, code, and name it raised."""
    try:
        return call()
    except FamilyError as error:
        return (type(error), error.code, error.name)


def _same_instance(got, want) -> None:
    assert got.canonical_bytes() == want.canonical_bytes()
    assert got.digest == want.digest
    assert got.descriptor_digest == want.descriptor_digest
    assert got.topology_order_digest == want.topology_order_digest
    assert got.nonce == want.nonce
    assert got.node_ids == want.node_ids
    assert got.edges == want.edges
    assert (got.h, got.j) == (want.h, want.j)
    assert got.offset_milli == want.offset_milli
    assert got.metadata == want.metadata


def _same_buffers(instance) -> None:
    def read(buf):
        return np.frombuffer(buf, dtype="<i8").tolist()

    assert read(instance.node_ids_i64) == instance.node_ids
    assert read(instance.h_i64) == instance.h
    assert read(instance.j_i64) == instance.j
    flat = [end for edge in instance.edges for end in edge]
    assert read(instance.edges_i64) == flat


def _check(args: tuple, nonces: list[bytes]) -> None:
    gen = families.Generator(*args)
    want = [_outcome(lambda n=n: families.generate(*args, n)) for n in nonces]
    single = [_outcome(lambda n=n: gen.generate(n)) for n in nonces]
    for threads in (1, 4):
        batch = gen.generate_batch(nonces, threads=threads)
        assert len(batch) == len(nonces)
        for got, expected in zip(batch, want, strict=True):
            if isinstance(expected, tuple):
                assert isinstance(got, SaltSkipped)
                assert (type(got), got.code, got.name) == expected
            else:
                _same_instance(got, expected)
    for got, expected in zip(single, want, strict=True):
        if isinstance(expected, tuple):
            assert got == expected
        else:
            _same_instance(got, expected)
            _same_buffers(got)


@pytest.mark.parametrize("case", _cases())
def test_generator_matches_generate(case):
    _check(_args(case), _nonces(case))


@pytest.mark.parametrize("args", _zephyr_z12_cases())
def test_generator_matches_generate_on_zephyr_z12(args):
    _check(args, NONCES[:8])


def test_every_registered_family_is_covered():
    covered = {
        (c["family_id"], c["revision"]) for name in FILES for c in _load(name)["cases"]
    }
    assert set(families.supported()) <= covered


def test_generator_raises_the_admission_errors_of_generate():
    nonce = bytes(32)
    for name in FILES:
        for row in _load(name).get("admission", []):
            if row["expected_code"] is None:
                continue
            nodes, edges = _topology(row["topology"]["topology"])
            params = bytes.fromhex(row["params_hex"])
            args = (
                row["family_id"],
                row["revision"],
                params,
                families.params_digest(params),
                nodes,
                edges,
            )
            want = _outcome(lambda a=args: families.generate(*a, nonce))
            assert want[0] is AdmissionError
            assert _outcome(lambda a=args: families.Generator(*a)) == want


def test_advertised_limits_raise_admission_errors():
    case = _load("ice_charge")["cases"][0]
    for limit in ({"max_nodes": 1}, {"max_edges": 1}, {"max_degree": 1}):
        with pytest.raises(AdmissionError) as caught:
            families.Generator(*_args(case), **limit)
        assert (caught.value.code, caught.value.name) == (13, "BoundsExceeded")


def test_a_bad_nonce_is_a_value_error():
    gen = families.Generator(*_args(_load("ice_charge")["cases"][0]))
    with pytest.raises(ValueError, match="32 bytes"):
        gen.generate(bytes(31))
    with pytest.raises(ValueError, match="nonce 1 must be exactly 32 bytes"):
        gen.generate_batch([bytes(32), bytes(33)])


def test_an_empty_batch_is_empty():
    gen = families.Generator(*_args(_load("ice_charge")["cases"][0]))
    assert gen.generate_batch([]) == []
