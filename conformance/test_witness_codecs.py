"""Integration checks for both codec surfaces and every local golden schema."""

import random

import pytest
from codec_cases import ROOT, cases, payload
from family_oracle import codecs
from family_oracle.codec_common import CodecError

CASES = list(cases())


@pytest.fixture(scope="module")
def core():
    import quip_solver_core

    return quip_solver_core


def test_codec_exports(core):
    assert issubclass(core.CodecError, ValueError)
    assert not issubclass(core.CodecError, core.FamilyError)
    for name in (
        "encode_instance",
        "decode_instance",
        "encode_witness",
        "decode_witness",
    ):
        assert callable(getattr(core.families, name, None))


@pytest.mark.parametrize("case", CASES, ids=lambda c: c["slug"] + ":" + c["name"])
def test_wheel_matches_codec_vectors(core, case):
    f = core.families
    context = (case["family"], 1, case["params"], f.params_digest(case["params"]))
    instance = f.decode_instance(*context, case["compact"])
    assert instance.canonical_bytes() == case["canonical"]
    assert f.encode_instance(*context, instance) == case["compact"]
    independent = codecs.decode_instance(
        case["family"], 1, case["params"], case["compact"]
    )
    assert instance.digest == independent["digest"]
    for bad in case["bad_instance"]:
        with pytest.raises(core.CodecError) as error:
            f.decode_instance(*context, payload(bad))
        assert error.value.name == bad["error"]
    # Structural-superset fixtures deliberately fail family validation.
    if (case["slug"] == "routing" and case["name"] == "wider") or (
        case["slug"] == "flow" and case["name"] == "flow-structural-nongenerated"
    ):
        with pytest.raises(core.CodecError) as error:
            f.encode_witness(*context, instance, [1] * len(instance.node_ids))
        assert error.value.name == "invalid_instance"
        return
    for valid in case["valid"]:
        encoded = f.encode_witness(*context, instance, valid["spins"])
        assert isinstance(encoded, bytes)
        assert encoded == payload(valid)
        assert f.decode_witness(*context, instance, encoded) == valid["spins"]
    for bad in case["bad_witness"]:
        with pytest.raises(core.CodecError) as error:
            if "spins" in bad:
                f.encode_witness(*context, instance, bad["spins"])
            else:
                f.decode_witness(*context, instance, payload(bad))
        assert error.value.name == bad["error"]


@pytest.mark.parametrize("family", codecs.FAMILIES)
def test_decoder_canonical_properties(family):
    case = next(c for c in CASES if c["family"] == family)
    instance = codecs.decode_instance(family, 1, case["params"], case["compact"])
    rng = random.Random(2311)
    data_rows = [rng.randbytes(i % 40) for i in range(1024)]
    if len(payload(case["valid"][0])) == 1:
        data_rows += [bytes([i]) for i in range(256)]
    for data in data_rows:
        try:
            spins = codecs.decode(family, 1, instance, data)
        except CodecError:
            pass
        else:
            assert codecs.encode(family, 1, instance, spins) == data
    for i in range(1024):
        data = bytearray(case["compact"])
        data[rng.randrange(len(data))] ^= 1 << (i % 8)
        try:
            decoded = codecs.decode_instance(family, 1, case["params"], bytes(data))
        except CodecError:
            pass
        else:
            assert codecs.encode_instance(family, 1, case["params"], decoded) == data


def test_wheel_context_and_lengths(core):
    case = CASES[0]
    f = core.families
    context = (case["family"], 1, case["params"], f.params_digest(case["params"]))
    instance = f.decode_instance(*context, case["compact"])
    with pytest.raises(core.CodecError) as error:
        f.decode_witness(*context, instance, bytes(131074))
    assert error.value.name == "length"
    changed = bytearray(case["compact"])
    changed[0] ^= 1
    with pytest.raises(core.CodecError) as error:
        f.decode_instance(*context, bytes(changed))
    assert error.value.name == "descriptor_mismatch"
    unsupported = (case["family"], 2, *context[2:])
    for name, args in [
        ("decode_instance", [case["compact"]]),
        ("encode_instance", [instance]),
        ("encode_witness", [instance, case["valid"][0]["spins"]]),
        ("decode_witness", [instance, b""]),
    ]:
        with pytest.raises(core.AdmissionError):
            getattr(f, name)(*unsupported, *args)


