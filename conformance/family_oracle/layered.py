"""The layered test construction (LAYERED), written from the plan's algorithm.

It is a test construction, not a registered family. Every operation is checked
against exact integers, and `generate` skips a salt the way FAM-26 and FAM-27
describe: code 14 for no instance, code 13 for a bound.
"""

import struct

from .chacha8 import ChaCha8, uniform
from .digests import (
    descriptor_digest,
    instance_digest,
    params_digest,
    stream_seed,
    topology_order_digest,
)
from .instance import canonical_bytes
from .params import UNIFORM_FIELD, decode_params

LABELS = (b"salt", b"couplings")
COEFFICIENT_BOUND = 2000
_CODE_BOUNDS = 13
_CODE_NO_INSTANCE = 14
_I32_MIN = -(2**31)
_I32_MAX = 2**31 - 1
_METADATA_KIND = 1


def check_topology(dense_edges: list[tuple[int, int]]) -> bool:
    """True when no dense edge is a self loop and no pair repeats."""
    seen = set()
    for a, b in dense_edges:
        pair = (min(a, b), max(a, b))
        if a == b or pair in seen:
            return False
        seen.add(pair)
    return True


def _layered_edges(
    n: int, n_layers: int, dense_edges: list[tuple[int, int]]
) -> list[tuple[int, int]]:
    edges = []
    for k in range(n_layers):
        edges += [(k * n + min(a, b), k * n + max(a, b)) for a, b in dense_edges]
    for k in range(n_layers - 1):
        edges += [(k * n + pos, (k + 1) * n + pos) for pos in range(n)]
    return sorted(edges)


def _metadata(n_layers: int, n: int) -> bytes:
    """One section of kind 1 holding the layer count and the base node count."""
    return struct.pack("<HHIHI", 1, _METADATA_KIND, 6, n_layers, n)


def generate(
    family_id: bytes,
    revision: int,
    params: bytes,
    nodes: list[int],
    native_edges: list[tuple[int, int]],
    nonce: bytes,
) -> tuple[int | None, dict | None]:
    """Return (skip_code, None) for code 13 or 14, else (None, instance)."""
    code, values = decode_params(UNIFORM_FIELD, params)
    if values is None:
        raise ValueError(f"params rejected with code {code}")
    n_layers = values["n_layers"]
    field_milli = values["field_milli"]
    n = len(nodes)
    position = {node: i for i, node in enumerate(nodes)}
    dense_edges = [(position[a], position[b]) for a, b in native_edges]
    if not check_topology(dense_edges):
        raise ValueError("topology is not simple")
    descriptor = descriptor_digest(family_id, revision, params_digest(params))
    order = topology_order_digest(nodes, native_edges)

    def stream(label: bytes) -> ChaCha8:
        return ChaCha8(stream_seed(nonce, descriptor, order, label))

    w = uniform(stream(LABELS[0]), 8)
    if w == 0:
        return _CODE_NO_INSTANCE, None
    if w == 2 and not _I32_MIN <= field_milli * 2**21 <= _I32_MAX:
        return _CODE_BOUNDS, None
    h_value = field_milli * (2 if w == 1 else 1)
    node_ids = list(range(n_layers * n))
    edges = _layered_edges(n, n_layers, dense_edges)
    h = [h_value] * len(node_ids)
    rng = stream(LABELS[1])
    j = [(uniform(rng, 3) - 1) * 1000 for _ in edges]
    offset_milli = 250 * n_layers
    if any(abs(value) > COEFFICIENT_BOUND for value in (*h, *j)):
        return _CODE_BOUNDS, None
    metadata = _metadata(n_layers, n)
    canonical = canonical_bytes(
        descriptor, order, nonce, node_ids, edges, h, j, offset_milli, metadata
    )
    return None, {
        "node_ids": node_ids,
        "edges": edges,
        "h": h,
        "j": j,
        "offset_milli": offset_milli,
        "metadata": metadata,
        "canonical": canonical,
        "digest": instance_digest(canonical),
        "energy_all_up_milli": offset_milli + sum(h) + sum(j),
    }
