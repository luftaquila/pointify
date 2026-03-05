#!/bin/bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
OPENSCAD="/Applications/OpenSCAD.app/Contents/MacOS/OpenSCAD"
SCAD_SRC="$SCRIPT_DIR/pointify.scad"
SCAD_TMP="/tmp/pointify_render.scad"
OUT_DIR="/tmp/pointify_gallery"
GALLERY="$SCRIPT_DIR/gallery.png"

COLORSCHEME_NAME="Pointify Gallery"
COLORSCHEME_PATH="/Applications/OpenSCAD.app/Contents/Resources/color-schemes/render/pointify-gallery.json"

# Install custom colorscheme if missing
if [ ! -f "$COLORSCHEME_PATH" ]; then
  cat > "$COLORSCHEME_PATH" << 'SCHEME'
{
    "name" : "Pointify Gallery",
    "index" : 9999,
    "show-in-gui" : false,
    "colors" : {
        "background" :         "#1e1e2e",
        "axes-color" :         "#e8e8e8",
        "opencsg-face-front" : "#a0a0a0",
        "opencsg-face-back" :  "#a0a0a0",
        "cgal-face-front" :    "#a0a0a0",
        "cgal-face-back" :     "#909090",
        "cgal-face-2d" :       "#a0a0a0",
        "cgal-edge-front" :    "#c5c8c6",
        "cgal-edge-back" :     "#c5c8c6",
        "cgal-edge-2d" :       "#a0a0a0",
        "crosshair" :          "#b294bb"
    }
}
SCHEME
  echo "Installed colorscheme: $COLORSCHEME_PATH"
fi

# Prepare scad without color() for uniform gray
sed 's/color("#ffaa00")//' "$SCAD_SRC" > "$SCAD_TMP"

mkdir -p "$OUT_DIR"

# Render configurations
CONFIGS="1:2 1:3 1:4 1:5 1:6 1:7 2:2 2:3"

for cfg in $CONFIGS; do
  rows="${cfg%%:*}"
  cols="${cfg##*:}"
  name="${rows}x${cols}"
  echo "Rendering $name..."
  "$OPENSCAD" -o "$OUT_DIR/$name.png" \
    "--imgsize=2400,1800" --viewall --autocenter \
    "--camera=0,0,0,55,0,25,0" \
    -D "explode_dist=0" -D "num_rows=$rows" -D "num_cols=$cols" \
    --render "--colorscheme=$COLORSCHEME_NAME" --backend Manifold \
    "$SCAD_TMP" 2>&1 &
done
wait
echo "All renders done"

# Trim and pad
for cfg in $CONFIGS; do
  rows="${cfg%%:*}"
  cols="${cfg##*:}"
  name="${rows}x${cols}"
  magick "$OUT_DIR/$name.png" -trim +repage \
    -gravity center -background "#1e1e2e" -extent 120%x120% \
    "$OUT_DIR/$name.png"
done

# Get max height per row for vertical centering
ROW1_H=0
for name in 1x2 1x3 1x4 1x5; do
  h=$(magick identify -format "%h" "$OUT_DIR/$name.png")
  [ "$h" -gt "$ROW1_H" ] && ROW1_H=$h
done

ROW2_H=0
for name in 1x6 1x7 2x2 2x3; do
  h=$(magick identify -format "%h" "$OUT_DIR/$name.png")
  [ "$h" -gt "$ROW2_H" ] && ROW2_H=$h
done

# Pad each image to row max height (center gravity)
for name in 1x2 1x3 1x4 1x5; do
  magick "$OUT_DIR/$name.png" -gravity center -background "#1e1e2e" \
    -extent "%[w]x${ROW1_H}" "$OUT_DIR/$name.png"
done
for name in 1x6 1x7 2x2 2x3; do
  magick "$OUT_DIR/$name.png" -gravity center -background "#1e1e2e" \
    -extent "%[w]x${ROW2_H}" "$OUT_DIR/$name.png"
done

# Compose 4x2 gallery
magick montage \
  "$OUT_DIR/1x2.png" "$OUT_DIR/1x3.png" "$OUT_DIR/1x4.png" "$OUT_DIR/1x5.png" \
  "$OUT_DIR/1x6.png" "$OUT_DIR/1x7.png" "$OUT_DIR/2x2.png" "$OUT_DIR/2x3.png" \
  -tile 4x2 -geometry +0+0 -background "#1e1e2e" \
  "$GALLERY"

# Cleanup
rm -rf "$OUT_DIR" "$SCAD_TMP"

echo "Gallery created: $GALLERY ($(magick identify -format '%wx%h' "$GALLERY"))"