def size_rows():
    import json

    rows = json.loads(
        (ROOT / "conformance/fixtures/witness_codec_sizes.json").read_text()
    )
    assert rows
    assert {r["family_id"] for r in rows} == set(codecs.FAMILIES)
    return rows


def test_size_rows_obey_byte_formulas():
    for row in size_rows():
        if row["witness_status"] != "measured":
            continue
        n = row["nodes"]
        assert row["one_bit_witness_bits"] == n
        assert row["one_bit_witness_bytes"] == (n + 7) // 8
        bits = row["independent_bits"]
        if bits is None:
            bits = 1 + n
            if row["mode"] == "structured":
                bits -= row["pairs"] + row["zero_tiles"] + 2 * row["charged_tiles"]
        assert row["compact_witness_bits"] == bits
        assert row["compact_witness_bytes"] == (bits + 7) // 8


def test_instance_size_rows_obey_formulas():
    for row in size_rows():
        if row["instance_status"] != "measured":
            continue
        assert (
            row["canonical_instance_bytes"]
            == 113 + 8 * row["nodes"] + 12 * row["edges"] + row["metadata_bytes"]
        )
        assert (
            row["compact_instance_bytes"]
            == row["prefix_bytes"] + row["compact_metadata_bytes"]
        )
        formula = row["instance_formula"]
        size = formula["scalar_bytes"]
        size += sum(1 + (c["count"] * c["width"] + 7) // 8 for c in formula["columns"])
        size += sum((bits + 7) // 8 for bits in formula["bit_arrays"])
        assert size == row["compact_instance_bytes"]
        for kind in ("canonical", "compact"):
            assert row[f"{kind}_instance_bits"] == 8 * row[f"{kind}_instance_bytes"]


def test_wheel_descriptor_mismatch_and_borrowed_instance(core):
    case = CASES[0]
    f = core.families
    context = (case["family"], 1, case["params"], f.params_digest(case["params"]))
    instance = f.decode_instance(*context, case["compact"])
    other = next(c for c in CASES if c["slug"] == "charge")
    other_context = (
        other["family"],
        1,
        other["params"],
        f.params_digest(other["params"]),
    )
    for operation, args in [
        ("encode_instance", [instance]),
        ("encode_witness", [instance, case["valid"][0]["spins"]]),
        ("decode_witness", [instance, b""]),
    ]:
        with pytest.raises(core.CodecError) as error:
            getattr(f, operation)(*other_context, *args)
        assert error.value.name == "descriptor_mismatch"
    with pytest.raises(core.CodecError) as error:
        f.decode_instance(*context, case["compact"] + bytes(131074))
    assert error.value.name == "length"


def test_defects_structure_precedes_metadata_minimality(core):
    from family_oracle import codec_defects
    from family_oracle.codec_common import encode_column, preflight_prefix, read_column

    case = next(c for c in CASES if c["slug"] == "defects")
    compact = case["compact"]
    prefix = preflight_prefix(compact, codec_defects.LIMITS)
    side = codec_defects._params(case["params"])["side"]
    start = prefix.metadata_start + 4 + (2 * (side - 1) ** 2 + 7) // 8
    positions, used = read_column(compact[start:], prefix.node_count)
    width = positions.width + 1
    packed = sum(positions.get(i) << (i * width) for i in range(positions.count))
    wider = bytes([width]) + packed.to_bytes(
        (positions.count * width + 7) // 8, "little"
    )
    changed = compact[:start] + wider + compact[start + used :]
    _, ids_len = read_column(compact[112:], prefix.node_count)
    _, endpoints_len = read_column(compact[112 + ids_len :], 2 * prefix.edge_count)
    at = 112 + ids_len
    changed = (
        changed[:at]
        + encode_column([0] * (2 * prefix.edge_count))
        + changed[at + endpoints_len :]
    )
    with pytest.raises(CodecError) as error:
        codecs.decode_instance(case["family"], 1, case["params"], changed)
    assert error.value.name == "invalid_instance"
    with pytest.raises(core.CodecError) as error:
        core.families.decode_instance(
            case["family"],
            1,
            case["params"],
            core.families.params_digest(case["params"]),
            changed,
        )
    assert error.value.name == "invalid_instance"
