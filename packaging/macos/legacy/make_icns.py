#!/usr/bin/env python3
"""Writes packaging/macos/legacy/Litecord.icns and packaging/macos/AppIcon.png from assets/app_icon.png (needs Pillow).

The icon files current tools make hold PNG pictures only, which OS X 10.5 / 10.6 show for
the two largest sizes at best. This writes the classic layout those systems read at every
size: run-length packed RGB plus a separate 8-bit mask for 16, 32, 48 and 128 pixels, and
PNG for 256 and 512. The result is committed, so builds do not need Pillow.

    python3 packaging/macos/legacy/make_icns.py
"""
import io
import os
import struct

from PIL import Image

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.dirname(os.path.dirname(os.path.dirname(HERE)))


def pack_plane(data):
    """The icns run-length scheme: 0-127 = that many + 1 literal bytes, 128-255 = repeat the next byte (n - 125) times."""
    out = bytearray()
    i, n = 0, len(data)
    while i < n:
        run = 1
        while i + run < n and run < 130 and data[i + run] == data[i]:
            run += 1
        if run >= 3:
            out += bytes([run + 125, data[i]])
            i += run
            continue
        start = i
        while i < n and i - start < 128:
            run = 1
            while i + run < n and run < 3 and data[i + run] == data[i]:
                run += 1
            if run >= 3:
                break
            i += 1
        out += bytes([i - start - 1]) + data[start:i]
    return bytes(out)


def classic(image, size, rgb_type, mask_type):
    picture = image.resize((size, size), Image.LANCZOS)
    r, g, b, a = [channel.tobytes() for channel in picture.split()]
    packed = pack_plane(r) + pack_plane(g) + pack_plane(b)
    if rgb_type == b"it32":
        packed = b"\0\0\0\0" + packed  # the 128 pixel entry starts with four zero bytes
    return [(rgb_type, packed), (mask_type, a)]


def png(image, size, kind):
    buffer = io.BytesIO()
    image.resize((size, size), Image.LANCZOS).save(buffer, format="PNG", optimize=True)
    return [(kind, buffer.getvalue())]


def on_tile(glyph, size=1024):
    """The project's logo is white on a transparent background, which disappears in a light
    Dock, Finder window or Applications stack. This puts it on a rounded tile in the app's
    accent colour, with the margin macOS icons usually have."""
    from PIL import ImageDraw

    scale = 4  # drawn larger and reduced, for smooth corners
    big = size * scale
    tile = Image.new("RGBA", (big, big), (0, 0, 0, 0))
    margin = int(big * 0.09)
    ImageDraw.Draw(tile).rounded_rectangle(
        (margin, margin, big - margin, big - margin), radius=int(big * 0.19), fill=(0x58, 0x65, 0xF2, 0xFF)
    )
    tile = tile.resize((size, size), Image.LANCZOS)
    inner = int(size * 0.54)
    logo = glyph.resize((inner, inner), Image.LANCZOS)
    tile.alpha_composite(logo, ((size - inner) // 2, (size - inner) // 2))
    return tile


def main():
    glyph = Image.open(os.path.join(ROOT, "assets", "app_icon.png")).convert("RGBA")
    image = on_tile(glyph)
    # The same picture as a PNG, for the icon of the normal macOS app (scripts/macos/bundle.sh).
    image.save(os.path.join(os.path.dirname(HERE), "AppIcon.png"), format="PNG", optimize=True)
    entries = []
    entries += classic(image, 16, b"is32", b"s8mk")
    entries += classic(image, 32, b"il32", b"l8mk")
    entries += classic(image, 48, b"ih32", b"h8mk")
    entries += classic(image, 128, b"it32", b"t8mk")
    entries += png(image, 256, b"ic08")
    entries += png(image, 512, b"ic09")
    body = b"".join(kind + struct.pack(">I", len(data) + 8) + data for kind, data in entries)
    target = os.path.join(HERE, "Litecord.icns")
    with open(target, "wb") as handle:
        handle.write(b"icns" + struct.pack(">I", len(body) + 8) + body)
    print(f"wrote {target}: {len(body) + 8} bytes, {len(entries)} entries")


if __name__ == "__main__":
    main()
