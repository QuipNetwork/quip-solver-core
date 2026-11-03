"""Command line: `python -m family_oracle write|check` or `topology encode|decode`.

Run from conformance/.
"""

import json
import sys
from pathlib import Path

from . import build, topology_blob

USAGE = (
    "usage: python -m family_oracle write|check\n"
    "       python -m family_oracle topology encode IN.json OUT.bin.gz\n"
    "       python -m family_oracle topology decode [--expand] IN.bin.gz [OUT.json]"
)


def _targets() -> dict:
    return {
        build.CORE_PATH: build.render(build.build_core()),
        build.LEGACY_PATH: build.render(build.build_legacy()),
        build.LAYERED_PATH: build.render(build.build_layered()),
        build.ICE_STRUCTURE_PATH: build.render(build.build_ice_structure()),
    }


def _topology(args: list[str]) -> int:
    expand = "--expand" in args
    args = [arg for arg in args if arg != "--expand"]
    try:
        if len(args) == 3 and args[0] == "encode" and not expand:
            body = json.loads(Path(args[1]).read_text())
            Path(args[2]).write_bytes(topology_blob.encode(body))
            return 0
        if len(args) in (2, 3) and args[0] == "decode":
            body = topology_blob.decode(Path(args[1]).read_bytes())
            if expand:
                body = topology_blob.expand(body)
            text = json.dumps(body, indent=2, sort_keys=True, ensure_ascii=True) + "\n"
            if len(args) == 3:
                Path(args[2]).write_text(text)
            else:
                sys.stdout.write(text)
            return 0
    except (topology_blob.BlobError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"topology: {error}", file=sys.stderr)
        return 1
    print(USAGE, file=sys.stderr)
    return 2


def main(argv: list[str]) -> int:
    if argv and argv[0] == "topology":
        return _topology(argv[1:])
    if len(argv) != 1 or argv[0] not in ("write", "check"):
        print(USAGE, file=sys.stderr)
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
