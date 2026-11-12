"""Exact revision dispatch to the independent family codecs."""

import importlib

from .codec_common import CodecError

FAMILIES = {
    "quip/polariton-static": "polariton",
    "quip/defects-static": "defects",
    "quip/flow-static": "flow",
    "quip/twisted-ice-dimer": "twisted",
    "quip/charged-ice": "charge",
    "quip/ice-glass": "glass",
    "quip/ice-fields": "fields",
    "quip/two-commodity-routing": "routing",
}


def module(family_id: str, revision: int):
    if revision != 1 or family_id not in FAMILIES:
        raise CodecError("unsupported_family")
    return importlib.import_module(f".codec_{FAMILIES[family_id]}", __package__)


def encode(family_id: str, revision: int, instance: dict, spins: list[int]) -> bytes:
    return module(family_id, revision).encode(instance, spins)


def decode(family_id: str, revision: int, instance: dict, data: bytes) -> list[int]:
    return module(family_id, revision).decode(instance, data)


def encode_instance(
    family_id: str, revision: int, params: bytes, instance: dict
) -> bytes:
    return module(family_id, revision).encode_instance(params, instance)


def decode_instance(family_id: str, revision: int, params: bytes, data: bytes) -> dict:
    codec = module(family_id, revision)
    instance = codec.decode_instance(params, data)
    if FAMILIES[family_id] in ("defects", "flow"):
        instance["params"] = codec._params(params)
    return instance
