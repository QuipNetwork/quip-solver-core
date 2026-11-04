"""Drift and traceability tests for the generated family vectors.

These tests only read fixtures. They never write or regenerate one.
"""

import json
import re

import blake3
import pytest
from family_oracle import build


def _spec_ids() -> set[str]:
    text = build.SPEC_PATH.read_text()
    tail = text.split("## Traceability", 1)[1]
    return set(re.findall(r"SC-2:([a-z0-9-]+)", tail))


def _core_doc() -> dict:
    return json.loads(build.CORE_PATH.read_text())


def _legacy_doc() -> dict:
    return json.loads(build.LEGACY_PATH.read_text())


def _layered_doc() -> dict:
    return json.loads(build.LAYERED_PATH.read_text())


def test_core_fixture_matches_oracle():
    assert build.render(build.build_core()) == build.CORE_PATH.read_text()


def test_legacy_fixture_matches_oracle():
    assert build.render(build.build_legacy()) == build.LEGACY_PATH.read_text()


def test_layered_fixture_matches_oracle():
    assert build.render(build.build_layered()) == build.LAYERED_PATH.read_text()


def _ice_doc() -> dict:
    return json.loads(build.ICE_STRUCTURE_PATH.read_text())


def _twisted_doc() -> dict:
    return json.loads(build.TWISTED_DIMER_PATH.read_text())


def _charge_doc() -> dict:
    return json.loads(build.ICE_CHARGE_PATH.read_text())


def test_ice_charge_fixture_matches_oracle():
    text = build.render(build.build_ice_charge())
    assert text == build.ICE_CHARGE_PATH.read_text()


def test_twisted_dimer_fixture_matches_oracle():
    text = build.render(build.build_twisted_dimer())
    assert text == build.TWISTED_DIMER_PATH.read_text()


def test_ice_structure_fixture_matches_oracle():
    text = build.render(build.build_ice_structure())
    assert text == build.ICE_STRUCTURE_PATH.read_text()


def test_topology_index_matches_oracle():
    text = build.render(build.build_topology_index())
    assert text == build.INDEX_PATH.read_text()


def test_file_matches_compares_payloads_not_compressed_bytes(tmp_path):
    data = build.topology_blob.encode(
        {"kind": "explicit", "nodes": [1, 2], "edges": [[1, 2]]}
    )
    path = tmp_path / "x.bin.gz"
    path.write_bytes(data)
    assert build.file_matches(path, data)
    # The same payload in another DEFLATE stream (stored, not compressed).
    payload = build.topology_blob.decompress(data)
    stored = (
        data[:10]
        + b"\x01"
        + len(payload).to_bytes(2, "little")
        + (len(payload) ^ 0xFFFF).to_bytes(2, "little")
        + payload
        + data[-8:]
    )
    assert stored != data
    path.write_bytes(stored)
    assert build.file_matches(path, data)
    path.write_bytes(data[:-1])
    assert not build.file_matches(path, data)
    other = build.topology_blob.encode({"kind": "explicit", "nodes": [1], "edges": []})
    assert not build.file_matches(path, other)


def test_topology_blobs_match_oracle():
    blobs = build.topology_blobs()
    assert blobs
    for path, data in blobs.items():
        # Payloads, not compressed bytes: another zlib build gives other DEFLATE bytes.
        assert build.file_matches(path, data), path.name


def test_topology_directory_holds_only_known_files():
    on_disk = {path.name for path in build.TOPOLOGIES_DIR.iterdir()}
    expected = {path.name for path in build.topology_blobs()}
    expected |= {build.INDEX_PATH.name, build.LIVE_BLOB_PATH.name}
    assert on_disk == expected


