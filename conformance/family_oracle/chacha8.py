"""ChaCha8 word stream, bounded uniform draw, and shuffle (FAM-21, 23, 24)."""

from typing import Any, Protocol

_MASK = 0xFFFFFFFF
_CONSTANTS = (0x61707865, 0x3320646E, 0x79622D32, 0x6B206574)
_DOUBLE_ROUNDS = 4


class WordSource(Protocol):
    """Anything that yields 32-bit words and counts them."""

    words_used: int

    def next_u32(self) -> int: ...


def _rotl(value: int, bits: int) -> int:
    return ((value << bits) | (value >> (32 - bits))) & _MASK


def _quarter_round(s: list[int], a: int, b: int, c: int, d: int) -> None:
    s[a] = (s[a] + s[b]) & _MASK
    s[d] = _rotl(s[d] ^ s[a], 16)
    s[c] = (s[c] + s[d]) & _MASK
    s[b] = _rotl(s[b] ^ s[c], 12)
    s[a] = (s[a] + s[b]) & _MASK
    s[d] = _rotl(s[d] ^ s[a], 8)
    s[c] = (s[c] + s[d]) & _MASK
    s[b] = _rotl(s[b] ^ s[c], 7)


class ChaCha8:
    """ChaCha with 8 rounds, a 64-bit block counter, and a zero nonce."""

    def __init__(self, seed: bytes) -> None:
        if len(seed) != 32:
            raise ValueError("seed must be 32 bytes")
        self._key = [int.from_bytes(seed[i : i + 4], "little") for i in range(0, 32, 4)]
        self._counter = 0
        self._block: list[int] = []
        self._pos = 0
        self.words_used = 0

    def _refill(self) -> None:
        init = [
            *_CONSTANTS,
            *self._key,
            self._counter & _MASK,
            (self._counter >> 32) & _MASK,
            0,
            0,
        ]
        state = list(init)
        for _ in range(_DOUBLE_ROUNDS):
            _quarter_round(state, 0, 4, 8, 12)
            _quarter_round(state, 1, 5, 9, 13)
            _quarter_round(state, 2, 6, 10, 14)
            _quarter_round(state, 3, 7, 11, 15)
            _quarter_round(state, 0, 5, 10, 15)
            _quarter_round(state, 1, 6, 11, 12)
            _quarter_round(state, 2, 7, 8, 13)
            _quarter_round(state, 3, 4, 9, 14)
        self._block = [(w + i) & _MASK for w, i in zip(state, init, strict=True)]
        self._counter = (self._counter + 1) & 0xFFFFFFFFFFFFFFFF
        self._pos = 0

    def next_u32(self) -> int:
        if self._pos >= len(self._block):
            self._refill()
        word = self._block[self._pos]
        self._pos += 1
        self.words_used += 1
        return word


def uniform(rng: WordSource, n: int) -> int:
    """Draw from 0..n-1 by rejection sampling over 32-bit words."""
    if not 1 <= n <= 2**32 - 1:
        raise ValueError(f"n out of range: {n}")
    limit = 2**32 - (2**32 % n)
    while True:
        x = rng.next_u32()
        if x < limit:
            return x % n


def shuffle(rng: WordSource, items: list[Any]) -> None:
    """Fisher-Yates shuffle from the end of the list."""
    for i in range(len(items) - 1, 0, -1):
        j = uniform(rng, i + 1)
        items[i], items[j] = items[j], items[i]
