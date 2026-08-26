#!/usr/bin/env python3
"""Rasterise the QW mark to a square PNG for `cargo tauri icon`.

The mark is the hourglass in landing/app/icon.svg — five nodes, six edges,
two triangles sharing a waist. It is redrawn here from the same coordinates
rather than converted from the SVG on purpose: this host has no SVG
rasteriser (no rsvg-convert, inkscape or imagemagick), and a script that
carries the geometry as data is a source file, where a checked-in 1024px PNG
would be a build output.

    python3 render-icon.py out.png [size]              # app icon, opaque
    python3 render-icon.py fg.png [size] --foreground   # adaptive-icon layer
    cargo tauri icon out.png        # writes src-tauri/icons/*

`cargo tauri icon` builds an Android adaptive icon by reusing the opaque
square as the *foreground* layer, which is wrong: the launcher scales that
layer up and masks it, so only the central ~61% survives and a mark sized to
fill the tile loses its outermost nodes to a circle crop. --foreground draws
the glyph on transparency at that smaller size instead, to be paired with a
dark `ic_launcher_background` colour rather than tauri's default white.

Keep NODES/EDGES in sync with landing/app/icon.svg and
landing/components/qw/logo.tsx when the mark changes; nothing enforces it.

Unlike the favicon this draws on an opaque violet-tinted square: an app icon
sits on a launcher wallpaper, where a transparent glyph disappears.
"""
import sys
from PIL import Image, ImageDraw

VIEWBOX = 24.0
NODES = [(5.5, 3.5), (18.5, 3.5), (12.0, 12.0), (5.5, 20.5), (18.5, 20.5)]
EDGES = [(0, 1), (0, 2), (1, 2), (2, 3), (2, 4), (3, 4)]
R = 1.7
STROKE = 1.3

FG = (167, 139, 250, 255)      # #a78bfa, the site's primary
BG = (15, 13, 26, 255)         # near-black violet; matches the dark card

# Padding so the glyph is not flush to the icon edge — Android rounds and
# masks these, and a mark touching the bounds loses its corners.
PAD = 0.14
# Android guarantees only the central 66 of a 108dp adaptive foreground is
# visible on every launcher mask. Sizing the glyph to ~60% keeps it inside
# that on a circle, a squircle and a teardrop alike.
PAD_FG = 0.20
SS = 4                          # supersample, then LANCZOS down: PIL has no AA


def render(size: int, foreground: bool = False) -> Image.Image:
    px = size * SS
    img = Image.new("RGBA", (px, px), (0, 0, 0, 0) if foreground else BG)
    d = ImageDraw.Draw(img)

    # Fit the mark's own bounding box, not the 24-unit viewBox: the
    # hourglass is much taller than wide, so mapping the whole viewBox
    # would leave it floating small in the middle of a launcher tile.
    ink = R + STROKE / 2
    x0 = min(n[0] for n in NODES) - ink
    x1 = max(n[0] for n in NODES) + ink
    y0 = min(n[1] for n in NODES) - ink
    y1 = max(n[1] for n in NODES) + ink

    span = px * (1 - 2 * (PAD_FG if foreground else PAD))
    scale = min(span / (x1 - x0), span / (y1 - y0))
    offx = (px - (x1 - x0) * scale) / 2 - x0 * scale
    offy = (px - (y1 - y0) * scale) / 2 - y0 * scale

    def xy(p):
        return (offx + p[0] * scale, offy + p[1] * scale)

    w = STROKE * scale
    for a, b in EDGES:
        d.line([xy(NODES[a]), xy(NODES[b])], fill=FG, width=int(round(w)))
    r = R * scale
    for n in NODES:
        cx, cy = xy(n)
        d.ellipse([cx - r, cy - r, cx + r, cy + r], fill=FG)

    return img.resize((size, size), Image.LANCZOS)


if __name__ == "__main__":
    args = [a for a in sys.argv[1:] if not a.startswith("--")]
    fg = "--foreground" in sys.argv
    out = args[0] if args else "icon-1024.png"
    n = int(args[1]) if len(args) > 1 else 1024
    render(n, foreground=fg).save(out)
    print(f"{out}: {n}x{n}{' (adaptive foreground)' if fg else ''}")