def test_every_json_topology_reference_names_an_indexed_file():
    index = {entry["file"] for entry in json.loads(build.INDEX_PATH.read_text())}
    refs: list[str] = []

    def walk(value):
        if isinstance(value, str) and value.endswith(".bin.gz"):
            refs.append(value)
        elif isinstance(value, list):
            for item in value:
                walk(item)
        elif isinstance(value, dict):
            for item in value.values():
                walk(item)

    for load in (_core_doc, _layered_doc, _ice_doc, _twisted_doc, _charge_doc):
        walk(load())
    assert refs
    # Every topology file is referenced by a JSON file and indexed.
    names = {(build.FAMILIES_DIR / ref).resolve().name for ref in refs}
    assert names == index == {p.name for p in build.TOPOLOGIES_DIR.glob("*.bin.gz")}
    for ref in refs:
        target = (build.FAMILIES_DIR / ref).resolve()
        assert target.parent == build.TOPOLOGIES_DIR.resolve(), ref
        assert target.name in index, ref
        assert target.exists(), ref


def test_ice_structure_evidence_is_sc8():
    doc = _ice_doc()
    tagged = [
        tag for key in ("structure", "live") for c in doc[key] for tag in c["evidence"]
    ]
    assert tagged
    assert set(tagged) == {"SC-8:ice-structure-vectors"}


def test_ice_glass_cases_evidence_matches_the_registry():
    registry = json.loads((build.FAMILIES_DIR / "registry.json").read_text())[
        "registry"
    ]
    entry = next(e for e in registry if e["name"] == "quip/ice-glass@1")
    tagged = {tag for case in _ice_doc()["cases"] for tag in case["evidence"]}
    assert tagged
    assert tagged == set(entry["evidence"])


def test_twisted_dimer_evidence_matches_the_registry():
    registry = json.loads((build.FAMILIES_DIR / "registry.json").read_text())[
        "registry"
    ]
    entry = next(e for e in registry if e["name"] == "quip/twisted-ice-dimer@1")
    doc = _twisted_doc()
    for section in ("cases", "recipes", "obstructions", "admission"):
        tagged = {tag for case in doc[section] for tag in case["evidence"]}
        assert tagged == set(entry["evidence"]), section


def test_charged_ice_evidence_matches_the_registry():
    registry = json.loads((build.FAMILIES_DIR / "registry.json").read_text())[
        "registry"
    ]
    entry = next(e for e in registry if e["name"] == "quip/charged-ice@1")
    doc = _charge_doc()
    for section in ("cases", "energies", "recipe", "admission"):
        tagged = {tag for case in doc[section] for tag in case["evidence"]}
        assert tagged == set(entry["evidence"]), section


def test_charged_ice_pin_matches_the_cases():
    registry = json.loads((build.FAMILIES_DIR / "registry.json").read_text())[
        "registry"
    ]
    entry = next(e for e in registry if e["name"] == "quip/charged-ice@1")
    digest = blake3.blake3(build.canonical_json(_charge_doc()["cases"]).encode())
    assert digest.hexdigest() == entry["vectors_blake3"]


def test_charged_ice_encoded_energy_equals_direct_energy():
    energies = _charge_doc()["energies"]
    assert energies
    for entry in energies:
        assert len(entry["states"]) == 4
        for state in entry["states"]:
            assert state["encoded_milli"] == state["direct_milli"]


def _decoded_params(params_hex: str) -> dict[int, int]:
    """The tag-to-value map of canonical parameter bytes."""
    data = bytes.fromhex(params_hex)
    assert data[0] == 1
    at, values = 1, {}
    while at < len(data):
        tag = int.from_bytes(data[at : at + 2], "little")
        size = int.from_bytes(data[at + 2 : at + 6], "little")
        values[tag] = int.from_bytes(data[at + 6 : at + 6 + size], "little")
        at += 6 + size
    return values


def test_twisted_contract_recipe_table_matches_the_vectors():
    text = (build.ROOT / "docs/families/quip-twisted-ice-dimer-1.md").read_text()
    row = re.compile(
        r"^\| `([^`]+)` \| `([^`]+)` \| (\d+) \| (\d+) \| `([0-9a-f]{64})` \|$",
        re.MULTILINE,
    )
    table = row.findall(text)
    recipes = _twisted_doc()["recipes"]
    assert len(table) == len(recipes) > 0
    for (alias, cell, weak, pairs, digest), recipe in zip(table, recipes, strict=True):
        values = _decoded_params(recipe["params_hex"])
        assert alias == recipe["alias"]
        assert cell == recipe["cell"]
        assert int(weak) == values[3]
        assert int(pairs) == values[4]
        assert digest == recipe["params_digest"]


