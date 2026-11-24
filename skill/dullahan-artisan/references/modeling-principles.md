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
  - `csg` (exact BSP mesh booleans): crisp hard-surface cutouts (`sub`), unions (`add`), and intersections (`int`) on solid primitives (carved stone notches, wheel wells, recessed vents, archways) where carved interior faces automatically adopt the cutter's palette swatch.
  - `sdf` / `meta` (Surface-Nets metaballs): seamless organic creatures, animals, characters, roots, and smooth sculpted forms using `sph`, `ell`, and point-to-point tapered `cap <a_xyz> <b_xyz> <r0,r1>` with smooth `k=` blending, `sub` eye/ear hollows, and `paint` volumes for organic fur/skin markings without Z-fighting.
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

---

## 4. Engine Colliders (`col <node_name> ... end`)
Keep visual geometry batched in 1 opaque PBR draw call while exporting simplified, invisible physics shapes as separate child glTF nodes via `col <node_name> ... end` (primitives inside `col` omit the material argument; verify fit with `--colliders` green wireframe overlay):

- **Godot 4**:
  - Whole-mesh static trimesh collider: name the `.mdl` file with `-col` (e.g. `dungeon_wall-col.mdl` → root node `dungeon_wall-col`).
  - Simplified invisible collision shapes alongside a detailed visual mesh: use `col <part>-colonly ... end` (concave trimesh) or `col <part>-convcolonly ... end` (convex hull — preferred for vehicles, props, and rigid bodies).
- **Unreal Engine 5**:
  - Prefix simplified collider nodes with `UBX_<MeshName>_01` (box), `UCX_<MeshName>_01` (convex hull), `USP_<MeshName>_01` (sphere), or `UCP_<MeshName>_01` (capsule), matching the `.mdl` filename stem `<MeshName>`.
- **Unity**:
  - Unity's built-in importer does not parse collider name suffixes automatically (outside its whole-mesh *Generate Colliders* option). Either attach native `BoxCollider`/`CapsuleCollider` components on the prefab, or export `col COL_<part> ... end` child nodes and convert them with a small `AssetPostprocessor` script.
