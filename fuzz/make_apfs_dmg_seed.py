#!/usr/bin/env python3
"""Wrap the independently created APFS partition slice in a raw UDIF/GPT."""

import base64
import pathlib
import struct
import sys
import zlib

source = pathlib.Path(__file__).resolve().parent.parent / "crates/rz-formats/tests/fixtures/apfs_compressed.bin"
partition = source.read_bytes()
assert len(partition) % 512 == 0
start = 40
sectors = start + len(partition) // 512
image = bytearray(sectors * 512)
image[start * 512 :] = partition

table = bytearray(128 * 128)
table[:16] = bytes.fromhex("ef57347c0000aa11aa1100306543ecac")
struct.pack_into("<QQ", table, 32, start, sectors - 1)
image[2 * 512 : 2 * 512 + len(table)] = table
header = bytearray(512)
header[:8] = b"EFI PART"
struct.pack_into("<III", header, 8, 0x10000, 92, 0)
struct.pack_into("<QQQQ", header, 24, 1, sectors - 1, start, sectors - 1)
struct.pack_into("<QIII", header, 72, 2, 128, 128, zlib.crc32(table))
struct.pack_into("<I", header, 16, zlib.crc32(header[:92]))
image[512:1024] = header

mish = bytearray(244)
mish[:4] = b"mish"
struct.pack_into(">I", mish, 4, 1)
struct.pack_into(">Q", mish, 16, sectors)
struct.pack_into(">I", mish, 200, 1)
struct.pack_into(">I", mish, 204, 1)
struct.pack_into(">QQQQ", mish, 212, 0, sectors, 0, len(image))
xml = b"<plist><dict><data>" + base64.b64encode(mish) + b"</data></dict></plist>"
trailer = bytearray(512)
trailer[:4] = b"koly"
struct.pack_into(">II", trailer, 4, 4, 512)
struct.pack_into(">QQ", trailer, 24, 0, len(image))
struct.pack_into(">I", trailer, 60, 1)
struct.pack_into(">QQ", trailer, 216, len(image), len(xml))
struct.pack_into(">Q", trailer, 492, sectors)
pathlib.Path(sys.argv[1]).write_bytes(image + xml + trailer)
