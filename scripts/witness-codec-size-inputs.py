"""Build deterministic admitted-size inputs with independent family witnesses."""

import importlib
import json
import sys
from itertools import combinations
from pathlib import Path

from family_oracle import build, codecs, ice


def vectors(slug, instance):
    codec = importlib.import_module(f"family_oracle.codec_{slug}")
    n = len(instance["node_ids"])
    if slug in ("polariton", "defects", "flow"):
        spins = [0] * n
        for edge, p in enumerate(instance["positions"]):
            spins[p] = instance["orientation"][edge]
        return [("hard", spins)]
    blocks = []
    if slug == "twisted":
        blocks = codec._blocks(instance)[0]
    elif slug == "charge":
        blocks = [
            (p, g, codec.MASKS[q], 3 if q == 0 else 2)
            for p, g, q in codec._blocks(instance)[0]
        ]
    elif slug in ("glass", "fields"):
        tiles, gauges, _ = codec._selected(instance)
        blocks = [(p, [1 - 2 * gauges[i] for i in p], [3], 3) for p in tiles]
    elif slug == "routing":
        blocks = [(a[3:5], [1, 1], [0], 1) for a in codec._metadata(instance)[1]]
    spins = [1] * n
    for positions, gauges, masks, _ in blocks:
        for i, p in enumerate(positions):
            spins[p] = gauges[i] * (1 if masks[0] & (1 << i) else -1)
    out = [("structured", spins)]
    if blocks:
        broken = spins.copy()
        if len(blocks[0][0]) == 2:
            broken[blocks[0][0][0]] *= -1
        else:
            # All-positive gauged spins violate every admitted tile charge.
            for p, g in zip(blocks[0][0], blocks[0][1], strict=True):
                broken[p] = g
        out.append(("fallback", broken))
    return out


def small_inputs():
    from codec_cases import cases

    schemas = dict(
        polariton=build.POLARITON,
        defects=build.DEFECTS,
        flow=build.FLOW,
        twisted=build.TWISTED_DIMER,
        charge=build.CHARGED_ICE,
        glass=build.ICE_GLASS,
        fields=build.ICE_FIELDS,
        routing=build.TWO_COMMODITY,
    )
    seen = set()
    for case in cases():
        slug = case["slug"]
        if slug in seen:
            continue
        seen.add(slug)
        _, values = build.decode_params(schemas[slug], case["params"])
        assert values is not None, case["name"]
        instance = codecs.decode_instance(
            case["family"], 1, case["params"], case["compact"]
        )
        if values["zephyr_m"]:
            nodes, edges = ice.lattice(values["zephyr_m"], values["zephyr_t"])
        else:
            nodes = instance["node_ids"]
            edges = [(nodes[u], nodes[v]) for u, v in instance["edges"]]
        nonce = case["canonical"][65:97]
        generator = importlib.import_module(f"family_oracle.{slug}")
        generated = generator.generate(
            case["family"].encode(), 1, case["params"], nodes, edges, nonce
        )
        generated = generated if isinstance(generated, dict) else generated[1]
        assert generated is not None and generated["canonical"] == case["canonical"], (
            case["name"]
        )
        yield dict(
            family_id=case["family"],
            revision=1,
            case="golden-" + case["name"],
            params_hex=case["params"].hex(),
            nonce_hex=nonce.hex(),
            topology_nodes=nodes,
            topology_edges=edges,
            expected_status="measured",
            expected_code=None,
            canonical_hex=case["canonical"].hex(),
            witnesses=[
                dict(mode=mode, spins=spins) for mode, spins in vectors(slug, generated)
            ],
        )


