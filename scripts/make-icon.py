"""Generate Feedr's geometric icon with the standard library and macOS tools."""
import pathlib
import struct
import subprocess
import zlib

root = pathlib.Path(__file__).resolve().parents[1]
iconset = root / ".artifacts/Feedr.iconset"
iconset.mkdir(parents=True, exist_ok=True)

def rounded(x, y, rect, radius):
    x0, y0, x1, y1 = rect
    if not (x0 <= x <= x1 and y0 <= y <= y1):
        return False
    cx = max(x0 + radius, min(x, x1 - radius))
    cy = max(y0 + radius, min(y, y1 - radius))
    return (x - cx) ** 2 + (y - cy) ** 2 <= radius ** 2

rows = bytearray()
for y in range(1024):
    rows.append(0)
    for x in range(1024):
        color = (16, 24, 31, 255) if rounded(x, y, (20, 20, 1004, 1004), 216) else (0, 0, 0, 0)
        for rect in [(240, 235, 735, 345), (240, 440, 625, 550), (240, 645, 505, 755)]:
            if rounded(x, y, rect, 42):
                color = (121, 201, 203, 255)
                break
        rows.extend(color)

def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))

png = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1024, 1024, 8, 6, 0, 0, 0)) + chunk(b"IDAT", zlib.compress(rows)) + chunk(b"IEND", b"")
original = iconset / "original.png"
original.write_bytes(png)
(root / "packaging/feedr.png").write_bytes(png)
for size in [16, 32, 128, 256, 512]:
    for scale in [1, 2]:
        name = f"icon_{size}x{size}{'@2x' if scale == 2 else ''}.png"
        subprocess.run(["sips", "-z", str(size * scale), str(size * scale), str(original), "--out", str(iconset / name)], check=True, stdout=subprocess.DEVNULL)
original.unlink()
subprocess.run(["iconutil", "-c", "icns", str(iconset), "-o", str(root / "packaging/Feedr.icns")], check=True)
print("Generated packaging/Feedr.icns")
