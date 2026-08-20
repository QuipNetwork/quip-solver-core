#!/usr/bin/env bash
# Verifies that every License-File named in an sdist's PKG-INFO is present in
# the tarball.
#
# PyPI enforces this and rejects the upload with `400 License-File <name> does
# not exist in distribution file`. twine check does not, because the metadata
# itself is well formed, so the failure only appears at upload time, after the
# wheel has already been accepted.
#
# This happened for real: maturin declared LICENSE and NOTICE from the project
# config but packed neither, because both sit at the repository root rather
# than inside the crate.
set -euo pipefail

SDIST=${1:?usage: check-sdist-license-files.sh <sdist.tar.gz>}

if [ ! -f "$SDIST" ]; then
    echo "No such sdist: $SDIST" >&2
    exit 1
fi

# Read the listing once. Deriving the root with `tar | head` instead would kill
# tar with SIGPIPE, which pipefail then reports as a failure of this script.
CONTENTS=$(tar tzf "$SDIST")

# Every path in the tarball is prefixed with the distribution directory, and
# PKG-INFO names its license files relative to that directory.
ROOT=${CONTENTS%%/*}
if [ -z "$ROOT" ] || [ "$ROOT" = "$CONTENTS" ]; then
    echo "Could not read the root directory of $SDIST" >&2
    exit 1
fi
PKGINFO=$(tar xzOf "$SDIST" "$ROOT/PKG-INFO")

DECLARED=$(printf '%s\n' "$PKGINFO" | sed -n 's/^License-File: //p')
if [ -z "$DECLARED" ]; then
    echo "$SDIST declares no License-File. Nothing to check."
    exit 0
fi

MISSING=0
while IFS= read -r name; do
    [ -n "$name" ] || continue
    if printf '%s\n' "$CONTENTS" | grep -Fxq "$ROOT/$name"; then
        echo "  ok      $name"
    else
        echo "  MISSING $name" >&2
        MISSING=1
    fi
done <<EOF
$DECLARED
EOF

if [ "$MISSING" -ne 0 ]; then
    cat >&2 <<'MSG'

The sdist declares a License-File it does not contain. PyPI rejects this with
400 at upload time. Add the file under [tool.maturin] include with
format = "sdist".
MSG
    exit 1
fi

echo "$SDIST declares $(printf '%s\n' "$DECLARED" | grep -c .) license file(s), all present."
