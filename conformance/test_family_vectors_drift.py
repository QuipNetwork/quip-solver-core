"""Drift and traceability tests for the generated family vectors.

These tests only read fixtures. They never write or regenerate one.
"""

import json
import re

import blake3
import pytest
from family_oracle import build, topology_blob


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


@pytest.mark.parametrize(
    ("build_fn", "path"),
    [
        pytest.param(build.build_core, build.CORE_PATH, id="core"),
        pytest.param(build.build_legacy, build.LEGACY_PATH, id="legacy"),
        pytest.param(build.build_layered, build.LAYERED_PATH, id="layered"),
        pytest.param(build.build_polariton, build.POLARITON_PATH, id="polariton"),
        pytest.param(build.build_defects, build.DEFECTS_PATH, id="defects"),
        pytest.param(
            build.build_two_commodity, build.TWO_COMMODITY_PATH, id="two-commodity"
        ),
        pytest.param(build.build_ice_fields, build.ICE_FIELDS_PATH, id="ice-fields"),
        pytest.param(build.build_ice_charge, build.ICE_CHARGE_PATH, id="ice-charge"),
        pytest.param(
            build.build_twisted_dimer, build.TWISTED_DIMER_PATH, id="twisted-dimer"
        ),
        pytest.param(
            build.build_ice_structure, build.ICE_STRUCTURE_PATH, id="ice-structure"
        ),
        pytest.param(build.build_topology_index, build.INDEX_PATH, id="topology-index"),
    ],
)
def test_fixture_matches_oracle(build_fn, path):
    assert build.render(build_fn()) == path.read_text()


def _ice_doc() -> dict:
    return json.loads(build.ICE_STRUCTURE_PATH.read_text())


def _twisted_doc() -> dict:
    return json.loads(build.TWISTED_DIMER_PATH.read_text())


def _charge_doc() -> dict:
    return json.loads(build.ICE_CHARGE_PATH.read_text())


def _fields_doc() -> dict:
    return json.loads(build.ICE_FIELDS_PATH.read_text())


def _routing_doc() -> dict:
    return json.loads(build.TWO_COMMODITY_PATH.read_text())


def _polariton_doc() -> dict:
    return json.loads(build.POLARITON_PATH.read_text())


def _defects_doc() -> dict:
    return json.loads(build.DEFECTS_PATH.read_text())


def _registry_entry(name: str) -> dict:
    registry = json.loads((build.FAMILIES_DIR / "registry.json").read_text())[
        "registry"
    ]
    return next(e for e in registry if e["name"] == name)


@pytest.mark.parametrize(
    ("name", "load", "sections"),
    [
        pytest.param(
            "quip/polariton-static@1",
            _polariton_doc,
            ("cases", "witnesses", "polariton_recipes", "admission"),
            id="polariton",
        ),
        pytest.param(
            "quip/defects-static@1",
            _defects_doc,
            ("cases", "defect_witnesses", "routes", "table", "admission"),
            id="defects",
        ),
        pytest.param(
            "quip/two-commodity-routing@1",
            _routing_doc,
            ("cases", "terminals", "routing_recipes", "admission"),
            id="two-commodity",
        ),
        pytest.param("quip/ice-glass@1", _ice_doc, ("cases",), id="ice-glass"),
        pytest.param(
            "quip/twisted-ice-dimer@1",
            _twisted_doc,
            ("cases", "recipes", "obstructions", "admission"),
            id="twisted-dimer",
        ),
        pytest.param(
            "quip/charged-ice@1",
            _charge_doc,
            ("cases", "energies", "recipe", "admission"),
            id="charged-ice",
        ),
        pytest.param(
            "quip/ice-fields@1",
            _fields_doc,
            ("cases", "selection", "field_recipes", "admission"),
            id="ice-fields",
        ),
    ],
)
def test_evidence_matches_the_registry(name, load, sections):
    entry = _registry_entry(name)
    doc = load()
    for section in sections:
        tagged = {tag for case in doc[section] for tag in case["evidence"]}
        assert tagged, section
        assert tagged == set(entry["evidence"]), section


@pytest.mark.parametrize(
    ("name", "load"),
    [
        pytest.param("quip/polariton-static@1", _polariton_doc, id="polariton"),
        pytest.param("quip/defects-static@1", _defects_doc, id="defects"),
        pytest.param("quip/two-commodity-routing@1", _routing_doc, id="two-commodity"),
        pytest.param("quip/charged-ice@1", _charge_doc, id="charged-ice"),
        pytest.param("quip/ice-fields@1", _fields_doc, id="ice-fields"),
    ],
)
def test_pin_matches_the_cases(name, load):
    entry = _registry_entry(name)
    digest = blake3.blake3(build.canonical_json(load()["cases"]).encode())
    assert digest.hexdigest() == entry["vectors_blake3"]


