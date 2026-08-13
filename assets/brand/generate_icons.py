#!/usr/bin/env python3
"""Generate the p-track brand icon set and README banner.

Brand concept: a dark terminal-style squircle holding three kanban columns
(todo / doing / done) in the brand palette, finished with a check on the done
column and a faint track rail underneath. Everything is code-drawn so the
assets are deterministic, crisp at every size, and reproducible.

Outputs (relative to the repository root):
  build/appicon.png                 1024x1024 master icon used by Wails
  assets/brand/icon-{16..512}.png   standalone PNG exports
  assets/brand/AppIcon.icns         macOS icon bundle (via iconutil)
  src-tauri/icons/icon.ico          Windows application resource
  assets/brand/banner.png           1280x400 README banner
  assets/brand/social.png           1280x640 social/Open Graph card

Requires: Pillow + numpy (Kimi Work managed Python). macOS for iconutil.
Run from anywhere:  python3 assets/brand/generate_icons.py
"""

from __future__ import annotations

import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

import numpy as np
from PIL import Image, ImageDraw, ImageFont

ROOT = Path(__file__).resolve().parents[2]
BRAND = ROOT / "assets" / "brand"
APPICON = ROOT / "build" / "appicon.png"
WINDOWS_ICON = ROOT / "src-tauri" / "icons" / "icon.ico"

# --- brand palette (matches the README badge colors) -----------------------
INK_TOP = (14, 18, 34)       # deep navy, top of the squircle gradient
INK_BOTTOM = (22, 30, 58)    # slightly lifted navy at the bottom
BLUE = (95, 175, 255)        # #5FAFFF  doing
GREEN = (61, 214, 163)       # #3DD6A3  done
LAVENDER = (175, 168, 255)   # #AFA8FF  todo
WHITE = (245, 248, 255)
RAIL = (140, 155, 200)

SUPER = 4  # supersampling factor for anti-aliasing


def lerp(a: tuple[int, ...], b: tuple[int, ...], t: float) -> tuple[int, ...]:
    return tuple(round(a[i] + (b[i] - a[i]) * t) for i in range(len(a)))


def vertical_gradient(size: int, top: tuple, bottom: tuple) -> Image.Image:
    """Smooth top-to-bottom gradient as an RGB image."""
    column = np.zeros((size, 1, 3), dtype=np.uint8)
    for y in range(size):
        column[y, 0] = lerp(top, bottom, y / (size - 1))
    return Image.fromarray(np.repeat(column, size, axis=1), "RGB")


def squircle_mask(size: int, radius: int) -> Image.Image:
    """Rounded-rect alpha mask (approximation of the macOS squircle)."""
    mask = Image.new("L", (size, size), 0)
    d = ImageDraw.Draw(mask)
    d.rounded_rectangle((0, 0, size - 1, size - 1), radius=radius, fill=255)
    return mask


def draw_icon(canvas: int) -> Image.Image:
    """Draw the master icon on a transparent `canvas` x `canvas` image."""
    s = canvas * SUPER

    # macOS Big Sur icon grid: 824x824 artwork centred in the 1024 canvas.
    margin = round(s * 100 / 1024)
    art = s - 2 * margin
    radius = round(art * 0.2237)

    base = Image.new("RGBA", (s, s), (0, 0, 0, 0))

    # Squircle with vertical gradient fill.
    grad = vertical_gradient(art, INK_TOP, INK_BOTTOM).convert("RGBA")
    mask = squircle_mask(art, radius)
    base.paste(grad, (margin, margin), mask)

    d = ImageDraw.Draw(base)

    # --- kanban columns -----------------------------------------------------
    # Geometry in artwork-fraction units, then scaled to pixels.
    def ax(fx: float) -> int:  # artwork-relative x
        return margin + round(art * fx)

    def ay(fy: float) -> int:  # artwork-relative y
        return margin + round(art * fy)

    bar_w = art * 0.16
    bar_r = bar_w * 0.32
    col_x = (0.20, 0.42, 0.64)          # left edge of each column
    base_y = 0.76                        # shared baseline (bottom of bars)
    top_y = (0.36, 0.28, 0.20)           # staggered heights: todo/doing/done
    fills = (LAVENDER, BLUE, GREEN)

    # Track rail under the columns.
    rail_y = ay(base_y) + round(bar_w * 0.9)
    d.rounded_rectangle(
        (ax(col_x[0]), rail_y, ax(col_x[2] + 0.16), rail_y + round(bar_w * 0.28)),
        radius=round(bar_w * 0.14),
        fill=RAIL + (90,),
    )
    # Node on the rail under the done column — "tracked to done".
    node_r = bar_w * 0.30
    node_cx = ax(col_x[2]) + bar_w / 2
    node_cy = rail_y + bar_w * 0.14
    d.ellipse(
        (node_cx - node_r, node_cy - node_r, node_cx + node_r, node_cy + node_r),
        fill=GREEN + (255,),
    )

    for fx, fy, fill in zip(col_x, top_y, fills):
        x0, x1 = ax(fx), ax(fx + 0.16)
        y0, y1 = ay(fy), ay(base_y)
        d.rounded_rectangle((x0, y0, x1, y1), radius=round(bar_r), fill=fill + (255,))

    # Check mark inside the done column (kept within the bar's width).
    cx = ax(col_x[2]) + bar_w / 2
    cy = ay(top_y[2]) + bar_w * 1.35
    arm = bar_w * 0.34
    lw = round(bar_w * 0.26)
    check_ink = (10, 34, 26, 255)
    d.line(
        [(cx - arm, cy + arm * 0.1), (cx - arm * 0.12, cy + arm * 0.78)],
        fill=check_ink, width=lw,
    )
    d.line(
        [(cx - arm * 0.12, cy + arm * 0.78), (cx + arm, cy - arm * 0.55)],
        fill=check_ink, width=lw,
    )

    return base.resize((canvas, canvas), Image.LANCZOS)


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


def export_iconset(master: Image.Image) -> None:
    """Emit PNG exports plus native Windows and macOS icon resources."""
    for size in (16, 32, 64, 128, 256, 512):
        master.resize((size, size), Image.LANCZOS).save(BRAND / f"icon-{size}.png")
        print(f"wrote assets/brand/icon-{size}.png")

    WINDOWS_ICON.parent.mkdir(parents=True, exist_ok=True)
    master.save(
        WINDOWS_ICON,
        format="ICO",
        sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
    )
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
            master.resize((px, px), Image.LANCZOS).save(iconset / name)
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

    export_iconset(master)
    draw_banner(1280, 400, BRAND / "banner.png")
    draw_banner(1280, 640, BRAND / "social.png", social=True)


if __name__ == "__main__":
    main()
