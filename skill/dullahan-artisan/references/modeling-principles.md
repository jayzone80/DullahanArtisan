# Universal Low-Poly PBR Modeling Principles

Genre-neutral craft guidelines for turning compact `.mdl` primitives into cohesive, game-ready 3D assets. Scale geometric density to match the user's requested detail level rather than a fixed complexity tier.

---

## 1. Palette & Material Harmony
Because palette-atlas art uses flat-cell UVs with no painted textures, **value contrast and PBR separation carry the visual read**:
- **Author 2–4 step value ramps per material family** (dark / base / light): use dark tones for recesses, shadow gaps, frames, and underbodies; mid tones for primary surfaces; light tones for trim, proud edges, and top accents.
- **Organize `.pal` rows by material family** (surfaces, metals, accents, glass, emissives) and inspect `--sheet` to catch jarring hue/value outliers early.
- **Separate roughness and metallic cleanly**: matte organic/stone surfaces (`r=0.82–0.92`, `m=0`), painted/lacquered parts (`r=0.50–0.70`, `m=0–0.1`), bare metals (`r=0.22–0.42`, `m=0.88–0.98`), glass (`#RRGGBBAA` with `r=0.06–0.14`), and focal lights via the emissive column.

## 2. Silhouette, Depth & Primitive Selection
- **Anchor key dimensions with `$vars` first**: define ground `Y=0`, base height, overall width/depth, and repeating spacing as `$var` constants at the top of `.mdl` so resizing never creates floating or misaligned sub-parts.
- **Break flat planes with proud structural depth**: instead of a single flat slab, layer recessed cores with proud border trim, corner posts, panel seams, fasteners, or surface relief offset outward by `0.015–0.05` units.
- **Catch PBR edge light with chamfers (`bev=`)**: even a subtle 1-segment bevel on `box`, `pri`, and `cyl` turns dull 90° edges into crisp specular rim highlights.
- **Choose the right sculpting primitive**:
  - `pri` (2D polygon extrude): clean recessed cutouts (wheel arches, door/window openings, contoured handles/stocks, shields, blades) without needing mesh booleans.
  - `lat` (Y-revolve): turned round forms (barrels, bottles, wheels/tires with `cap=0` for recessed rims, domes, columns).
  - `lof` + tapered `cap` (+ `sm=1`): organic bodies, limbs, curved tails, draped forms, and smooth tapered silhouettes; use sticky per-section color transitions for clean markings.
  - `tub` / `bend=` / `twist=`: pipes, handles, guard bars, branches, arches, and curved structural parts.
  - `ter` + `ico` (`jit=`, `mats=`): rugged terrain pads, mounded bulk surfaces, rocks, and natural clusters.
- **Factor repeating assemblies**: put multi-part sub-assemblies (wheels, windows, modular tiles, fasteners) inside `def ... end` and instantiate with `use`, `rep=`, `rad=`, `m=x|z|xz`, and `ov=` color overrides.
- **Use translucent glass intentionally**: because translucent triangles do not cast opaque shadows, place simplified interior shapes behind `#RRGGBBAA` glass panes and seal internal cavities (e.g., wheelhouse liners, inner walls) so gaps never expose background light or interior parts from side angles.

## 3. Visual QA Checklist (Always Inspect `--turn` + `--focus`)
1. **No gaps or unintended see-through**: verify side/low angles so glass panes tuck solidly into frames and cutouts have solid backing liners.
2. **Grounding & contact**: check every joint and ground contact at `Y=0` (use `--focus <id>` for small attachments like handles, brackets, and mounts).
3. **No Z-fighting or buried accents**: offset layered surfaces along their normal by at least `0.015` units; avoid capping hollow rings (`cap=0`) when inner recessed detail must stay visible.
4. **No open single-sided holes**: backface culling is always active; pair any open shell with solid rim volume or backing.
5. **Verify orientation & handedness**: positive rotations follow the right-hand rule around `+X`, `+Y`, `+Z`; check directional details (handles, asymmetric stances, driver-side layouts) across all 4 turntable views.
