"""Canonical JSON used to hash vector cases."""

import json


def _canonical(value: object) -> str:
    if isinstance(value, float):
        raise TypeError(f"float in canonical JSON: {value!r}")
    if isinstance(value, str) and not value.isascii():
        raise TypeError(f"non-ASCII string in canonical JSON: {value!r}")
    if isinstance(value, dict):
        for key in value:
            _canonical(key)
        for item in value.values():
            _canonical(item)
    elif isinstance(value, list):
        for item in value:
            _canonical(item)
    return ""


def canonical_json(value: object) -> str:
    """Keys sorted, no whitespace, integers and ASCII strings only."""
    _canonical(value)
    return json.dumps(value, sort_keys=True, separators=(",", ":"), ensure_ascii=True)
