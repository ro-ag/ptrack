#!/usr/bin/env python3
"""Generate the p-track brand icon set and README banner.

Brand concept: a dark squircle holding three kanban columns (todo / doing /
done) in the brand palette, with a hand-drawn check mark carved through the
columns as negative space — progress, checked off. Below 48 px the carving
cannot survive, so small frames simplify to the three-bar silhouette alone,
pixel-snapped for crisp taskbar and favicon rendering. Everything is
code-drawn so the assets are deterministic, crisp at every size, and
reproducible.

Outputs (relative to the repository root):
  build/appicon.png                 1024x1024 master icon
  assets/brand/icon-{16..512}.png   standalone PNG exports
  assets/brand/AppIcon.icns         macOS icon bundle (via iconutil)
  src-tauri/icons/icon.ico          Windows application resource (per-size frames)
  assets/brand/banner.png           1280x400 README banner
  assets/brand/social.png           1280x640 social/Open Graph card

Requires: Pillow + numpy. macOS for iconutil.
Run from anywhere:  python3 assets/brand/generate_icons.py
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageChops, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
BRAND = ROOT / "assets" / "brand"
APPICON = ROOT / "build" / "appicon.png"
WINDOWS_ICON = ROOT / "src-tauri" / "icons" / "icon.ico"

# --- brand palette (matches the README badge colors) -----------------------
INK_TOP = (14, 18, 34)       # deep navy, top of the squircle gradient
INK_BOTTOM = (38, 50, 94)    # lifted navy at the bottom (perceptible sweep)
BLUE = (95, 175, 255)        # #5FAFFF  doing
GREEN = (61, 214, 163)       # #3DD6A3  done
LAVENDER = (175, 168, 255)   # #AFA8FF  todo
WHITE = (245, 248, 255)

SUPER = 4  # supersampling factor for anti-aliasing

# Hand-drawn check mark (flattened from a potrace vector), carved through the
# columns as negative space. x,y pairs on a 1000-wide grid, aspect 0.929.
CHECK_GLYPH = (
    "981,7,949,26,917,46,884,69,851,92,817,118,784,144,750,171,717,200,690,224,"
    "663,248,636,274,609,301,581,328,554,356,528,385,501,414,483,434,464,455,"
    "446,477,427,499,408,521,390,544,372,567,354,590,347,599,338,611,328,624,"
    "317,638,306,653,297,666,288,677,282,686,280,689,278,692,276,694,274,696,"
    "273,698,272,699,271,699,271,700,268,698,260,694,248,687,231,677,212,666,"
    "189,654,165,640,138,625,7,551,4,554,3,555,3,555,2,556,1,557,1,557,1,558,"
    "0,558,0,559,1,560,6,567,18,581,40,607,76,649,129,710,200,793,295,903,"
    "318,929,321,929,324,929,342,893,375,829,408,768,441,708,474,651,508,596,"
    "542,542,578,490,614,439,655,384,698,329,742,276,787,224,834,173,882,123,"
    "931,74,982,26,986,23,989,20,992,17,995,14,997,12,999,11,1000,10,1000,9,"
    "1000,9,999,8,998,6,997,5,996,3,995,2,994,1,994,0,993,0,992,1,991,1,990,2,"
    "988,3,986,4,983,6,981,7"
)
_glyph_values = [int(v) for v in CHECK_GLYPH.split(",")]
GLYPH_POINTS = list(zip(_glyph_values[0::2], _glyph_values[1::2]))
GLYPH_ASPECT = 0.929


def lerp(a: tuple[int, ...], b: tuple[int, ...], t: float) -> tuple[int, ...]:
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def vertical_gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    """Smooth top-to-bottom gradient as an RGB image."""
    column = np.zeros((size, 1, 3), dtype=np.uint8)
    for y in range(size):
        column[y, 0] = lerp(top, bottom, y / (size - 1))
    return Image.fromarray(np.repeat(column, size, axis=1))


def squircle_mask(size: int, radius: int) -> Image.Image:
    """Rounded-rect alpha mask (approximation of the macOS squircle)."""
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    d.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def check_mask(s: int, margin: int, art: int) -> Image.Image:
    """The brush check scaled to the artwork, as a filled polygon mask."""
    width = art * 0.66
    height = width * GLYPH_ASPECT
    ox = margin + art * 0.50 - width / 2
    oy = margin + art * 0.47 - height / 2
    mask = Image.new("L", (s, s), 0)
    ImageDraw.Draw(mask).polygon(
        [(ox + x / 1000 * width, oy + y / 1000 * width) for x, y in GLYPH_POINTS],
        fill=255,
    )
    return mask


def draw_icon_full(canvas: int) -> Image.Image:
    """Full design: gradient squircle, rim light, bars, check carved out."""
    s = canvas * SUPER

    # macOS Big Sur icon grid: 824x824 artwork centred in the 1024 canvas.
    margin = round(s * 100 / 1024)
    art = s - 2 * margin
    radius = round(art * 0.2237)

    base = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    grad = vertical_gradient(art, INK_TOP, INK_BOTTOM).convert("RGBA")
    mask = squircle_mask(art, radius)
    base.paste(grad, (margin, margin), mask)

    # Top rim light so the tile separates from dark grounds.
    rim = Image.new("L", (art, art), 0)
    rim_h = round(art * 0.18)
    rd = ImageDraw.Draw(rim)
    for y in range(rim_h):
        rd.line([(0, y), (art, y)], fill=round(30 * (1 - y / rim_h)))
    clipped = Image.new("L", (art, art), 0)
    clipped.paste(rim, (0, 0), mask)
    rim_img = Image.new("RGBA", (art, art), (255, 255, 255, 0))
    rim_img.putalpha(clipped)
    base.alpha_composite(rim_img, (margin, margin))

    # Kanban columns on their own layer so the check can carve through them.
    def ax(fx: float) -> int:
        return margin + round(art * fx)

    def ay(fy: float) -> int:
        return margin + round(art * fy)

    bars = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    d = ImageDraw.Draw(bars)
    bar_w = art * 0.17
    for fx, fy, fill in zip(
        (0.175, 0.415, 0.655), (0.38, 0.29, 0.20), (LAVENDER, BLUE, GREEN)
    ):
        d.rounded_rectangle(
            (ax(fx), ay(fy), ax(fx + 0.17), ay(0.80)),
            radius=round(bar_w * 0.34),
            fill=fill + (255,),
        )

    bars.putalpha(ImageChops.subtract(bars.split()[3], check_mask(s, margin, art)))
    base.alpha_composite(bars)
    return base.resize((canvas, canvas), Image.LANCZOS)


# Pixel-snapped small frames: (x0, y0, x1, y1) per bar, plus corner radii.
SMALL_FRAMES = {
    16: {"tile_radius": 3, "bar_radius": 1,
         "bars": [(3, 8, 5, 13), (7, 6, 9, 13), (11, 4, 13, 13)]},
    32: {"tile_radius": 7, "bar_radius": 2,
         "bars": [(6, 15, 11, 26), (14, 11, 19, 26), (22, 7, 27, 26)]},
}


def draw_icon_small(canvas: int) -> Image.Image:
    """<=48 px: three-bar silhouette only, pixel-snapped, no carving."""
    ref = 16 if canvas <= 20 else 32
    spec = SMALL_FRAMES[ref]
    k = canvas / ref
    img = Image.new("RGBA", (canvas, canvas), (0, 0, 0, 0))
    grad = vertical_gradient(canvas, INK_TOP, INK_BOTTOM).convert("RGBA")
    mask = squircle_mask(canvas, max(2, round(spec["tile_radius"] * k)))
    img.paste(grad, (0, 0), mask)
    d = ImageDraw.Draw(img)
    for (x0, y0, x1, y1), fill in zip(spec["bars"], (LAVENDER, BLUE, GREEN)):
        d.rounded_rectangle(
            (round(x0 * k), round(y0 * k), round(x1 * k), round(y1 * k)),
            radius=max(1, round(spec["bar_radius"] * k)),
            fill=fill + (255,),
        )
    return img


def draw_icon(canvas: int) -> Image.Image:
    return draw_icon_small(canvas) if canvas <= 48 else draw_icon_full(canvas)


def find_font(preferred: list[tuple[str, int]]) -> str | None:
    for path, _ in preferred:
        if Path(path).exists():
            return path
    return None


MENLO = find_font([
    ("/System/Library/Fonts/Menlo.ttc", 1),      # Bold face
    ("/System/Library/Fonts/SFNSMono.ttf", 0),
])


def menlo(size: int, bold: bool = True) -> ImageFont.FreeTypeFont:
    if MENLO and MENLO.endswith(".ttc"):
        return ImageFont.truetype(MENLO, size, index=1 if bold else 0)
    if MENLO:
        return ImageFont.truetype(MENLO, size)
    return ImageFont.load_default(size)


def draw_banner(width: int, height: int, out: Path, social: bool = False) -> None:
    s = SUPER
    W, H = width * s, height * s
    img = vertical_gradient(H, INK_TOP, INK_BOTTOM).convert("RGBA")
    # Stretch the vertical gradient horizontally.
    img = img.resize((W, H))
    d = ImageDraw.Draw(img)

    icon_px = round(H * (0.62 if not social else 0.56))
    icon = draw_icon(icon_px)
    ix = round(W * 0.055)
    iy = (H - icon_px) // 2
    img.paste(icon, (ix, iy), icon)

    tx = ix + icon_px + round(W * 0.04)
    max_text_w = W - tx - round(W * 0.05)

    title_size = round(H * (0.30 if not social else 0.24))
    title_font = menlo(title_size)
    title = "p-track"
    tagline = "Observe agent work. Keep the plan. Pass the context."

    # Shrink the tagline until it fits the canvas width.
    tag_size = round(H * (0.085 if not social else 0.062))
    while tag_size > 8:
        tag_font = menlo(tag_size, bold=False)
        if d.textlength(tagline, font=tag_font) <= max_text_w:
            break
        tag_size -= 2

    ty = H // 2 - title_size // 2 - round(H * 0.05)
    d.text((tx, ty), title, font=title_font, fill=WHITE)
    bbox = d.textbbox((tx, ty), title, font=title_font)
    d.text((tx, ty + (bbox[3] - bbox[1]) + round(H * 0.04)), tagline,
           font=tag_font, fill=(170, 185, 220))

    out.parent.mkdir(parents=True, exist_ok=True)
    img.resize((width, height), Image.LANCZOS).save(out)
    print(f"wrote {out.relative_to(ROOT)}")


def export_iconset() -> None:
    """Emit PNG exports plus native Windows and macOS icon resources."""
    for size in (16, 32, 64, 128, 256, 512):
        draw_icon(size).save(BRAND / f"icon-{size}.png")
        print(f"wrote assets/brand/icon-{size}.png")

    # Per-size ICO frames so the small designs actually ship on Windows.
    ico_frames = [draw_icon(size) for size in (16, 24, 32, 48, 64, 128, 256)]
    WINDOWS_ICON.parent.mkdir(parents=True, exist_ok=True)
    ico_frames[-1].save(WINDOWS_ICON, format="ICO", append_images=ico_frames[:-1])
    print(f"wrote {WINDOWS_ICON.relative_to(ROOT)}")

    if shutil.which("iconutil") is None:
        print("iconutil not found; skipping .icns", file=sys.stderr)
        return
    with tempfile.TemporaryDirectory() as tmp:
        iconset = Path(tmp) / "ptrack.iconset"
        iconset.mkdir()
        specs = {
            "icon_16x16.png": 16, "icon_16x16@2x.png": 32,
            "icon_32x32.png": 32, "icon_32x32@2x.png": 64,
            "icon_128x128.png": 128, "icon_128x128@2x.png": 256,
            "icon_256x256.png": 256, "icon_256x256@2x.png": 512,
            "icon_512x512.png": 512, "icon_512x512@2x.png": 1024,
        }
        for name, px in specs.items():
            draw_icon(px).save(iconset / name)
        subprocess.run(
            ["iconutil", "-c", "icns", str(iconset), "-o", str(BRAND / "AppIcon.icns")],
            check=True,
        )
        print("wrote assets/brand/AppIcon.icns")


def main() -> None:
    BRAND.mkdir(parents=True, exist_ok=True)
    master = draw_icon(1024)

    APPICON.parent.mkdir(parents=True, exist_ok=True)
    master.save(APPICON)
    print(f"wrote {APPICON.relative_to(ROOT)}")

    export_iconset()
    draw_banner(1280, 400, BRAND / "banner.png")
    draw_banner(1280, 640, BRAND / "social.png", social=True)


if __name__ == "__main__":
    main()