def test_charged_ice_contract_recipe_table_matches_the_vectors():
    text = (build.ROOT / "docs/families/quip-charged-ice-1.md").read_text()
    row = re.compile(
        r"^\| `([^`]+)` \| `([^`]+)` \| (\d+) \| (\d+) \| (\d+) \| `([0-9a-f]{64})` \|$",
        re.MULTILINE,
    )
    table = row.findall(text)
    recipes = _charge_doc()["recipe"]
    assert len(table) == len(recipes) > 0
    for (alias, cell, scale, bg, charged, digest), recipe in zip(
        table, recipes, strict=True
    ):
        values = _decoded_params(recipe["params_hex"])
        assert alias == recipe["alias"]
        assert cell == recipe["cell"]
        assert int(scale) == values[3]
        assert int(bg) == values[4]
        assert int(charged) == values[5]
        assert digest == recipe["params_digest"]


def test_canonical_json_is_sorted_compact_and_rejects_floats():
    assert build.canonical_json({"b": [1, "x"], "a": None}) == '{"a":null,"b":[1,"x"]}'
    with pytest.raises(TypeError):
        build.canonical_json({"a": 1.5})
    with pytest.raises(TypeError):
        build.canonical_json({"a": "caf\u00e9"})


def test_every_sc2_id_is_provided_partial_or_deferred():
    evidence = build.EVIDENCE
    provided = set(evidence["provided"])
    partial = {item["id"] for item in evidence["partial"]}
    deferred = {item["id"] for item in evidence["deferred"]}
    assert len(provided) == len(evidence["provided"])
    assert not provided & partial
    assert not provided & deferred
    assert not partial & deferred
    assert provided | partial | deferred == _spec_ids()
    assert len(_spec_ids()) == 51


def test_every_provided_id_has_a_case():
    tagged: set[str] = set()
    for doc in (_core_doc(), _legacy_doc()):
        for value in doc.values():
            if isinstance(value, list):
                for case in value:
                    if isinstance(case, dict):
                        tagged.update(case.get("evidence", []))
    wanted = set(build.EVIDENCE["provided"])
    wanted |= {item["id"] for item in build.EVIDENCE["partial"]}
    assert wanted <= {tag.removeprefix("SC-2:") for tag in tagged}
    for tag in tagged:
        assert tag.startswith(("SC-2:", "SC-10:"))


@pytest.mark.parametrize(
    "load",
    [_core_doc, _legacy_doc, _layered_doc, _ice_doc, _twisted_doc, _charge_doc],
)
def test_sources_hash_spec_and_oracle(load):
    doc = load()
    root = build.SPEC_PATH.parents[1]
    paths = ["docs/problem-families.md"] + [
        f"conformance/family_oracle/{p.name}"
        for p in (root / "conformance/family_oracle").glob("*.py")
    ]
    expected = [
        {"path": path, "blake3": blake3.blake3((root / path).read_bytes()).hexdigest()}
        for path in sorted(paths)
    ]
    assert doc["sources"] == expected


def test_no_evidence_item_waits_for_a_finished_ticket():
    items = build.EVIDENCE["partial"] + build.EVIDENCE["deferred"]
    assert all("SC-5" not in item["deferred_to"] for item in items)


def test_layered_evidence_is_sc7():
    text = build.SPEC_PATH.read_text()
    table = set(re.findall(r"SC-7:([a-z0-9-]+)", text.split("## Traceability", 1)[1]))
    tagged = [tag for case in _layered_doc()["cases"] for tag in case["evidence"]]
    assert tagged
    for tag in tagged:
        assert tag.startswith("SC-7:")
        assert tag.removeprefix("SC-7:") in table
