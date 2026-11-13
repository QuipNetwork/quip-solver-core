"""Write or verify one family's independent compact codec vectors."""

import argparse
import importlib
import json
from pathlib import Path

import blake3

from .build import canonical_json

FAMILIES = (
    "polariton",
    "defects",
    "flow",
    "twisted",
    "charge",
    "glass",
    "fields",
    "routing",
)
ROOT = Path(__file__).resolve().parents[2]


def document(family: str) -> dict:
    if family not in FAMILIES:
        raise ValueError(f"unknown codec family: {family}")
    module = importlib.import_module(f".codec_{family}", __package__)
    cases = module.cases()
    sources = {}
    for name in (
        "codec_common.py",
        "codec_vectors.py",
        "build.py",
        "digests.py",
        "instance.py",
        f"codec_{family}.py",
    ):
        relative = f"conformance/family_oracle/{name}"
        sources[relative] = blake3.blake3((ROOT / relative).read_bytes()).hexdigest()
    return {
        "format": 1,
        "sources": sources,
        "cases": cases,
        "cases_blake3": blake3.blake3(
            canonical_json(cases).encode("ascii")
        ).hexdigest(),
    }


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("write", "check"))
    parser.add_argument("--family", choices=FAMILIES, required=True)
    args = parser.parse_args()
    path = (
        ROOT
        / f"crates/quip-solver-conformance/vectors/families/codecs/{args.family}.json"
    )
    expected = document(args.family)
    if args.action == "write":
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(json.dumps(expected, indent=2, sort_keys=True) + "\n")
    elif json.loads(path.read_text()) != expected:
        raise SystemExit(f"codec vector drift: {path}")


if __name__ == "__main__":
    main()
