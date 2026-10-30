"""Command line: `python -m family_oracle write` or `check`. Run from conformance/."""

import sys

from . import build


def _targets() -> dict:
    # Task 11 adds LEGACY_PATH with build_legacy.
    return {build.CORE_PATH: build.render(build.build_core())}


def main(argv: list[str]) -> int:
    if len(argv) != 1 or argv[0] not in ("write", "check"):
        print("usage: python -m family_oracle write|check", file=sys.stderr)
        return 2
    targets = _targets()
    if argv[0] == "write":
        for path, text in targets.items():
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_text(text)
            print(f"wrote {path}")
        return 0
    for path, text in targets.items():
        if not path.exists() or path.read_text() != text:
            print(f"differs: {path}", file=sys.stderr)
            return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
