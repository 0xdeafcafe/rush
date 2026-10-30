#!/bin/sh
# Turns a generated icon into the app's icons: cuts just inside its painted
# tile, then writes rush-icon.png (the tile on the macOS grid, 824 on 1024,
# with a soft shadow) and the menu bar app's AppIcon.icns from it, and
# AppIcon-full.icns from the whole square, for macOS 26 on, which draws its
# own tile. Usage: docs/brand/finish-icon.sh docs/brand/rush-icon-source.png
set -e
brand=$(cd "$(dirname "$0")" && pwd)
icons="$brand/../../rust/crates/rush-menubar/icons"
tmp=$(mktemp -d)
python3 - "$1" "$tmp" <<'PY'
import sys
import numpy as np
from PIL import Image, ImageFilter
src, tmp = Image.open(sys.argv[1]).convert("RGBA"), sys.argv[2]
# Outside the tile is see-through, or a flat colour: its corner's.
px = np.array(src)
bg = px[2, 2, :3].astype(int)
like = (px[:, :, 3] < 230) | (np.abs(px[:, :, :3].astype(int) - bg).sum(2) < 300)
# Only what reaches the border: the label's black text is the tile's.
out_of = np.zeros_like(like)
out_of[[0, -1], :], out_of[:, [0, -1]] = like[[0, -1], :], like[:, [0, -1]]
while True:
    grown = out_of.copy()
    grown[1:] |= out_of[:-1]; grown[:-1] |= out_of[1:]
    grown[:, 1:] |= out_of[:, :-1]; grown[:, :-1] |= out_of[:, 1:]
    grown &= like
    if (grown == out_of).all():
        break
    out_of = grown
px[:, :, 3] = np.where(out_of, 0, px[:, :, 3])
src = Image.fromarray(px)
a = ~out_of
rows, cols = np.where(a.sum(1) > 300)[0], np.where(a.sum(0) > 300)[0]
cx, cy = (cols[0] + cols[-1]) / 2, (rows[0] + rows[-1]) / 2
half = min(cols[-1] - cols[0], rows[-1] - rows[0]) / 2 - 16  # inside its painted edge
box = (round(cx-half), round(cy-half), round(cx+half), round(cy+half))
# Fill its ragged, see-through edge with the colour just inside it.
arr = np.array(src).astype(np.float32)
al = arr[:, :, 3:4] / 255
pre = Image.fromarray((arr[:, :, :3] * al).astype(np.uint8))
wt = Image.fromarray((al[:, :, 0] * 255).astype(np.uint8))
r = 24
fill = np.array(pre.filter(ImageFilter.GaussianBlur(r))).astype(np.float32) / np.maximum(np.array(wt.filter(ImageFilter.GaussianBlur(r))).astype(np.float32)[:, :, None] / 255, 1e-3)
flat = arr[:, :, :3] * al + np.clip(fill, 0, 255) * (1 - al)
full = Image.fromarray(flat.astype(np.uint8)).crop(box).resize((1024, 1024), Image.LANCZOS).convert("RGBA")
full.save(f"{tmp}/full.png")
tile = full.resize((824, 824), Image.LANCZOS)
S = 824 * 4
y, x = np.mgrid[0:S, 0:S].astype(np.float32)
u, v = np.abs((x+.5)/S*2-1), np.abs((y+.5)/S*2-1)
mask = Image.fromarray(((u**5 + v**5) <= 1).astype(np.uint8)*255).resize((824, 824), Image.LANCZOS)
tile.putalpha(mask)
out = Image.new("RGBA", (1024, 1024))
sh = Image.new("RGBA", (1024, 1024))
sh.paste(Image.new("RGBA", (824, 824), (0, 0, 0, 77)), (100, 112), mask)
out = Image.alpha_composite(out, sh.filter(ImageFilter.GaussianBlur(14)))
out.alpha_composite(tile, (100, 100))
out.save(f"{tmp}/tile.png")
PY
cp "$tmp/tile.png" "$brand/rush-icon.png"
for v in tile:AppIcon full:AppIcon-full; do
  set="$tmp/${v#*:}.iconset"
  mkdir "$set"
  for s in 16 32 128 256 512; do
    sips -z $s $s "$tmp/${v%%:*}.png" --out "$set/icon_${s}x${s}.png" >/dev/null
    sips -z $((s*2)) $((s*2)) "$tmp/${v%%:*}.png" --out "$set/icon_${s}x${s}@2x.png" >/dev/null
  done
  iconutil -c icns "$set" -o "$icons/${v#*:}.icns"
done
rm -rf "$tmp"