def _check_polariton_witnesses(doc: dict) -> None:
    by_case: dict[str, list[dict]] = {}
    for witness in doc["witnesses"]:
        by_case.setdefault(witness["case"], []).append(witness)
    assert set(by_case) == {case["name"] for case in doc["cases"]}
    cases = {case["name"]: case for case in doc["cases"]}
    for name, found in by_case.items():
        nodes = {
            inst["index"]: len(inst["node_ids"]) for inst in cases[name]["instances"]
        }
        for witness in found:
            assert len(witness["spins"]) == nodes.get(witness["index"]), name
        kinds = sorted(w["feasible"] for w in found)
        assert kinds == [False, True], name
        for witness in found:
            kind = "feasible" if witness["feasible"] else "infeasible"
            assert witness["name"] == f"{name}-{kind}"
            # One violated label costs 500 milli. A feasible witness costs 0.
            expected = 0 if witness["feasible"] else 500
            assert witness["energy_milli"] == witness["loss_milli"] == expected


def test_polariton_witness_coverage_rejects_a_swap_and_a_higher_energy():
    doc = _polariton_doc()
    swapped = json.loads(json.dumps(doc))
    l2 = [w for w in doc["witnesses"] if w["case"] == "polariton-l2"]
    retry = [
        i for i, w in enumerate(doc["witnesses"]) if w["case"] == "polariton-retry"
    ]
    for slot, witness in zip(retry, l2, strict=True):
        copy = dict(witness)
        copy["case"] = "polariton-retry"
        copy["name"] = witness["name"].replace("polariton-l2", "polariton-retry")
        swapped["witnesses"][slot] = copy
    with pytest.raises(AssertionError):
        _check_polariton_witnesses(swapped)
    loud = json.loads(json.dumps(doc))
    bad = next(w for w in loud["witnesses"] if not w["feasible"])
    bad["energy_milli"] = bad["loss_milli"] = 32000
    with pytest.raises(AssertionError):
        _check_polariton_witnesses(loud)


def test_oracle_template_equals_the_research_template_on_the_live_topology():
    from family_oracle import routing

    live = topology_blob.expand(topology_blob.decode(build.LIVE_BLOB_PATH.read_bytes()))
    template = routing.template(12, 4, live["nodes"], [tuple(e) for e in live["edges"]])
    fixture = json.loads(
        (
            build.ROOT / "conformance/fixtures/two_commodity_research_template.json"
        ).read_text()
    )
    assert [list(c) for c in template["cells"]] == fixture["nodes_cell"]
    assert [[a["tail"], a["head"]] for a in template["arcs"]] == fixture["arc_nodes"]
    assert [a["x1"] for a in template["arcs"]] == fixture["arc_spin_commodity1"]
    assert [list(a["chain"]) for a in template["arcs"]] == fixture[
        "arc_chain_commodity2"
    ]


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


@pytest.fixture(scope="module")
def topology_blobs():
    return build.topology_blobs()


def test_topology_blobs_match_oracle(topology_blobs):
    blobs = topology_blobs
    assert blobs
    for path, data in blobs.items():
        # Payloads, not compressed bytes: another zlib build gives other DEFLATE bytes.
        assert build.file_matches(path, data), path.name


def test_topology_directory_holds_only_known_files(topology_blobs):
    on_disk = {path.name for path in build.TOPOLOGIES_DIR.iterdir()}
    expected = {path.name for path in topology_blobs}
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

    for load in (
        _core_doc,
        _layered_doc,
        _ice_doc,
        _twisted_doc,
        _charge_doc,
        _fields_doc,
        _routing_doc,
        _polariton_doc,
        _defects_doc,
    ):
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


def test_charged_ice_encoded_energy_equals_direct_energy():
    energies = _charge_doc()["energies"]
    assert energies
    for entry in energies:
        assert len(entry["states"]) == 4
        for state in entry["states"]:
            assert state["encoded_milli"] == state["direct_milli"]


def test_ice_fields_shortfall_cases_skip_every_or_some_salt():
    cases = {c["name"]: c for c in _fields_doc()["cases"]}
    every = cases["ice-fields-shortfall"]
    assert every["skipped"]
    assert not every["instances"]
    mixed = cases["ice-fields-shortfall-mixed"]
    assert mixed["skipped"]
    assert mixed["instances"]
    for case in (every, mixed):
        assert {entry["skip_code"] for entry in case["skipped"]} == {14}


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


