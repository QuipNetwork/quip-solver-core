#!/usr/bin/env bash
# Verifies that the built wheel carries the `families` module and that it
# lists the registered families.
#
# A wheel built without the `families` bindings installs and imports, and only
# a Python miner that asks for a family finds out. This installs the wheel into
# a clean virtual environment and compares `families.supported()` with the
# registry, so the check sees what a consumer sees.
#
# Usage: check-wheel-families.sh <wheel.whl>
set -euo pipefail

WHEEL=${1:?usage: check-wheel-families.sh <wheel.whl>}
REGISTRY=crates/quip-solver-conformance/vectors/families/registry.json

if [ ! -f "$WHEEL" ]; then
    echo "No such wheel: $WHEEL" >&2
    exit 1
fi
if [ ! -f "$REGISTRY" ]; then
    echo "No such registry: $REGISTRY (run from the repository root)" >&2
    exit 1
fi

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

python3 -m venv "$WORK/venv"
"$WORK/venv/bin/python" -m pip install --quiet --disable-pip-version-check "$WHEEL"

# -I keeps the current directory off the import path, so the check sees the
# installed wheel and not a source tree.
"$WORK/venv/bin/python" -I - "$REGISTRY" <<'PY'
import json
import sys

from quip_solver_core import families

# The same rule as the registry: archived entries and the `vectors/*` test
# constructions are not part of what a solver implements.
with open(sys.argv[1], encoding="utf-8") as handle:
    entries = json.load(handle)["registry"]
expected = sorted(
    (e["family_id"], e["revision"])
    for e in entries
    if e["status"] != "archived" and not e["family_id"].startswith("vectors/")
)
actual = sorted(families.supported())
if actual != expected:
    print("families.supported() differs from registry.json", file=sys.stderr)
    print(f"  only in the registry: {sorted(set(expected) - set(actual))}", file=sys.stderr)
    print(f"  only in the wheel:    {sorted(set(actual) - set(expected))}", file=sys.stderr)
    sys.exit(1)
for family_id, revision in actual:
    print(f"  ok      {family_id}@{revision}")
PY

echo "$WHEEL carries the families module and lists every registered family."
