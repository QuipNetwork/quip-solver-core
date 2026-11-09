"""Write the research template fixture from the archived research model.

Usage: python3 -I extract_two_commodity_template.py MODEL.npz OUT.json

Kernel indices of the arc spins map through `physical_nodes` to native
Zephyr ids. CI never runs this script. It reads only the fixture.
"""

import json
import sys

import numpy as np


def main(src: str, out: str) -> None:
    d = np.load(src, allow_pickle=False)
    phys = d["physical_nodes"]
    doc = {
        "about": "The template of the archived research model on the live z(12, 4) "
        "topology. Arc spins are native Zephyr ids. CI reads only this file.",
        "source": {
            "command": "python3 -I conformance/fixtures/extract_two_commodity_template.py "
            "<models>/" + src.rsplit("/", 1)[1] + " "
            "conformance/fixtures/two_commodity_research_template.json",
            "npz": "quip-data/regimes/new-pow/families/two-commodity-routing/models/"
            + src.rsplit("/", 1)[1],
            "topology_sha256": str(d["topology_sha256"]),
        },
        "nodes_cell": d["nodes_cell"].tolist(),
        "arc_nodes": d["arc_nodes"].tolist(),
        "arc_spin_commodity1": [int(phys[i]) for i in d["arc_spin_commodity1"]],
        "arc_chain_commodity2": [
            [int(phys[i]) for i in row] for row in d["arc_chain_commodity2"]
        ],
    }
    lines = [f" {json.dumps(k)}: {json.dumps(v)}" for k, v in sorted(doc.items())]
    with open(out, "w") as f:
        f.write("{\n" + ",\n".join(lines) + "\n}\n")


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
