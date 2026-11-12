"""Compact codec oracle adapter, supplied by the family task."""

from .codec_common import CodecError


def encode(instance: dict, spins: list[int]) -> bytes:
    raise CodecError("unsupported_family")


def decode(instance: dict, data: bytes) -> list[int]:
    raise CodecError("unsupported_family")


def encode_instance(params: bytes, instance: dict) -> bytes:
    raise CodecError("unsupported_family")


def decode_instance(params: bytes, data: bytes) -> dict:
    raise CodecError("unsupported_family")


def cases() -> list[dict]:
    return []