def build_inputs() -> list[dict]:
    rows = []
    live = build.live_topology()
    live_top = (live["nodes"], live["edges"])

    def add(slug, name, params, topology, fixed_nonce=None):
        family = next(k for k, v in codecs.FAMILIES.items() if v == slug)
        generator = importlib.import_module(
            f"family_oracle.{'glass' if slug == 'glass' else slug}"
        )
        nodes, edges = topology
        admission = getattr(generator, "admission_code", lambda *args: None)(
            params, nodes, edges
        )
        for seed in range(4):
            nonce = fixed_nonce if fixed_nonce is not None else bytes([seed]) * 32
            row = dict(
                family_id=family,
                revision=1,
                case=f"{name}-nonce{seed}",
                params_hex=params.hex(),
                nonce_hex=nonce.hex(),
                topology_nodes=nodes,
                topology_edges=edges,
            )
            if admission:
                rows.append(
                    row
                    | dict(
                        expected_code=admission,
                        expected_status="rejected",
                        witnesses=[],
                    )
                )
                break
            result = generator.generate(family.encode(), 1, params, nodes, edges, nonce)
            code, instance = (0, result) if isinstance(result, dict) else result
            if instance is None:
                rows.append(
                    row
                    | dict(expected_code=code, expected_status="skipped", witnesses=[])
                )
                continue
            rows.append(
                row
                | dict(
                    expected_status="measured",
                    expected_code=None,
                    canonical_hex=instance["canonical"].hex(),
                    witnesses=[
                        dict(mode=mode, spins=spins)
                        for mode, spins in vectors(slug, instance)
                    ],
                )
            )
            break

    for slug in ("polariton", "flow"):
        for side in (2, 8, 62, 64):
            m = max(2, (side + 3) // 2)
            m = min(m, 32)
            params = getattr(build, f"_{slug}_params")(m, 4, side, 0)
            add(slug, f"side-{side}", params, ice.lattice(m, 4))
    for side in (2, 3, 4):
        for obstacles, pins in ((0, 0), (0, 2), (1, 2)):
            add(
                "defects",
                f"side-{side}-obstacles-{obstacles}-pins-{pins}",
                build._defects_params(3, 4, side, 0, obstacles, pins, 300, -200),
                ice.lattice(3, 4),
            )
    for n in (16, 4096, 131072):
        nodes = list(range(n))
        edges = [
            pair
            for start in range(0, n, 4)
            for pair in combinations(range(start, start + 4), 2)
        ]
        add("glass", f"k4-{n}", build._ice_glass_params(0, 0, 125), (nodes, edges))
        for fraction in (0, 256):
            add(
                "charge",
                f"k4-{n}-charge-{fraction}",
                build._charge_params(0, 0, 250, 31, fraction),
                (nodes, edges),
            )
        if n != 131072:
            joined = edges + [(i, i + 4) for i in range(0, n - 4, 8)]
            for pairs in (0, n // 8):
                add(
                    "twisted",
                    f"k4-{n}-pairs-{pairs}",
                    build._twisted_params(0, 0, 62, pairs),
                    (nodes, joined),
                )
        motif_nodes, motif_edges = build._three_k4()
        shared = [
            (u + start, v + start)
            for start in range(0, n - len(motif_nodes) + 1, len(motif_nodes))
            for u, v in motif_edges
        ]
        add(
            "fields",
            f"overlap-{n}",
            build._fields_params(0, 0, 125, 500, 65535 if n == 131072 else 16),
            (nodes, sorted(shared)),
        )
    for m, t, top in [
        (2, 4, ice.lattice(2, 4)),
        (12, 4, live_top),
        (32, 3, ice.lattice(32, 3)),
    ]:
        add("routing", f"zephyr-{m}-{t}", build._routing_params(m, t, 32), top)
    for slug, filename, section in [
        ("polariton", "polariton", "polariton_recipes"),
        ("flow", "flow", "flow_recipes"),
        ("twisted", "twisted_dimer", "recipes"),
        ("charge", "ice_charge", "recipe"),
        ("fields", "ice_fields", "field_recipes"),
        ("routing", "two_commodity", "routing_recipes"),
    ]:
        document = json.loads((build.FAMILIES_DIR / f"{filename}.json").read_text())
        for recipe in document[section]:
            salts = recipe["salts"] or [{"nonce": bytes(32).hex()}]
            for index, salt in enumerate(salts):
                add(
                    slug,
                    f"{recipe['name']}-salt{index}",
                    bytes.fromhex(recipe["params_hex"]),
                    live_top,
                    bytes.fromhex(salt["nonce"]),
                )
    # A lower field count realizes the requested 4096-node size after the
    # divisor-16 attempts above record their construction shortfalls.
    _, motif_edges = build._three_k4()
    shared = [
        (u + start, v + start)
        for start in range(0, 4096 - 8, 9)
        for u, v in motif_edges
    ]
    add(
        "fields",
        "overlap-4096-divisor32",
        build._fields_params(0, 0, 125, 500, 32),
        (list(range(4096)), shared),
    )
    rows.extend(small_inputs())
    return rows


if __name__ == "__main__":
    Path(sys.argv[1]).write_text(json.dumps(build_inputs(), sort_keys=True) + "\n")
