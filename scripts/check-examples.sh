#!/usr/bin/env bash
# Builds the four sample solvers and runs the conformance gate on each.
#
# SPEC section 6 defines conformance as a green quip-solver-drive run, so this
# is the same check a solver author runs against their own binary. Each example
# consumes a published artifact, which makes this a test of the releases as
# much as a test of the examples.
#
# Set SKIP to a space-separated list to leave languages out, for example
# SKIP="python typescript" when uv or node is unavailable.
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

SKIP="${SKIP:-}"
SOCKET_DIR="$(mktemp -d)"
trap 'rm -rf "$SOCKET_DIR"' EXIT

skipped() {
    case " $SKIP " in
        *" $1 "*) return 0 ;;
        *) return 1 ;;
    esac
}

echo "==> building the conformance driver"
cargo build -p quip-solver-conformance --bin quip-solver-drive --release
DRIVE="$REPO_ROOT/target/release/quip-solver-drive"

# Runs the gate and reports the language, so a failure names itself rather than
# leaving the reader to match a socket path back to an example.
drive() {
    local lang="$1" bin="$2"
    echo "==> conformance: $lang"
    if "$DRIVE" "$bin" "unix://$SOCKET_DIR/$lang.sock"; then
        echo "    $lang CONFORMANT"
    else
        echo "    $lang FAILED" >&2
        return 1
    fi
}

if ! skipped rust; then
    echo "==> building the Rust sample"
    # Resolves quip-solver-core from crates.io, not from this tree.
    (cd examples/rust && cargo build --release)
    drive rust examples/rust/target/release/mock_miner
fi

if ! skipped cpp; then
    echo "==> building the C library and the C++ sample"
    cargo build -p quip-solver-c --release
    make -C examples/cpp mock_miner
    drive cpp examples/cpp/mock_miner
fi

if ! skipped python; then
    echo "==> building the Python sample"
    # The wheel carries the PyO3 consensus primitives the example calls.
    python3 -m venv .venv-examples
    # shellcheck disable=SC1091  # created just above, so it cannot be followed
    . .venv-examples/bin/activate
    pip install --quiet maturin
    maturin develop
    cat > "$SOCKET_DIR/py_miner" <<EOF
#!/bin/sh
exec "$REPO_ROOT/.venv-examples/bin/python" "$REPO_ROOT/examples/python/mock_miner.py" "\$@"
EOF
    chmod +x "$SOCKET_DIR/py_miner"
    drive python "$SOCKET_DIR/py_miner"
    deactivate
fi

if ! skipped typescript; then
    echo "==> building the npm package and the TypeScript sample"
    (cd npm && npm ci && npm run build && npm pack)
    # Installs the tarball built from this tree rather than the registry, so a
    # change to the npm package is exercised before it is published.
    (cd examples/typescript \
        && npm install ../../npm/quip.network-quip-solver-core-*.tgz \
        && npm install \
        && npm run build)
    cat > "$SOCKET_DIR/ts_miner" <<EOF
#!/bin/sh
exec node "$REPO_ROOT/examples/typescript/dist/mock_miner.js" "\$@"
EOF
    chmod +x "$SOCKET_DIR/ts_miner"
    drive typescript "$SOCKET_DIR/ts_miner"
fi

echo
echo "all sample solvers conformant"
