"""Independent cycle witness codec and structural flow instance codec."""

import struct
from collections import deque

from . import flow
from .codec_common import (
    CodecError,
    check_bit_array,
    decode_prefix,
    encode_column,
    encode_prefix,
    preflight_prefix,
    read_column,
)
from .digests import descriptor_digest, instance_digest, params_digest
from .instance import canonical_bytes
from .params import FLOW, decode_params
from .polariton import Lattice

LIMITS = {
    "nodes": flow.MAX_NODES,
    "edges": flow.MAX_EDGES,
    "coefficient": flow.MAX_COEFFICIENT,
    "offset": flow.MAX_OFFSET,
    "metadata": flow.MAX_METADATA,
}


def _pack(values):
    out = bytearray((len(values) + 7) // 8)
    for i, value in enumerate(values):
        out[i // 8] |= (value == 1) << (i % 8)
    return bytes(out)


def _unpack(data, count):
    check_bit_array(data, count)
    return [1 if data[i // 8] & (1 << (i % 8)) else -1 for i in range(count)]


def _params(params):
    _, values = decode_params(FLOW, params)
    if values is None:
        raise CodecError("invalid_instance")
    return values


def _origin(values, window):
    if min(window) < 1 or max(window) + values["side"] > 2 * values["zephyr_m"] - 1:
        raise CodecError("invalid_instance")


def _metadata(instance, values):
    side = values["side"]
    n, vertices = 2 * side * (side + 1), (side + 1) ** 2
    data, at, parts = instance["metadata"], 2, []
    if len(data) < 2 or struct.unpack_from("<H", data)[0] != 3:
        raise CodecError("invalid_instance")
    for kind, length in ((4, 2 + vertices), (5, 4 + 4 * n), (9, 8 + 5 * n)):
        if at + 6 + length > len(data) or struct.unpack_from("<HI", data, at) != (
            kind,
            length,
        ):
            raise CodecError("invalid_instance")
        parts.append(data[at + 6 : at + 6 + length])
        at += 6 + length
    if at != len(data) or len(instance["node_ids"]) != n:
        raise CodecError("invalid_instance")
    charges, incidence, costs = parts
    if struct.unpack_from("<H", charges)[0] != side:
        raise CodecError("invalid_instance")
    lattice = Lattice(side)
    labels = list(struct.unpack(f"<{vertices}b", charges[2:]))
    boundary = set(flow.edge_vertices(lattice))
    if any(q not in (-1, 1) if v in boundary else q != 0 for v, q in enumerate(labels)):
        raise CodecError("invalid_instance")
    window = struct.unpack_from("<HH", incidence)
    _origin(values, window)
    positions = list(struct.unpack(f"<{n}I", incidence[4:]))
    if set(positions) != set(range(n)):
        raise CodecError("invalid_instance")
    weight = struct.unpack_from("<Q", costs)[0]
    targets, amounts = [], []
    for i in range(n):
        target, cost = struct.unpack_from("<bI", costs, 8 + 5 * i)
        if target not in (-1, 1):
            raise CodecError("invalid_instance")
        targets.append(target)
        amounts.append(cost)
    return lattice, labels, window, positions, weight, targets, amounts


def _forest(instance):
    parsed = flow.parse_metadata(instance)
    if parsed is None:
        raise CodecError("invalid_instance")
    lattice = Lattice(instance["params"]["side"])
    visited, queue, tree, order = {0}, deque([0]), set(), []
    while queue:
        v = queue.popleft()
        for edge, incidence in lattice.incident[v]:
            tail, head, _ = lattice.links[edge]
            other = head if v == tail else tail
            if other not in visited:
                visited.add(other)
                queue.append(other)
                tree.add(edge)
                order.append((other, edge, -incidence))
    independent = [e for e in range(len(lattice.links)) if e not in tree]
    return lattice, parsed, order, independent


def encode(instance: dict, spins: list[int]) -> bytes:
    if len(spins) != len(instance["node_ids"]):
        raise CodecError("length")
    if any(s not in (-1, 1) for s in spins):
        raise CodecError("spin_value")
    lattice, parsed, _, independent = _forest(instance)
    links = [spins[p] for p in parsed["positions"]]
    if lattice.charges(links) != parsed["labels"]:
        raise CodecError("infeasible")
    return _pack([links[e] for e in independent])


def decode(instance: dict, data: bytes) -> list[int]:
    lattice, parsed, order, independent = _forest(instance)
    free = _unpack(data, len(independent))
    links = [0] * len(lattice.links)
    residual = list(parsed["labels"])
    for edge, spin in zip(independent, free, strict=True):
        links[edge] = spin
        tail, head, _ = lattice.links[edge]
        residual[tail] += spin
        residual[head] -= spin
    for vertex, edge, incidence in reversed(order):
        spin = residual[vertex] * incidence
        if spin not in (-1, 1):
            raise CodecError("infeasible")
        links[edge] = spin
        tail, head, _ = lattice.links[edge]
        residual[tail] += spin
        residual[head] -= spin
    if any(residual):
        raise CodecError("infeasible")
    spins = [0] * len(links)
    for edge, position in enumerate(parsed["positions"]):
        spins[position] = links[edge]
    return spins


def encode_instance(params: bytes, instance: dict) -> bytes:
    values = _params(params)
    prefix = encode_prefix(instance)
    preflight = preflight_prefix(prefix, LIMITS)
    expected = descriptor_digest(flow.FAMILY_ID.encode(), 1, params_digest(params))
    if preflight.descriptor_digest != expected:
        raise CodecError("descriptor_mismatch")
    try:
        lattice, labels, window, positions, weight, targets, costs = _metadata(
            instance, values
        )
    except CodecError as error:
        raise CodecError("unencodable_instance") from error
    if len(instance["metadata"]) > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    return (
        prefix
        + struct.pack("<HH", *window)
        + _pack([labels[v] for v in flow.edge_vertices(lattice)])
        + encode_column(positions)
        + struct.pack("<Q", weight)
        + _pack(targets)
        + encode_column(costs)
    )


def decode_instance(params: bytes, data: bytes) -> dict:
    prefix = preflight_prefix(data, LIMITS)
    values = _params(params)
    side = values["side"]
    n, vertices = 2 * side * (side + 1), (side + 1) ** 2
    if 34 + vertices + 9 * n > LIMITS["metadata"]:
        raise CodecError("bounds_exceeded")
    view = memoryview(data)[prefix.metadata_start :]
    bits = 4 * (side - 1)
    end = 4 + (bits + 7) // 8
    if len(view) < end:
        raise CodecError("length")
    window = struct.unpack_from("<HH", view)
    signs = view[4:end]
    check_bit_array(signs, bits)
    positions, used = read_column(view[end:], n)
    at = end + used
    if at + 8 + (n + 7) // 8 > len(view):
        raise CodecError("length")
    weight = struct.unpack_from("<Q", view, at)[0]
    at += 8
    end = at + (n + 7) // 8
    target_bits = view[at:end]
    check_bit_array(target_bits, n)
    costs, used = read_column(view[end:], n)
    if end + used != len(view):
        raise CodecError("length")
    expected = descriptor_digest(flow.FAMILY_ID.encode(), 1, params_digest(params))
    if prefix.descriptor_digest != expected:
        raise CodecError("descriptor_mismatch")
    if prefix.node_count != n:
        raise CodecError("invalid_instance")
    _origin(values, window)
    # A fixed bitmap checks the permutation without a count-sized reservation.
    seen = bytearray(1040)
    for i in range(n):
        p = positions.get(i)
        if p >= n or seen[p // 8] & (1 << (p % 8)):
            raise CodecError("invalid_instance")
        seen[p // 8] |= 1 << (p % 8)
    lattice = Lattice(side)
    labels = [0] * vertices
    for vertex, sign in zip(
        flow.edge_vertices(lattice), _unpack(signs, bits), strict=True
    ):
        labels[vertex] = sign
    metadata = flow.metadata(
        side,
        labels,
        window,
        [positions.get(i) for i in range(n)],
        weight,
        _unpack(target_bits, n),
        [costs.get(i) for i in range(n)],
    )
    instance = decode_prefix(prefix, metadata, LIMITS, (4, 5, 9))
    positions.minimal()
    costs.minimal()
    return instance


def fixtures():
    """Generate small independent instances, including reversed registration order."""
    from .build import _flow_params, _flow_rows

    for name, params, salts, (nodes, edges), _ in _flow_rows():
        for seed in range(min(salts, 2)):
            code, instance = flow.generate(
                flow.FAMILY_ID.encode(),
                1,
                params,
                nodes,
                edges,
                seed.to_bytes(32, "little"),
            )
            if code == 0:
                yield f"{name}-{seed}", params, instance
    from .ice import lattice as zephyr_lattice

    nodes, edges = zephyr_lattice(6, 4)
    for side, half in ((2, 0), (8, 5)):
        params = _flow_params(6, 4, side, half)
        code, instance = flow.generate(
            flow.FAMILY_ID.encode(), 1, params, nodes, edges, bytes(32)
        )
        if code == 0:
            yield f"flow-l{side}-half{half}", params, instance


def cases() -> list[dict]:
    rows = []
    for name, params, instance in fixtures():
        compact = encode_instance(params, instance)
        rows.append(
            {
                "id": name,
                "kind": "instance",
                "params": params.hex(),
                "canonical": instance["canonical"].hex(),
                "compact": compact.hex(),
                "digest": instance["digest"].hex(),
            }
        )
        spins = [0] * len(instance["positions"])
        for edge, p in enumerate(instance["positions"]):
            spins[p] = instance["orientation"][edge]
        witness = encode(instance, spins)
        rows.append(
            {
                "id": name + "-witness",
                "kind": "witness",
                "instance": name,
                "spins": spins,
                "compact": witness.hex(),
            }
        )
        for suffix, malformed in (
            ("short", witness[:-1]),
            ("extra", witness + b"\x00"),
        ):
            rows.append(
                {
                    "id": name + "-" + suffix,
                    "kind": "witness_error",
                    "instance": name,
                    "compact": malformed.hex(),
                    "error": "length",
                }
            )
        prefix = preflight_prefix(compact, LIMITS)
        malformed_instances = [("extra", compact + b"\x00", "length")]
        for suffix, at, value, error in (
            ("nodes", 96, 8321, "bounds_exceeded"),
            ("edges", 100, 24575, "bounds_exceeded"),
        ):
            changed = bytearray(compact)
            struct.pack_into("<I", changed, at, value)
            malformed_instances.append((suffix, bytes(changed), error))
        for suffix, at, value, error in (
            ("descriptor", 0, compact[0] ^ 1, "descriptor_mismatch"),
            ("width", 112, 33, "invalid_symbol"),
        ):
            changed = bytearray(compact)
            changed[at] = value
            malformed_instances.append((suffix, bytes(changed), error))
        n, e = len(instance["node_ids"]), len(instance["edges"])
        _, ids_len = read_column(compact[112:], n)
        _, endpoints_len = read_column(compact[112 + ids_len :], 2 * e)
        h_start = 112 + ids_len + endpoints_len
        palette_count = struct.unpack_from("<I", compact, h_start)[0]
        _, ranks_len = read_column(compact[h_start + 4 + 4 * palette_count :], n)
        h_end = h_start + 4 + 4 * palette_count + ranks_len
        unused = struct.pack("<Iii", 2, -1, 0) + encode_column([0] * n)
        malformed_instances.append(
            (
                "unused-palette",
                compact[:h_start] + unused + compact[h_end:],
                "non_canonical",
            )
        )
        changed = bytearray(compact)
        struct.pack_into("<I", changed, h_start, n + 1)
        malformed_instances.append(("palette-count", bytes(changed), "bounds_exceeded"))
        if not any(instance["costs"]):
            nonminimal = compact[:-1] + b"\x01" + bytes((n + 7) // 8)
            malformed_instances.append(("cost-width", nonminimal, "non_canonical"))
            endpoints = [prefix.endpoints.get(i) for i in range(2 * e)]
            endpoints[0] = endpoints[1] = 0
            start = 112 + ids_len
            invalid_edges = (
                nonminimal[:start]
                + encode_column(endpoints)
                + nonminimal[start + endpoints_len :]
            )
            malformed_instances.append(
                ("edge-before-width", invalid_edges, "invalid_instance")
            )
        if instance["side"] == 2:
            for candidate in range(16):
                try:
                    decode(instance, bytes([candidate]))
                except CodecError as error:
                    if error.name == "infeasible":
                        rows.append(
                            {
                                "id": name + "-infeasible",
                                "kind": "witness_error",
                                "instance": name,
                                "compact": bytes([candidate]).hex(),
                                "error": "infeasible",
                            }
                        )
                        break
        boundary_bits = 4 * (instance["side"] - 1)
        if boundary_bits % 8:
            changed = bytearray(compact)
            at = prefix.metadata_start + 4 + (boundary_bits + 7) // 8 - 1
            changed[at] |= 0x80
            malformed_instances.append(("boundary-padding", bytes(changed), "padding"))
        for suffix, payload, error in malformed_instances:
            rows.append(
                {
                    "id": name + "-" + suffix + "-instance",
                    "kind": "instance_error",
                    "params": params.hex(),
                    "compact": payload.hex(),
                    "error": error,
                }
            )

    _, params, generated = next(fixtures())
    wider = dict(generated)
    wider["h"] = list(generated["h"])
    wider["h"][0] += 7
    metadata = bytearray(generated["metadata"])
    flow_start = len(metadata) - 8 - 5 * len(wider["h"])
    struct.pack_into("<Q", metadata, flow_start, 2**64 - 1)
    struct.pack_into("<I", metadata, flow_start + 9, 2**32 - 1)
    for vertex in flow.edge_vertices(Lattice(generated["side"])):
        metadata[10 + vertex] = 1
    wider["metadata"] = bytes(metadata)
    head = generated["canonical"]
    canonical = canonical_bytes(
        head[1:33],
        head[33:65],
        head[65:97],
        wider["node_ids"],
        wider["edges"],
        wider["h"],
        wider["j"],
        wider["offset_milli"],
        wider["metadata"],
    )
    rows.append(
        {
            "id": "flow-structural-nongenerated",
            "kind": "instance",
            "params": params.hex(),
            "canonical": canonical.hex(),
            "compact": encode_instance(params, wider).hex(),
            "digest": instance_digest(canonical).hex(),
        }
    )
    return rows
