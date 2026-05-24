#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
cd "$ROOT"
python3 << 'PY'
import struct, zlib

def png_chunk(tag, data):
    return struct.pack('>I', len(data)) + tag + data + struct.pack('>I', zlib.crc32(tag + data) & 0xffffffff)

w, h = 512, 512
rows = []
for y in range(h):
    row = b'\x00'
    for x in range(w):
        cx, cy = x - 256, y - 256
        if cx*cx + cy*cy < 200*200:
            row += bytes([91, 141, 239, 255])  # accent blue
        else:
            row += bytes([15, 17, 23, 255])
    rows.append(row)
raw = b''.join(rows)
compressed = zlib.compress(raw, 9)
ihdr = struct.pack('>IIBBBBB', w, h, 8, 6, 0, 0, 0)
png = b'\x89PNG\r\n\x1a\n' + png_chunk(b'IHDR', ihdr) + png_chunk(b'IDAT', compressed) + png_chunk(b'IEND', b'')
open('icon.png', 'wb').write(png)
print('icon.png created')
PY
mkdir -p icons
mv icon.png icons/icon.png
