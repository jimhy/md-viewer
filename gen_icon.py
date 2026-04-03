"""Generate high-quality ICO from 1.png"""
from PIL import Image
import struct, io, os

src = Image.open(os.path.join(os.path.dirname(__file__), "1.png")).convert("RGBA")
sizes = [16, 24, 32, 48, 64, 128, 256]

# Build ICO manually for best quality
# ICO with PNG-compressed entries for all sizes
entries = []
for s in sizes:
    img = src.resize((s, s), Image.LANCZOS)
    buf = io.BytesIO()
    img.save(buf, format="PNG")
    entries.append((s, buf.getvalue()))

# ICO header: 2 reserved + 2 type (1=ico) + 2 count
header = struct.pack("<HHH", 0, 1, len(entries))

# Each directory entry: 16 bytes
dir_offset = 6 + 16 * len(entries)
directory = b""
data = b""
for (s, png_data) in entries:
    w = 0 if s == 256 else s  # 0 means 256
    h = 0 if s == 256 else s
    directory += struct.pack("<BBBBHHII", w, h, 0, 0, 1, 32, len(png_data), dir_offset)
    data += png_data
    dir_offset += len(png_data)

out = os.path.join(os.path.dirname(__file__), "icon.ico")
with open(out, "wb") as f:
    f.write(header + directory + data)

print(f"Created {out} ({len(entries)} sizes)")
