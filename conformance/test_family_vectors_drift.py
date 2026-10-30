"""Drift and traceability tests for the generated family vectors.

These tests only read fixtures. They never write or regenerate one.
"""

import json
import re

import blake3
from family_oracle import build

# Provided by legacy.json (Task 11), so no core.json case carries it.
LEGACY_ONLY = {"legacy-replay-bit-for-bit"}


def _spec_ids() -> set[str]:
    text = build.SPEC_PATH.read_text()
    tail = text.split("## Traceability", 1)[1]
    return set(re.findall(r"SC-2:([a-z0-9-]+)", tail))


def _core_doc() -> dict:
    return json.loads(build.CORE_PATH.read_text())


def test_core_fixture_matches_oracle():
    assert build.render(build.build_core()) == build.CORE_PATH.read_text()


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
    doc = _core_doc()
    tagged: set[str] = set()
    for value in doc.values():
        if isinstance(value, list):
            for case in value:
                if isinstance(case, dict):
                    tagged.update(case.get("evidence", []))
    wanted = set(build.EVIDENCE["provided"]) - LEGACY_ONLY
    wanted |= {item["id"] for item in build.EVIDENCE["partial"]}
    assert wanted <= {tag.removeprefix("SC-2:") for tag in tagged}
    for tag in tagged:
        assert tag.startswith("SC-2:")


def test_sources_hash_spec_and_oracle():
    doc = _core_doc()
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
