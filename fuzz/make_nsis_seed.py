#!/usr/bin/env python3
"""Write a tiny synthetic PE overlay seed for the bounded NSIS signature sniffer."""

import struct
import sys
from pathlib import Path


def main() -> None:
    target = Path(sys.argv[1])
    pe_offset = 0x80
    optional_size = 0xE0
    section_table = pe_offset + 24 + optional_size
    raw_start = 0x200
    raw_size = 0x200
    overlay = raw_start + raw_size
    data = bytearray(overlay + 512)

    data[:2] = b"MZ"
    struct.pack_into("<I", data, 0x3C, pe_offset)
    data[pe_offset : pe_offset + 4] = b"PE\0\0"
    struct.pack_into("<HH", data, pe_offset + 4, 0x14C, 1)
    struct.pack_into("<H", data, pe_offset + 20, optional_size)
    struct.pack_into("<H", data, pe_offset + 24, 0x10B)
    struct.pack_into("<II", data, section_table + 16, raw_size, raw_start)

    struct.pack_into(
        "<7I",
        data,
        overlay,
        0,
        0xDEADBEEF,
        0x6C6C754E,
        0x74666F73,
        0x74736E49,
        1024,
        2048,
    )
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(data)


if __name__ == "__main__":
    main()
