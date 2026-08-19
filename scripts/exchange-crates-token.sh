#!/usr/bin/env bash
# Exchanges the GitLab OIDC token for a short-lived crates.io publish token.
#
# crates.io Trusted Publishing gained GitLab CI/CD support in January 2026, so
# nothing here needs a long-lived CARGO_REGISTRY_TOKEN. The pipeline proves its
# own identity instead, and the minted token expires in 30 minutes.
#
# The caller supplies CRATES_IO_ID_TOKEN through `id_tokens:` with audience
# `crates.io`, and uses the output as CARGO_REGISTRY_TOKEN:
#
#   export CARGO_REGISTRY_TOKEN="$(bash scripts/exchange-crates-token.sh)"
#
# Every crate published from this pipeline needs a matching Trusted Publisher
# registered at crates.io/crates/<name>/settings. Without one the exchange is
# rejected, so a missing registration fails here rather than mid-publish.
#
# Only stdout carries the token. Diagnostics go to stderr so `$(...)` capture
# stays clean.
set -euo pipefail

: "${CRATES_IO_ID_TOKEN:?CRATES_IO_ID_TOKEN is unset. Declare it under id_tokens: with aud: crates.io}"

# --retry without --retry-all-errors, so only transient failures are retried. A
# 400 means the token or the publisher registration is wrong, and retrying that
# just delays the error.
RESPONSE=$(curl --fail-with-body --silent --show-error \
    --retry 3 \
    -X POST 'https://crates.io/api/v1/trusted_publishing/tokens' \
    -H 'Content-Type: application/json' \
    -H 'User-Agent: quip-solver-core release pipeline (https://gitlab.com/quip.network/quip-solver-core)' \
    -d "{\"jwt\": \"${CRATES_IO_ID_TOKEN}\"}") || {
    echo "crates.io rejected the OIDC exchange. Check that a Trusted Publisher is registered for every crate." >&2
    echo "Response: ${RESPONSE:-<none>}" >&2
    exit 1
}

TOKEN=$(printf '%s' "$RESPONSE" | jq -r '.token // empty')
if [ -z "$TOKEN" ]; then
    echo "crates.io returned no token. Response: $RESPONSE" >&2
    exit 1
fi

echo "Minted a crates.io publish token, valid for 30 minutes." >&2
printf '%s' "$TOKEN"
