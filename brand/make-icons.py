#!/usr/bin/env python3
"""Regenerate every app icon from `brand/logo-source.png`.

Run from the repo root when the logo changes:  python3 brand/make-icons.py

Two things are derived from the one source:

* the **app icon** — the badge with its white surround removed, so macOS and Windows do not draw a
  white square behind the rounded artwork;
* the **menu-bar glyph** — a monochrome template image of the mark alone (the N and its dot), which
  is what a macOS status item needs so the system can recolour it per theme.

macOS only: it shells out to `sips` and `iconutil` for resizing and `.icns` packing. That is an
acceptable limit for a step run once per logo revision.

Standard library only; the PNG codec below is the minimum needed to read and write 8-bit RGBA.
See docs/wiki/desktop.md
"""

import struct
import subprocess
import sys
import tempfile
import zlib
from collections import deque
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SOURCE = ROOT / "brand" / "logo-source.png"
ICONS = ROOT / "crates" / "desktop" / "icons"


# --------------------------------------------------------------------------- PNG codec


def read_png(path):
    data = Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", f"{path} is not a PNG"
    pos, idat, w, h, color = 8, b"", 0, 0, 0
    while pos < len(data):
        (length,) = struct.unpack(">I", data[pos : pos + 4])
        tag = data[pos + 4 : pos + 8]
        body = data[pos + 8 : pos + 8 + length]
        if tag == b"IHDR":
            w, h, depth, color, _, _, interlace = struct.unpack(">IIBBBBB", body)
            assert depth == 8 and interlace == 0, "expected an 8-bit, non-interlaced PNG"
        elif tag == b"IDAT":
            idat += body
        elif tag == b"IEND":
            break
        pos += 12 + length

    channels = {0: 1, 2: 3, 4: 2, 6: 4}[color]
    raw = zlib.decompress(idat)
    stride = w * channels
    rows, prev, p = [], bytearray(stride), 0
    for _ in range(h):
        f = raw[p]
        p += 1
        line = bytearray(raw[p : p + stride])
        p += stride
        for i in range(stride):
            a = line[i - channels] if i >= channels else 0
            b = prev[i]
            c = prev[i - channels] if i >= channels else 0
            if f == 1:
                line[i] = (line[i] + a) & 255
            elif f == 2:
                line[i] = (line[i] + b) & 255
            elif f == 3:
                line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[i] = (line[i] + pr) & 255
        prev = line
        row = []
        for x in range(w):
            q = line[x * channels : (x + 1) * channels]
            if channels == 4:
                row.append(tuple(q))
            elif channels == 3:
                row.append((q[0], q[1], q[2], 255))
            elif channels == 2:
                row.append((q[0], q[0], q[0], q[1]))
            else:
                row.append((q[0], q[0], q[0], 255))
        rows.append(row)
    return rows


def write_png(path, px):
    h, w = len(px), len(px[0])
    raw = bytearray()
    for row in px:
        raw.append(0)
        for r, g, b, a in row:
            raw += bytes((r, g, b, a))

    def chunk(tag, body):
        c = struct.pack(">I", len(body)) + tag + body
        return c + struct.pack(">I", zlib.crc32(tag + body) & 0xFFFFFFFF)

    blob = b"\x89PNG\r\n\x1a\n"
    blob += chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
    blob += chunk(b"IDAT", zlib.compress(bytes(raw), 9))
    blob += chunk(b"IEND", b"")
    Path(path).write_bytes(blob)


# --------------------------------------------------------------------------- derivation


def luma(p):
    return (p[0] * 299 + p[1] * 587 + p[2] * 114) // 1000


def flood(w, h, passable):
    """Every pixel reachable from the border through `passable`."""
    hit = [[False] * w for _ in range(h)]
    q = deque()
    border = [(x, y) for x in range(w) for y in (0, h - 1)]
    border += [(x, y) for y in range(h) for x in (0, w - 1)]
    for x, y in border:
        if passable(x, y) and not hit[y][x]:
            hit[y][x] = True
            q.append((x, y))
    while q:
        x, y = q.popleft()
        for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
            nx, ny = x + dx, y + dy
            if 0 <= nx < w and 0 <= ny < h and not hit[ny][nx] and passable(nx, ny):
                hit[ny][nx] = True
                q.append((nx, ny))
    return hit


def neighbours(x, y, w, h):
    for dx, dy in ((1, 0), (-1, 0), (0, 1), (0, -1)):
        nx, ny = x + dx, y + dy
        if 0 <= nx < w and 0 <= ny < h:
            yield nx, ny


