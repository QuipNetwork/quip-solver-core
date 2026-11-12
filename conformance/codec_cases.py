"""Read family-local golden schemas without changing their pinned bytes."""

import json
from pathlib import Path

from family_oracle.codecs import FAMILIES

ROOT = Path(__file__).resolve().parents[1]
DIRECTORY = ROOT / "crates/quip-solver-conformance/vectors/families/codecs"


def pick(row, *keys, default=None):
    return next((row[key] for key in keys if key in row), default)


def payload(row):
    return bytes.fromhex(
        pick(row, "encoded_hex", "compact_hex", "compact", "hex", "data", "bytes")
    )


def cases():
    for family, slug in FAMILIES.items():
        rows = json.loads((DIRECTORY / f"{slug}.json").read_text())["cases"]
        for row in rows:
            if row.get("kind", "instance") != "instance":
                continue
            name = pick(row, "name", "id")
            if slug == "flow":
                related = [r for r in rows if r.get("instance") == name]
                valid = [r for r in related if r["kind"] == "witness"]
                bad_witness = [r for r in related if r["kind"] == "witness_error"]
                bad_instance = [r for r in related if r["kind"] == "instance_error"]
            else:
                valid = row.get(
                    "witnesses",
                    [{"spins": row.get("spins"), "compact": row.get("witness")}],
                )
                bad_witness = pick(
                    row,
                    "invalid_witnesses",
                    "malformed_witnesses",
                    "negative_witness",
                    "bad_witnesses",
                    default=[],
                )
                bad_instance = pick(
                    row,
                    "invalid_instances",
                    "malformed_instances",
                    "negative_instance",
                    "malformed",
                    default=[],
                )
            yield dict(
                family=family,
                slug=slug,
                name=name,
                params=bytes.fromhex(pick(row, "params_hex", "params")),
                canonical=bytes.fromhex(pick(row, "canonical_hex", "canonical")),
                compact=bytes.fromhex(
                    pick(row, "compact_instance", "compact_hex", "compact")
                ),
                valid=valid,
                bad_witness=bad_witness,
                bad_instance=bad_instance,
            )