@pytest.mark.parametrize(
    ("doc_name", "load", "key", "columns"),
    [
        pytest.param(
            "quip-polariton-static-1.md",
            _polariton_doc,
            "polariton_recipes",
            (("m", 1), ("t", 2), ("side", 3), ("pairs", 4)),
            id="polariton",
        ),
        pytest.param(
            "quip-two-commodity-routing-1.md",
            _routing_doc,
            "routing_recipes",
            (("m", 1), ("t", 2), ("divisor", 3)),
            id="two-commodity",
        ),
        pytest.param(
            "quip-twisted-ice-dimer-1.md",
            _twisted_doc,
            "recipes",
            (("weak", 3), ("pairs", 4)),
            id="twisted-dimer",
        ),
        pytest.param(
            "quip-charged-ice-1.md",
            _charge_doc,
            "recipe",
            (("scale", 3), ("background", 4), ("charged", 5)),
            id="charged-ice",
        ),
        pytest.param(
            "quip-ice-fields-1.md",
            _fields_doc,
            "field_recipes",
            (("glass", 3), ("field", 4), ("divisor", 5)),
            id="ice-fields",
        ),
    ],
)
def test_contract_recipe_table_matches_the_vectors(doc_name, load, key, columns):
    text = (build.ROOT / "docs/families" / doc_name).read_text()
    row = re.compile(
        r"^\| `([^`]+)` \| `([^`]+)` \|"
        + r" (\d+) \|" * len(columns)
        + r" `([0-9a-f]{64})` \|$",
        re.MULTILINE,
    )
    table = row.findall(text)
    recipes = load()[key]
    assert len(table) == len(recipes) > 0
    for (alias, cell, *numbers, digest), recipe in zip(table, recipes, strict=True):
        values = _decoded_params(recipe["params_hex"])
        assert alias == recipe["alias"]
        assert cell == recipe["cell"]
        assert {
            name: int(n) for (name, _), n in zip(columns, numbers, strict=True)
        } == {name: values[tag] for name, tag in columns}
        assert digest == recipe["params_digest"]


def test_polariton_recipe_pairs_follow_the_side():
    for recipe in _polariton_doc()["polariton_recipes"]:
        values = _decoded_params(recipe["params_hex"])
        assert values[4] == (values[3] - 1) ** 2 // 8


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
    [
        _core_doc,
        _legacy_doc,
        _layered_doc,
        _ice_doc,
        _twisted_doc,
        _charge_doc,
        _fields_doc,
        _routing_doc,
        _polariton_doc,
        _defects_doc,
    ],
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


def test_defects_contract_states_the_table_numbers_of_the_vectors():
    text = (build.ROOT / "docs/families/quip-defects-static-1.md").read_text()
    l3 = next(t for t in _defects_doc()["table"] if t["case"] == "defects-l3")
    assert l3["kept"] == 52
    assert f"= {l3['kept_magnitude']}" in text
    assert f"`U = {l3['weight']}`" in text


def test_defects_routes_and_witnesses_cover_the_contract():
    doc = _defects_doc()
    errors = [r["error"] for r in doc["routes"]]
    assert errors[:5] == [None, "Endpoints", "Difference", "Obstacle", "RepeatedVertex"]
    # The two-fault routes name every error that a route can hold, in order.
    assert errors[5:] == [
        "Spins",
        "Shape",
        "Shape",
        "NotAdjacent",
        "NotAdjacent",
        "RepeatedVertex",
        "RepeatedVertex",
        "RepeatedLink",
        "Obstacle",
        "Obstacle",
        "Endpoints",
        "Endpoints",
    ]
    for case in doc["cases"]:
        kinds = {
            w["name"].removeprefix(case["name"] + "-")
            for w in doc["defect_witnesses"]
            if w["case"] == case["name"]
        }
        assert kinds == {"feasible", "infeasible", "pin-violating"}
    assert {w["physical_status"] for w in doc["defect_witnesses"]} == {
        "InsufficientPhysicalEvidence"
    }


def test_defects_table_entries_match_the_oracle_table():
    from family_oracle import defects

    for row in _defects_doc()["table"]:
        case = next(c for c in _defects_doc()["cases"] if c["name"] == row["case"])
        side = _decoded_params(case["params_hex"])[3]
        table = defects.table_of(side)
        assert row["entries"] == len(table.entries)
        assert row["kept"] == len(table.kept)
        assert row["kept_magnitude"] == table.kept_magnitude
        assert row["dropped"] == table.dropped
        assert row["truncation_bound"] == str(table.truncation_bound)
        assert row["commitment"] == table.commitment().hex()
