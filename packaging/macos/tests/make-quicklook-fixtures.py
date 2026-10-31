#!/usr/bin/env python3
import struct
import sys
import zipfile
from pathlib import Path

base, zip64, malformed, oversized, multi_disk, bad_zip64, encrypted = map(Path, sys.argv[1:])
with zipfile.ZipFile(base, "w") as archive:
    archive.writestr("readme.txt", "preview only")
    archive.writestr("資料/數據.csv", "x,y\n1,2")
    archive.writestr("unsafe <tag> & name.txt", "<script>not rendered</script>")

raw = base.read_bytes()
eocd = raw.rfind(b"PK\x05\x06")
if eocd < 0:
    raise SystemExit("failed to find ZIP end record")
count = struct.unpack_from("<H", raw, eocd + 10)[0]
central_size = struct.unpack_from("<I", raw, eocd + 12)[0]
central_offset = struct.unpack_from("<I", raw, eocd + 16)[0]
zip64_record = struct.pack(
    "<IQHHIIQQQQ",
    0x06064B50,
    44,
    45,
    45,
    0,
    0,
    count,
    count,
    central_size,
    central_offset,
)
locator = struct.pack("<IIQI", 0x07064B50, 0, eocd, 1)
legacy_eocd = bytearray(raw[eocd:])
struct.pack_into("<HHII", legacy_eocd, 8, 0xFFFF, 0xFFFF, 0xFFFFFFFF, 0xFFFFFFFF)
zip64.write_bytes(raw[:eocd] + zip64_record + locator + legacy_eocd)

damaged = bytearray(raw)
damaged[central_offset : central_offset + 4] = b"BAD!"
malformed.write_bytes(damaged)

large_metadata = bytearray(raw)
struct.pack_into("<I", large_metadata, eocd + 12, 16 * 1024 * 1024 + 1)
oversized.write_bytes(large_metadata)

multiple_disk = bytearray(raw)
struct.pack_into("<H", multiple_disk, eocd + 4, 1)
multi_disk.write_bytes(multiple_disk)

bad_record = bytearray(zip64.read_bytes())
zip64_eocd = bad_record.rfind(b"PK\x06\x06")
if zip64_eocd < 0:
    raise SystemExit("failed to find ZIP64 end record")
struct.pack_into("<Q", bad_record, zip64_eocd + 4, 1 << 40)
bad_zip64.write_bytes(bad_record)

encrypted_entry = bytearray(raw)
name_start = central_offset + 46
if encrypted_entry[name_start : name_start + len(b"readme.txt")] != b"readme.txt":
    raise SystemExit("expected the first central-directory entry to be readme.txt")
flags = struct.unpack_from("<H", encrypted_entry, central_offset + 8)[0]
struct.pack_into("<H", encrypted_entry, central_offset + 8, flags | 1)
encrypted.write_bytes(encrypted_entry)
