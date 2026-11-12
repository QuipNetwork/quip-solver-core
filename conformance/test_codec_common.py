"""Independent compact prefix and integer-column checks."""

import struct

import pytest

from family_oracle.codec_common import (
    CodecError,
    encode_column,
    decode_column,
    encode_palette,
    decode_palette,
    encode_prefix,
    preflight_prefix,
    decode_prefix,
)


@pytest.mark.parametrize(
    "values,width",
    [([], 0), ([0, 0], 0), ([0, 1], 1), ([0, 1 << 30], 31), ([0, 2**32 - 1], 32)],
)
def test_columns(values, width):
    encoded = encode_column(values)
    assert encoded[0] == width
    assert decode_column(encoded, len(values)) == (values, len(encoded))


@pytest.mark.parametrize(
    "data,count,error",
    [
        (b"\x01\x00", 1, "non_canonical"),
        (b"\x01\x85", 3, "padding"),
        (b"\x21", 0, "invalid_symbol"),
        (b"\x20", 2, "length"),
    ],
)
def test_bad_columns(data, count, error):
    with pytest.raises(CodecError) as caught:
        decode_column(data, count)
    assert caught.value.name == error


def test_palettes():
    for values in [[], [0], [0, 0], [-(2**31), 2**31 - 1, 0, -(2**31)]]:
        data = encode_palette(values)
        assert decode_palette(data, len(values), 2**31) == (values, len(data))
    with pytest.raises(CodecError, match="non_canonical"):
        decode_palette(struct.pack("<IiiB", 2, 0, 1, 0), 2, 10)
    with pytest.raises(CodecError, match="non_canonical"):
        decode_palette(struct.pack("<IiBB", 1, 0, 1, 0), 2, 10)
    with pytest.raises(CodecError, match="length"):
        decode_palette(struct.pack("<Ii", 2, 0), 2, 10)


def test_prefix_roundtrip():
    instance = {
        "descriptor_digest": bytes([1]) * 32,
        "topology_order_digest": bytes([2]) * 32,
        "nonce": bytes(32),
        "node_ids": [90, 3, 70, 0],
        "edges": [(0, 1), (1, 3)],
        "h": [0, 2**30, 0, -(2**30)],
        "j": [-7, -7],
        "offset_milli": -(2**56),
        "metadata": b"\0\0",
    }
    data = encode_prefix(instance)
    prefix = preflight_prefix(data)
    decoded = decode_prefix(prefix, b"\0\0")
    assert encode_prefix(decoded) == data
    assert decoded["node_ids"] == instance["node_ids"]
    assert decoded["nonce"] == bytes(32)
    hostile = bytearray(data)
    struct.pack_into("<I", hostile, 96, 2**32 - 1)
    with pytest.raises(CodecError, match="bounds_exceeded"):
        preflight_prefix(bytes(hostile))
    with pytest.raises(CodecError, match="bounds_exceeded"):
        decode_prefix(prefix, b"\0\0\0", {"metadata": 2})


def test_wire_order_bounds_and_empty_palettes():
    hostile = bytearray(100)
    struct.pack_into("<I", hostile, 96, 2**32 - 1)
    with pytest.raises(CodecError, match="bounds_exceeded"):
        preflight_prefix(bytes(hostile))
    hostile = bytearray(124)
    struct.pack_into("<I", hostile, 96, 5)
    with pytest.raises(CodecError, match="bounds_exceeded"):
        preflight_prefix(bytes(hostile), {"nodes": 4})
    with pytest.raises(CodecError, match="invalid_symbol"):
        decode_palette(bytes(5), 1)
    assert decode_palette(bytes(5), 0) == ([], 5)
    assert encode_column([90, 3, 70, 0]).hex() == "07da811100"
    assert (
        encode_palette([0, 2**30, 0, -(2**30)]).hex()
        == "03000000000000c000000000000000400219"
    )
