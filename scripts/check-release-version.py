#!/usr/bin/env python3
"""Assert that a release tag matches the version in every manifest.

The four artifacts are versioned in three separate files, and nothing made them
agree. A tag pushed against a tree whose manifests still carried the previous
version published that previous version again: crates.io rejects the duplicate,
but only after npm and PyPI have already accepted it, and a crates.io version
can never be replaced.

Run from the repository root:

    python3 scripts/check-release-version.py v0.0.0-rc6

The tag is compared without its leading `v`.
"""

import json
import sys
import tomllib
from pathlib import Path


def python_form(version: str) -> str:
    """Return the PEP 440 spelling of a Cargo/npm semver prerelease.

    Cargo and npm separate a prerelease with a hyphen (`0.0.0-rc6`). PEP 440
    does not allow one, so pyproject.toml carries `0.0.0rc6` for the same
    release. Dropping the hyphens is the whole difference for the `rcN` scheme
    this project uses; it is not a general PEP 440 normalizer.
    """
    return version.replace("-", "")


def main(argv: list[str]) -> int:
    if len(argv) != 2:
        print(f"usage: {argv[0]} <tag>", file=sys.stderr)
        return 2

    tag = argv[1]
    version = tag[1:] if tag.startswith("v") else tag
    root = Path(__file__).resolve().parent.parent

    cargo = tomllib.loads((root / "Cargo.toml").read_text())
    pyproject = tomllib.loads((root / "pyproject.toml").read_text())
    package_json = json.loads((root / "npm" / "package.json").read_text())

    expected = [
        ("Cargo.toml", cargo["workspace"]["package"]["version"], version),
        ("npm/package.json", package_json["version"], version),
        ("pyproject.toml", pyproject["project"]["version"], python_form(version)),
    ]

    mismatched = [
        (name, found, want) for name, found, want in expected if found != want
    ]

    for name, found, want in expected:
        status = "ok" if found == want else "MISMATCH"
        print(f"  {status:9} {name}: {found} (expected {want})")

    if mismatched:
        print(
            f"\nTag {tag} does not match every manifest. Bump the files listed"
            " above, or retag, before publishing.",
            file=sys.stderr,
        )
        return 1

    print(f"\nTag {tag} matches every manifest.")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
