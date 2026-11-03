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
    targets = build.targets()
    if argv[0] == "write":
        for path, data in targets.items():
            if build.file_matches(path, data):
                continue
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(data)
            print(f"wrote {path}")
        return 0
    stale = [
        path
        for path in build.TOPOLOGIES_DIR.glob("*.bin.gz")
        if path not in targets and path != build.LIVE_BLOB_PATH
    ]
    differs = [
        path for path, data in targets.items() if not build.file_matches(path, data)
    ]
    for path in [*differs, *stale]:
        print(f"differs: {path}", file=sys.stderr)
    return 1 if differs or stale else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
