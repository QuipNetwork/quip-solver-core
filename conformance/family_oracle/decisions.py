"""Stable-code table and decision order for family admission (FAM-42 to FAM-65)."""

from .params import family_id_code

_ROWS = [
    (1, "DescriptorMissing", ["admission"], "MALFORMED", None),
    (2, "DescriptorUnexpected", ["admission"], "MALFORMED", None),
    (3, "FamilyIdInvalid", ["admission"], "MALFORMED", None),
    (4, "RevisionZero", ["admission"], "MALFORMED", None),
    (5, "FamilyUnsupported", ["admission"], "FAMILY_UNSUPPORTED", None),
    (6, "RevisionUnsupported", ["admission"], "FAMILY_UNSUPPORTED", None),
    (7, "ParamsMalformed", ["admission", "explicit-job"], "MALFORMED", "MALFORMED"),
    (
        8,
        "ParamsDigestMismatch",
        ["admission", "explicit-job"],
        "MALFORMED",
        "MALFORMED",
    ),
    (9, "ParamsOutOfBounds", ["admission", "explicit-job"], "MALFORMED", "MALFORMED"),
    (
        10,
        "ParamsRestricted",
        ["admission", "explicit-job"],
        "FAMILY_UNSUPPORTED",
        "FAMILY_UNSUPPORTED",
    ),
    (11, "TopologyOrderMismatch", ["admission"], "TOPOLOGY_MISMATCH", None),
    (12, "TopologyIncompatible", ["admission"], "FAMILY_UNSUPPORTED", None),
    (
        13,
        "BoundsExceeded",
        ["admission", "generation", "explicit-job"],
        "TOO_LARGE",
        "TOO_LARGE",
    ),
    (14, "NoInstance", ["generation"], None, None),
    (15, "InstanceDigestMismatch", ["explicit-job"], None, "MALFORMED"),
    (16, "MetadataInvalid", ["explicit-job"], None, "MALFORMED"),
]

CODES = [
    {
        "code": code,
        "name": name,
        "phases": phases,
        "wire_admission": admission,
        "wire_generation": None,
        "wire_explicit_job": explicit,
    }
    for code, name, phases, admission, explicit in _ROWS
]

_SESSION_WIRE = {
    "session:expired": "EXPIRED",
    "session:topology-missing": "TOPOLOGY_MISSING",
    "session:target-missing": "TARGET_MISSING",
    "session:topology-hash-mismatch": "TOPOLOGY_MISMATCH",
}

_FIELD_STEPS = [
    "field:generator-absent",
    "field:unknown-generator",
    "field:hash-length",
    "field:salt-count-zero",
    "field:salt-range-overflow",
]

LEASE_ADMISSION_ORDER = [
    "session:expired",
    "session:topology-missing",
    "session:target-missing",
    *_FIELD_STEPS,
    *[f"code:{n}" for n in range(1, 11)],
    "session:topology-hash-mismatch",
    "code:11",
    "code:12",
    "code:13",
]

EXPLICIT_JOB_ORDER = [
    "tier:not-listed",
    "code:7",
    "code:8",
    "code:9",
    "code:10",
    "code:13",
    "code:15",
    "code:16",
]

_PHASE_WIRE_KEY = {
    "admission": "wire_admission",
    "explicit-job": "wire_explicit_job",
}


def winner(order: list[str], faults: list[str]) -> str:
    """Return the first step of `order` that is in `faults`."""
    if not faults:
        raise ValueError("no faults")
    unknown = [f for f in faults if f not in order]
    if unknown:
        raise ValueError(f"unknown step: {unknown[0]}")
    return next(step for step in order if step in faults)


def step_outcome(step: str, phase: str) -> tuple[int | None, str | None]:
    """Return (code, wire reason) of a failing step in a phase."""
    if step in _SESSION_WIRE:
        return None, _SESSION_WIRE[step]
    if step.startswith("field:"):
        return None, "MALFORMED"
    if step == "tier:not-listed":
        return None, None
    if step.startswith("code:"):
        code = int(step.removeprefix("code:"))
        row = CODES[code - 1]
        return code, row[_PHASE_WIRE_KEY[phase]]
    raise ValueError(f"unknown step: {step}")


def _claims(entry: dict) -> bool:
    if family_id_code(entry["family_id"].encode(), "lease") is not None:
        return False
    revisions = entry["revisions"]
    if not revisions or 0 in revisions:
        return False
    return all(len(bytes.fromhex(d)) == 32 for d in entry["params_digests"])


def entry_claims(entries: list[dict]) -> list[bool]:
    """FAM-61: whether each entry claims support."""
    ids = [e["family_id"] for e in entries]
    return [_claims(e) and ids.count(e["family_id"]) == 1 for e in entries]


def lease_code(
    entries: list[dict], family_id: str, revision: int, params_digest: bytes
) -> int | None:
    """FAM-62: code 5, 6, 10, or None for a lease."""
    claiming = [
        e
        for e, ok in zip(entries, entry_claims(entries), strict=True)
        if ok and e["family_id"] == family_id
    ]
    if not claiming:
        return 5
    entry = claiming[0]
    if revision not in entry["revisions"]:
        return 6
    digests = entry["params_digests"]
    if digests and params_digest.hex() not in digests:
        return 10
    return None


def field_length_code(
    field: str, length: int, plain_node_count: int | None
) -> int | None:
    """FAM-46: the code for a wrong field length, else None."""
    if field == "FamilyDescriptor.params_digest":
        return None if length == 32 else 8
    if field == "IsingProblemGenerator.topology_order_digest":
        return None if length == 32 else 1
    if field == "FamilyBinding.nonce":
        return None if length in (0, 32) else 15
    if field in (
        "FamilyBinding.instance_digest",
        "FamilyBinding.topology_order_digest",
    ):
        return None if length == 32 else 15
    if field == "FamilyBinding.node_ids":
        return None if length == plain_node_count else 15
    raise ValueError(f"unknown field: {field}")