def derive(px):
    h, w = len(px), len(px[0])

    # The surround is opaque white, so flood it from the border and drop it.
    bg = flood(w, h, lambda x, y: luma(px[y][x]) >= 250)

    # Badge colour is the MODE inside, never the minimum: the artwork has a drop shadow reaching
    # pure black, which would drag a min-based threshold down to nothing.
    hist = [0] * 256
    for y in range(h):
        for x in range(w):
            if not bg[y][x]:
                hist[luma(px[y][x])] += 1
    dark = max(range(256), key=lambda v: hist[v])

    icon = [[(0, 0, 0, 0)] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            if bg[y][x]:
                continue
            r, g, b, a = px[y][x]
            if any(bg[ny][nx] for nx, ny in neighbours(x, y, w, h)):
                # Ramp the rim that was antialiased against white, or it reads as a bright halo.
                a = max(0, min(255, round(255 * (255 - luma(px[y][x])) / max(255 - dark, 1))))
            icon[y][x] = (r, g, b, a)

    # The badge is the largest connected run of dark pixels. Going by "not background" alone would
    # also catch the drop shadow, which is light enough to read as part of the mark.
    dark_px = [[luma(px[y][x]) < 120 for x in range(w)] for y in range(h)]
    seen = [[False] * w for _ in range(h)]
    badge = []
    for sy in range(h):
        for sx in range(w):
            if not dark_px[sy][sx] or seen[sy][sx]:
                continue
            comp, stack = [], [(sx, sy)]
            seen[sy][sx] = True
            while stack:
                x, y = stack.pop()
                comp.append((x, y))
                for nx, ny in neighbours(x, y, w, h):
                    if dark_px[ny][nx] and not seen[ny][nx]:
                        seen[ny][nx] = True
                        stack.append((nx, ny))
            if len(comp) > len(badge):
                badge = comp

    inside = [[False] * w for _ in range(h)]
    for x, y in badge:
        inside[y][x] = True
    outside = flood(w, h, lambda x, y: not inside[y][x])

    # The mark IS the badge's holes, so they are solid whatever their colour — a
    # luminance-proportional alpha would render the lilac dot as a ghost beside the white N, when in
    # a menu bar both are simply "the mark". Only badge pixels bordering a hole get a ramp, to keep
    # that edge smooth; the badge's OUTER rim is left alone, since it borders the drop shadow.
    mask = [[0] * w for _ in range(h)]
    for y in range(h):
        for x in range(w):
            if outside[y][x]:
                continue
            if not inside[y][x]:
                mask[y][x] = 255
                continue
            if any(
                not inside[ny][nx] and not outside[ny][nx]
                for nx, ny in neighbours(x, y, w, h)
            ):
                mask[y][x] = max(0, min(255, round(255 * (luma(px[y][x]) - dark) / 55)))

    xs = [x for y in range(h) for x in range(w) if mask[y][x] > 40]
    ys = [y for y in range(h) for x in range(w) if mask[y][x] > 40]
    x0, x1, y0, y1 = min(xs), max(xs), min(ys), max(ys)
    side = max(x1 - x0, y1 - y0) + 1
    side += round(side * 0.12) * 2  # breathing room, or the glyph crowds the menu bar
    cx0 = x0 - (side - (x1 - x0 + 1)) // 2
    cy0 = y0 - (side - (y1 - y0 + 1)) // 2

    glyph = [[(0, 0, 0, 0)] * side for _ in range(side)]
    for y in range(side):
        for x in range(side):
            sx, sy = cx0 + x, cy0 + y
            if 0 <= sx < w and 0 <= sy < h:
                glyph[y][x] = (0, 0, 0, mask[sy][sx])
    return icon, glyph


# --------------------------------------------------------------------------- output


def sips(src, size, dst):
    subprocess.run(
        ["sips", "-z", str(size), str(size), str(src), "--out", str(dst)],
        check=True,
        capture_output=True,
    )


def write_ico(src, dst, sizes=(16, 32, 48, 64, 128, 256)):
    entries = []
    with tempfile.TemporaryDirectory() as td:
        for s in sizes:
            p = Path(td) / f"{s}.png"
            sips(src, s, p)
            entries.append((s, p.read_bytes()))
    offset = 6 + 16 * len(entries)
    head = struct.pack("<HHH", 0, 1, len(entries))
    table, blobs = b"", b""
    for s, data in entries:
        dim = 0 if s >= 256 else s  # 0 means 256 in an ICO directory entry
        table += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        blobs += data
        offset += len(data)
    Path(dst).write_bytes(head + table + blobs)


ICNS_SIZES = [
    (16, "icon_16x16"), (32, "icon_16x16@2x"), (32, "icon_32x32"),
    (64, "icon_32x32@2x"), (128, "icon_128x128"), (256, "icon_128x128@2x"),
    (256, "icon_256x256"), (512, "icon_256x256@2x"), (512, "icon_512x512"),
    (1024, "icon_512x512@2x"),
]


def main():
    if sys.platform != "darwin":
        sys.exit("macOS only: this script uses sips and iconutil.")
    if not SOURCE.exists():
        sys.exit(f"missing {SOURCE}")
    ICONS.mkdir(parents=True, exist_ok=True)

    icon, glyph = derive(read_png(SOURCE))
    base = ICONS / "icon.png"
    write_png(base, icon)
    # Anything larger than the source is an upscale — swap in a 1024px master when one exists.
    sips(base, 1024, base)

    for size in (32, 128, 256, 512):
        sips(base, size, ICONS / f"{size}x{size}.png")
    sips(base, 256, ICONS / "128x128@2x.png")

    iconset = ICONS / "icon.iconset"
    iconset.mkdir(exist_ok=True)
    for size, name in ICNS_SIZES:
        sips(base, size, iconset / f"{name}.png")
    subprocess.run(
        ["iconutil", "-c", "icns", str(iconset), "-o", str(ICONS / "icon.icns")], check=True
    )
    for f in iconset.iterdir():
        f.unlink()
    iconset.rmdir()

    write_ico(base, ICONS / "icon.ico")

    for name, size in (("tray.png", 32), ("tray@2x.png", 64)):
        write_png(ICONS / name, glyph)
        sips(ICONS / name, size, ICONS / name)

    print(f"icons regenerated in {ICONS.relative_to(ROOT)}")


if __name__ == "__main__":
    main()
