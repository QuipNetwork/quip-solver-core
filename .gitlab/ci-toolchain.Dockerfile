# CI toolchain image: the Rust toolchain, protoc, Node.js, a Python virtual
# environment, the shell linters, and wasm-pack.
#
# The `build-toolchain-image` job in `.gitlab-ci.yml` builds and pushes this
# image. The job runs automatically when this file changes and is manual
# otherwise. The build context is empty: nothing from the repository, no npm
# packages, and no cargo registry goes into the image.
#
# Jobs reference the image by digest. After a rebuild, copy the digest the job
# prints into TOOLCHAIN_IMAGE in `.gitlab-ci.yml`.

# Nothing watches these pins for updates. Check the upstream release pages on
# rebuild. The Rust version must match rust-toolchain.toml.
FROM rust:1.97.1@sha256:b1b3c9c0d921d7fa0a6d1f9ec7e4eab87f8c8ec97644c3d791450f131dec813f

# Must match the protoc that generated npm/src/generated. ts-proto stamps the
# protoc version into the generated header, so an unpinned compiler trips the
# stub drift guard on the version line alone. Bump PROTOC_VERSION and
# regenerate npm/src/generated together, the same way the Python guard pins
# grpcio-tools.
ARG PROTOC_VERSION=35.1
# sha256 of protoc-35.1-linux-x86_64.zip, as published in the GitHub release
# asset digest for tag v35.1 and confirmed against the downloaded file. Bump it
# together with PROTOC_VERSION; the release page carries the new digest.
ARG PROTOC_SHA256=6930ebf62bd4ea607b98fff052596c6ee564b9835b4ce172c75a3f53ae9d91b7
# shfmt is pinned because a release can change the formatting it considers
# canonical, which would fail the lint job on untouched files. The sha256 is
# the GitHub release asset digest for v3.13.1, confirmed against the downloaded
# binary. shfmt is not packaged in trixie at a version worth pinning to.
ARG SHFMT_VERSION=3.13.1
ARG SHFMT_SHA256=fb096c5d1ac6beabbdbaa2874d025badb03ee07929f0c9ff67563ce8c75398b1
# wasm-pack is installed from crates.io at a pinned version rather than through
# the upstream `curl | sh` installer, which resolves whatever release is
# current and is not checksummed.
ARG WASM_PACK_VERSION=0.15.0
# The nodesource channel, asserted after install so a silent channel change
# cannot leave an unexpected major on PATH.
ARG NODE_MAJOR=24
# 11.5.1 is the floor for npm OIDC trusted publishing.
ARG NPM_VERSION=11.21.2
# The checked-in python/quip/v1/*_pb2.py were generated with grpcio-tools 1.82.1
# (protoc gencode 7.35.0). A newer grpcio-tools stamps a different gencode
# version and trips the stub drift diff. Bump this pin and regenerate the stubs
# together.
ARG GRPCIO_TOOLS_VERSION=1.82.1
# Pinned because ruff's default rule set grows between minors, so an unpinned
# install turns upstream releases into red pipelines. Bump deliberately and fix
# the new findings in the same change.
ARG RUFF_VERSION=0.16.2
ARG MATURIN_VERSION=1.16.0
ARG TWINE_VERSION=7.0.0

# pipefail, because `curl ... | bash -` reports only bash's status, and bash
# reading an empty stdin exits 0.
SHELL ["/bin/bash", "-euo", "pipefail", "-c"]

# python3 provides the interpreter for the virtual environment below and for
# the pyo3 build script, which probes an interpreter even for a check-only
# build.
# hadolint ignore=DL3008
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        curl unzip make g++ jq shellcheck python3 python3-venv python3-pip \
    && rm -rf /var/lib/apt/lists/*

# Verify before unzipping. protoc generates the wire types every artifact in
# this repository is built from, so a substituted compiler would be signed and
# published by the release jobs.
RUN curl -fsSLo /tmp/protoc.zip "https://github.com/protocolbuffers/protobuf/releases/download/v${PROTOC_VERSION}/protoc-${PROTOC_VERSION}-linux-x86_64.zip" \
    && echo "${PROTOC_SHA256}  /tmp/protoc.zip" | sha256sum -c - \
    && unzip -q -o /tmp/protoc.zip -d /usr/local \
    && rm /tmp/protoc.zip

RUN curl -fsSLo /usr/local/bin/shfmt "https://github.com/mvdan/sh/releases/download/v${SHFMT_VERSION}/shfmt_v${SHFMT_VERSION}_linux_amd64" \
    && echo "${SHFMT_SHA256}  /usr/local/bin/shfmt" | sha256sum -c - \
    && chmod +x /usr/local/bin/shfmt

# hadolint ignore=DL3008
RUN curl -fsSL "https://deb.nodesource.com/setup_${NODE_MAJOR}.x" | bash - \
    && apt-get install -y --no-install-recommends nodejs \
    && rm -rf /var/lib/apt/lists/* \
    && HAVE="$(node --version)" \
    && if [ "${HAVE%%.*}" != "v${NODE_MAJOR}" ]; then \
        echo "expected node ${NODE_MAJOR}.x, got ${HAVE}" >&2; exit 1; \
    fi \
    && npm install -g "npm@${NPM_VERSION}" \
    && npm cache clean --force

# A virtual environment rather than --break-system-packages: pip cannot
# uninstall Debian's `packaging` (no RECORD file), so installing twine into the
# system interpreter fails outright.
RUN python3 -m venv /opt/venv \
    && /opt/venv/bin/pip install --no-cache-dir --quiet \
        "maturin==${MATURIN_VERSION}" "twine==${TWINE_VERSION}" \
        "grpcio-tools==${GRPCIO_TOOLS_VERSION}" "ruff==${RUFF_VERSION}"
ENV PATH="/opt/venv/bin:${PATH}"

# The base image ships rustc and cargo only. RUSTUP_TOOLCHAIN pins the
# already-installed toolchain and makes rustup ignore rust-toolchain.toml for
# these commands, which avoids a channel re-sync that dies on `detected
# conflict: bin/cargo-fmt`. The cargo registry is removed afterwards so the
# image carries only the installed binary.
RUN export RUSTUP_TOOLCHAIN=1.97.1 \
    && rustup component add clippy rustfmt \
    && rustup target add wasm32-unknown-unknown \
    && cargo install wasm-pack --version "${WASM_PACK_VERSION}" --locked \
    && rm -rf "${CARGO_HOME}/registry" "${CARGO_HOME}/git"

# Fail the image build here, not in a CI job. sed rather than head, because head
# closes the pipe early and pipefail turns the resulting SIGPIPE into a failure.
RUN rustc --version && cargo --version && cargo clippy --version \
    && cargo fmt --version && rustup target list --installed \
    && wasm-pack --version && protoc --version \
    && node --version && npm --version \
    && python3 --version && pip --version \
    && maturin --version && twine --version \
    && python -m grpc_tools.protoc --version && ruff --version \
    && shellcheck --version && shfmt --version \
    && make --version | sed -n 1p && g++ --version | sed -n 1p \
    && jq --version && curl --version | sed -n 1p && unzip -v | sed -n 1p
