#!/usr/bin/env python3
"""Writes a tiny silent PCM WAV seed without third-party media payloads."""

import sys
from pathlib import Path


def make_wav() -> bytes:
    samples = bytes([128] * 32)
    return (
        b"RIFF"
        + (36 + len(samples)).to_bytes(4, "little")
        + b"WAVEfmt "
        + (16).to_bytes(4, "little")
        + (1).to_bytes(2, "little")
        + (1).to_bytes(2, "little")
        + (8_000).to_bytes(4, "little")
        + (8_000).to_bytes(4, "little")
        + (1).to_bytes(2, "little")
        + (8).to_bytes(2, "little")
        + b"data"
        + len(samples).to_bytes(4, "little")
        + samples
    )


if len(sys.argv) != 2:
    raise SystemExit("usage: make_wav_seed.py OUTPUT")

output = Path(sys.argv[1])
output.parent.mkdir(parents=True, exist_ok=True)
output.write_bytes(make_wav())
