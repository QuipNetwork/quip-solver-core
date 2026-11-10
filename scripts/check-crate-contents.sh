#!/usr/bin/env bash
# Verifies that the packaged crates carry the family files.
#
# `cargo package` checks that a crate builds from its own tarball. It does not
# check that the tarball holds every file a consumer needs at run time. The
# family vectors are read by path and the family generators are separate
# modules, so a crate that drops one still builds, and the consumer finds out
# when a test or a pin check fails. This reads the .crate files and fails first.
#
# Usage: check-crate-contents.sh [package-dir]
# The directory defaults to ${CARGO_TARGET_DIR:-target}/package.
set -euo pipefail

PKG_DIR=${1:-${CARGO_TARGET_DIR:-target}/package}
# The crates.io upload limit for one crate.
LIMIT=$((10 * 1024 * 1024))

REGISTRY_PATH=crates/quip-solver-conformance/vectors/families/registry.json
VECTOR_DIR=crates/quip-solver-conformance/vectors/families

if [ ! -d "$VECTOR_DIR" ]; then
    echo "No such directory: $VECTOR_DIR (run from the repository root)" >&2
    exit 1
fi

find_crate() {
    local crates
    mapfile -t crates < <(find "$PKG_DIR" -maxdepth 1 -name "$1-[0-9]*.crate")
    if [ "${#crates[@]}" -ne 1 ]; then
        echo "Expected one $1 .crate in $PKG_DIR, found ${#crates[@]}" >&2
        exit 1
    fi
    printf '%s\n' "${crates[0]}"
}

MISSING=0

# Checks that a tarball listing holds each path below the root directory.
# $1 is the tarball name, $2 the root directory, $3 the listing, and the rest
# are the paths to find.
require() {
    local crate=$1 root=$2 contents=$3
    shift 3
    local name
    for name in "$@"; do
        if printf '%s\n' "$contents" | grep -Fxq "$root/$name"; then
            echo "  ok      $name"
        else
            echo "  MISSING $name (not in $crate)" >&2
            MISSING=1
        fi
    done
}

check_size() {
    local crate=$1 size
    size=$(stat -c %s "$crate")
    echo "  size    $size bytes"
    if [ "$size" -ge "$LIMIT" ]; then
        echo "  TOO BIG $crate is $size bytes, over the crates.io limit of $LIMIT" >&2
        MISSING=1
    fi
}

# --- ising-generators ---------------------------------------------------------
GEN=$(find_crate ising-generators)
echo "$GEN"
GEN_CONTENTS=$(tar tzf "$GEN")
GEN_ROOT=${GEN_CONTENTS%%/*}
require "$GEN" "$GEN_ROOT" "$GEN_CONTENTS" \
    src/registry.rs src/defects.rs src/flow.rs src/native.rs
check_size "$GEN"

# --- quip-solver-conformance -------------------------------------------------
CONF=$(find_crate quip-solver-conformance)
echo "$CONF"
CONF_CONTENTS=$(tar tzf "$CONF")
CONF_ROOT=${CONF_CONTENTS%%/*}

# The registry inside the tarball is the one that decides which files must be
# there. Reading the working tree would miss a registry that the package left
# out or changed.
REGISTRY_PATH_IN_CRATE=${REGISTRY_PATH#crates/quip-solver-conformance/}
REGISTRY_JSON=$(tar xzOf "$CONF" "$CONF_ROOT/$REGISTRY_PATH_IN_CRATE")

# Each vectors path in the registry is relative to the repository root.
# Command substitution, not process substitution, so a failed read stops the
# script under `set -e`.
REGISTERED_TEXT=$(
    printf '%s\n' "$REGISTRY_JSON" \
        | python3 -I -c '
import json, sys
for entry in json.load(sys.stdin)["registry"]:
    print(entry["vectors"].removeprefix("crates/quip-solver-conformance/"))
' | sort -u
)
mapfile -t REGISTERED <<<"$REGISTERED_TEXT"
if [ -z "$REGISTERED_TEXT" ]; then
    echo "The packaged registry lists no vector files" >&2
    exit 1
fi

# Every vectors/families/*.json in the working tree, so a new file cannot be
# left out of the package without notice.
mapfile -t ON_DISK < <(
    for f in "$VECTOR_DIR"/*.json; do
        printf '%s\n' "${f#crates/quip-solver-conformance/}"
    done
)

mapfile -t WANTED < <(printf '%s\n' "$REGISTRY_PATH_IN_CRATE" "${REGISTERED[@]}" "${ON_DISK[@]}" | sort -u)
require "$CONF" "$CONF_ROOT" "$CONF_CONTENTS" "${WANTED[@]}"
check_size "$CONF"

if [ "$MISSING" -ne 0 ]; then
    cat >&2 <<'MSG'

A packaged crate is missing a family file, or it is too big to upload. Check
the `include` and `exclude` lists in the crate manifest, and the size of the
vector files.
MSG
    exit 1
fi

echo "The packaged crates carry the family files."
