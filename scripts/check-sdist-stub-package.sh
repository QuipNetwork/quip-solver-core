#!/usr/bin/env bash
# Verifies that the sdist carries the generated `quip` gRPC stub package.
#
# quip_solver_core/__init__.py imports from `quip.v1`, so a distribution
# without the package installs successfully and then fails at first import
# with `ModuleNotFoundError: No module named 'quip'`. twine check does not
# look inside the tarball, so this does.
#
# This happened for real: 0.0.0's pyproject marked the package wheel-only
# ([tool.maturin] include, format = "wheel"), so every consumer that fell
# back to the sdist — macOS and aarch64 Linux, which had no wheel — built a
# broken install.
set -euo pipefail

SDIST=${1:?usage: check-sdist-stub-package.sh <sdist.tar.gz>}

if [ ! -f "$SDIST" ]; then
    echo "No such sdist: $SDIST" >&2
    exit 1
fi

# Read the listing once. Deriving the root with `tar | head` instead would kill
# tar with SIGPIPE, which pipefail then reports as a failure of this script.
CONTENTS=$(tar tzf "$SDIST")

ROOT=${CONTENTS%%/*}
if [ -z "$ROOT" ] || [ "$ROOT" = "$CONTENTS" ]; then
    echo "Could not read the root directory of $SDIST" >&2
    exit 1
fi

MISSING=0
for name in \
    python/quip/__init__.py \
    python/quip/v1/__init__.py \
    python/quip/v1/miner_pb2.py \
    python/quip/v1/miner_pb2_grpc.py; do
    if printf '%s\n' "$CONTENTS" | grep -Fxq "$ROOT/$name"; then
        echo "  ok      $name"
    else
        echo "  MISSING $name" >&2
        MISSING=1
    fi
done

if [ "$MISSING" -ne 0 ]; then
    cat >&2 <<'MSG'

The sdist is missing part of the `quip` stub package, so an install built
from it fails at import time. Keep `quip/**/*.py` in [tool.maturin] include
with format = ["sdist", "wheel"].
MSG
    exit 1
fi

echo "$SDIST carries the quip stub package."
