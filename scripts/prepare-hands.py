#!/usr/bin/env python3
"""Prepare the hand images for the Linux build of TinyHand.

A maintainer tool: the prepared hands are committed in assets/hands, so building
TinyHand never runs this. Run it when the original app gains or changes hands:

    scripts/prepare-hands.py --src path/to/nilbuild/tinyhand/assets

Reads hand_*.png (1024x1536 RGBA) from --src and writes smaller copies with the same
names to assets/hands. Needs Pillow (python3-pil / python-pillow).

What it does to each image:

1. Clears the colour of fully transparent pixels. Most of the source files carry
   leftover RGB under alpha 0. The app premultiplies before drawing, so those
   values never reach the screen, but they make up most of the file size.
   This step alone changes nothing that can be drawn.

2. Crops empty columns on the left and right. The kept range is the box around
   pixels with alpha > --alpha-floor (default 8, about 3%), plus --margin pixels
   (default 24) on each side. Inside that range every pixel is kept as it is.
   Outside it, the source images only hold scattered background-removal specks
   with alpha <= 8; the script prints the strongest one it drops for each file.
   Use --lossless to drop nothing but fully transparent columns.

   Why a horizontal crop is safe: the app (hand.rs `measure`, pose.rs, raster.rs)
   turns every x position into a fraction of the image width and multiplies it
   back by `width_pts = scale * aspect`, where aspect = w / h. Removing x0 columns
   moves every such position by the same amount, and the drawing is anchored on
   `rod_x`, so the rendered hand doesn't change. The crop doesn't have to be
   symmetric. `measure` only looks at alpha > 128, so the specks don't affect it.

   Height is never touched: the app measures tip, wrist and rod end as fractions
   of the full image height, including the transparent rows at the top and bottom.

3. Saves with maximum zlib compression, trying a few deflate strategies and
   keeping the smallest. Metadata (EXIF, sRGB chunk) is dropped.

The images are never downscaled: the hand is about 570 px tall in the source,
which is roughly what a 4K screen needs at the largest size setting.

Usage: scripts/prepare-hands.py --src DIR [--out DIR] [--alpha-floor N]
                                [--margin PX] [--lossless]
"""

import argparse
import io
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    sys.exit("prepare-hands: needs Pillow (Arch: python-pillow, Debian/Ubuntu: python3-pil)")

REPO_DIR = Path(__file__).resolve().parent.parent

# zlib strategies: 0 default, 1 filtered, 3 RLE. RLE often wins on these images.
STRATEGIES = (0, 1, 3)


def clear_hidden_rgb(im):
    """Set RGB to 0 wherever alpha is 0. Invisible once premultiplied."""
    r, g, b, a = im.split()
    visible = a.point(lambda v: 255 if v else 0)
    black = Image.new("L", im.size, 0)
    return Image.merge("RGBA", [Image.composite(c, black, visible) for c in (r, g, b)] + [a])


def crop_columns(im, alpha_floor, margin):
    """Columns to keep, and the strongest alpha dropped outside them."""
    a = im.getchannel("A")
    box = a.point(lambda v: 255 if v > alpha_floor else 0).getbbox()
    if box is None:
        return (0, im.width), 0
    x0 = max(0, box[0] - margin)
    x1 = min(im.width, box[2] + margin)
    dropped = 0
    for side in ((0, 0, x0, im.height), (x1, 0, im.width, im.height)):
        if side[2] > side[0]:
            dropped = max(dropped, a.crop(side).getextrema()[1])
    return (x0, x1), dropped


def encode(im):
    best = None
    for strategy in STRATEGIES:
        buf = io.BytesIO()
        im.save(buf, "PNG", optimize=True, compress_type=strategy)
        if best is None or buf.tell() < len(best):
            best = buf.getvalue()
    return best


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--src", type=Path, required=True, help="the original app's assets folder (hand_*.png)")
    ap.add_argument("--out", type=Path, default=REPO_DIR / "assets" / "hands", help="output folder (default assets/hands)")
    ap.add_argument("--alpha-floor", type=int, default=8, metavar="N",
                    help="columns holding only alpha <= N are cropped (0-255, default 8)")
    ap.add_argument("--margin", type=int, default=24, metavar="PX",
                    help="transparent columns kept on each side (default 24)")
    ap.add_argument("--lossless", action="store_true", help="only crop fully transparent columns")
    args = ap.parse_args()
    if args.lossless:
        args.alpha_floor = 0

    files = sorted(args.src.glob("hand_*.png"))
    if not files:
        sys.exit(f"prepare-hands: no hand_*.png in {args.src}")
    args.out.mkdir(parents=True, exist_ok=True)

    before = after = 0
    for path in files:
        src = Image.open(path)
        src.load()
        im = clear_hidden_rgb(src.convert("RGBA"))
        (x0, x1), dropped = crop_columns(im, args.alpha_floor, args.margin)
        im = im.crop((x0, 0, x1, im.height))
        data = encode(im)
        (args.out / path.name).write_bytes(data)
        size_in = path.stat().st_size
        before += size_in
        after += len(data)
        print(f"{path.name:22} {src.width}x{src.height} -> {im.width}x{im.height} "
              f"(cols {x0}-{x1}, max alpha dropped {dropped:3})  "
              f"{size_in / 1024:7.0f} KB -> {len(data) / 1024:5.0f} KB")

    print(f"\n{len(files)} hands: {before / 1e6:.1f} MB -> {after / 1e6:.1f} MB "
          f"({100 * (1 - after / before):.0f}% smaller) in {args.out}")


if __name__ == "__main__":
    main()
