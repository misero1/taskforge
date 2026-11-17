"""Generate assets/icon.ico and assets/app.res (Windows icon resource).

Run:  python tools/make_icon.py      (needs Pillow)
The .res file is linked into the exe by build.rs.
"""
import io
import struct
from pathlib import Path

from PIL import Image, ImageDraw

ROOT = Path(__file__).resolve().parent.parent
SIZES = [256, 64, 48, 32, 24, 16]


def draw(size: int) -> Image.Image:
    s = 4  # supersample
    n = size * s
    img = Image.new("RGBA", (n, n), (0, 0, 0, 0))
    # Vertical gradient (azure -> violet) inside a rounded square.
    grad = Image.new("RGBA", (n, n))
    gd = ImageDraw.Draw(grad)
    top, bot = (66, 133, 255), (124, 92, 255)
    for y in range(n):
        t = y / (n - 1)
        gd.line([(0, y), (n, y)], fill=tuple(int(a + (b - a) * t) for a, b in zip(top, bot)) + (255,))
    mask = Image.new("L", (n, n), 0)
    pad = int(n * 0.04)
    ImageDraw.Draw(mask).rounded_rectangle([pad, pad, n - pad, n - pad], radius=int(n * 0.22), fill=255)
    img.paste(grad, (0, 0), mask)
    # Heartbeat / activity line.
    pts = [(0.16, 0.56), (0.34, 0.56), (0.42, 0.34), (0.54, 0.74), (0.62, 0.46), (0.68, 0.56), (0.84, 0.56)]
    d = ImageDraw.Draw(img)
    w = max(int(n * (0.085 if size <= 24 else 0.065)), 1)
    d.line([(x * n, y * n) for x, y in pts], fill=(255, 255, 255, 255), width=w, joint="curve")
    for x, y in (pts[0], pts[-1]):
        r = w / 2
        d.ellipse([x * n - r, y * n - r, x * n + r, y * n + r], fill=(255, 255, 255, 255))
    return img.resize((size, size), Image.LANCZOS)


def png_bytes(im: Image.Image) -> bytes:
    b = io.BytesIO()
    im.save(b, format="PNG", optimize=True)
    return b.getvalue()


def res_entry(rtype: int, name: int, data: bytes, flags: int = 0x1010) -> bytes:
    header = struct.pack("<HH", 0xFFFF, rtype) + struct.pack("<HH", 0xFFFF, name)
    header += struct.pack("<IHHII", 0, flags, 0x0409, 0, 0)  # DataVersion, MemoryFlags, LangId, Version, Characteristics
    header_size = 8 + len(header)
    out = struct.pack("<II", len(data), header_size) + header + data
    out += b"\0" * ((4 - len(out) % 4) % 4)
    return out


def main() -> None:
    images = [(sz, png_bytes(draw(sz))) for sz in SIZES]
    (ROOT / "assets").mkdir(exist_ok=True)
    draw(256).save(ROOT / "assets" / "icon.png")

    # .ico file (handy for docs / shortcuts)
    ico = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    body = b""
    for sz, data in images:
        dim = 0 if sz >= 256 else sz
        ico += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset + len(body))
        body += data
    (ROOT / "assets" / "icon.ico").write_bytes(ico + body)

    # .res: empty header entry, one RT_ICON per size, one RT_GROUP_ICON (id 1)
    res = struct.pack("<II", 0, 32) + struct.pack("<HHHH", 0xFFFF, 0, 0xFFFF, 0) + b"\0" * 16
    group = struct.pack("<HHH", 0, 1, len(images))
    for i, (sz, data) in enumerate(images, start=1):
        res += res_entry(3, i, data)  # RT_ICON
        dim = 0 if sz >= 256 else sz
        group += struct.pack("<BBBBHHIH", dim, dim, 0, 0, 1, 32, len(data), i)
    res += res_entry(14, 1, group, flags=0x1030)  # RT_GROUP_ICON
    (ROOT / "assets" / "app.res").write_bytes(res)
    print("wrote assets/icon.ico, assets/icon.png, assets/app.res")


if __name__ == "__main__":
    main()
