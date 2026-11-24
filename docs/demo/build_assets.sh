#!/usr/bin/env bash
set -euo pipefail

# Reproducibly compile 100% of the Three.js demo assets using Dullahan Artisan (`artisan`).
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
OUT="$SCRIPT_DIR/assets"
SRC="$SCRIPT_DIR/src"

ARTISAN="${ARTISAN:-artisan}"
if ! command -v "$ARTISAN" >/dev/null 2>&1; then
  ARTISAN="$ROOT_DIR/target/release/artisan"
fi

mkdir -p "$OUT"
echo "Compiling demo assets using: $ARTISAN"

# 1. 400x400m eroded alpine terrain (16-bit heightmap, RGBA biome splatmap & QEM-decimated GLB)
"$ARTISAN" terrain "$SRC/mountain_trail.ter" \
  -o "$OUT/mountain_trail" \
  --mesh "$OUT/mountain_trail.glb" \
  --grid 80 --tris 5600 --skirt 1

# Remove unused auxiliary maps from web runtime assets to keep download lightweight
rm -f "$OUT/mountain_trail_normal.png" "$OUT/mountain_trail_splat.png"

# 2. 3D Models (all sharing examples/palette.pal)
for m in offroad_chassis offroad_wheel mountain_cabin pine_tree boulder_rock bush_foliage grass_tuft wildflowers trail_coin; do
  "$ARTISAN" mesh "$SRC/${m}.mdl" -o "$OUT/${m}.glb"
done

# Reuse our metaball + CSG showcase Red Fox as a scenic landmark Easter egg
"$ARTISAN" mesh "$ROOT_DIR/examples/models/fox_on_stones.mdl" -o "$OUT/fox_on_stones.glb"

# 3. Procedural Audio (seamless loops & one-shots)
"$ARTISAN" sfx "$SRC/engine_loop.sfx" -o "$OUT/engine_loop.wav"
"$ARTISAN" sfx "$SRC/coin_pickup.sfx" -o "$OUT/coin_pickup.wav"
"$ARTISAN" sfx "$SRC/bump_thud.sfx"   -o "$OUT/bump_thud.wav"
"$ARTISAN" sfx "$SRC/car_horn.sfx"    -o "$OUT/car_horn.wav"
"$ARTISAN" sfx "$ROOT_DIR/examples/audio/woodland_ambience.sfx"  -o "$OUT/woodland_ambience.wav"
"$ARTISAN" sfx "$ROOT_DIR/examples/audio/arcade_anthem_loop.sfx" -o "$OUT/arcade_anthem_loop.wav"

# 4. Crisp Game-UI SVG -> RGBA PNG badges
"$ARTISAN" svg "$SRC/ui_crest.svg"     -o "$OUT/ui_crest.png"
"$ARTISAN" svg "$SRC/ui_coin_icon.svg" -o "$OUT/ui_coin_icon.png"

echo "Done! Compiled assets in $OUT:"
du -sh "$OUT"
