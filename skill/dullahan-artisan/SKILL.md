---
name: dullahan-artisan
description: >-
  Workflow, compact DSL reference, and craft principles for Dullahan Artisan
  (`artisan`), a headless cross-platform CLI compiler for shared PBR
  color-palette atlases (.pal), low-poly 3D game models (.mdl -> .glb/.gltf
  + PBR turnaround renders), and procedural game SFX, seamless loops, and
  music (.sfx/.kit -> .wav + spectrogram diagnostics). Use whenever authoring,
  editing, or validating 3D models, PBR palettes, or procedural audio assets.
---

# Dullahan Artisan (`artisan`) — Headless 3D PBR & Procedural Audio Tooling

`artisan` is a single static, zero-dependency native executable (Linux, macOS, Windows) that compiles compact line-oriented plaintext source files into game-ready `.glb`/`.gltf` models, shared 3-map PBR texture atlases, and 16-bit PCM `.wav` audio, with built-in visual and acoustic verification.

Run `artisan spec [pal|mesh|sfx]` at any time to print the authoritative grammar directly from the executable.

---

## Commands

| Task | Command |
| :--- | :--- |
| Build 3 PBR atlases (+ swatch sheet) | `artisan pal <file.pal> [-o <prefix>] [--sheet <png>]` |
| Build 3D model + validate + render | `artisan mesh <file.mdl> [-p <pal>] [-o <out.glb\|.gltf>] [--ext-pal <prefix>] [--render <hero.png>] [--turn <turn.png>] [--focus <id>]` |
| Build audio + validate + plot | `artisan sfx <file.sfx> [-k <kit>] [-o <out.wav>] [--var N] [--plot <diag.png>] [--size WxH] [--seed S]` |
| Standalone validation | `artisan validate <file.glb\|.gltf\|.wav> [--loop]` |
| Print full grammar | `artisan spec [pal\|mesh\|sfx]` |

Useful mesh render flags: `--size WxH`, `--yaw D`, `--pitch D`, `--fill 0.1..0.96`, `--fov D`, `--ortho`, `--wire`, `--hud 0|1`, `--bg #RRGGBB`.

---

## Closed-Loop Authoring Workflow

Adapt visual style, acoustic character, and geometric/sonic detail freely to the user's request—do not impose any default genre or fixed complexity tier. For concise, genre-neutral craft guidelines (value ramps, primitive choice, acoustic layering, and mixing hygiene), consult [references/modeling-principles.md](./references/modeling-principles.md) and [references/sound-principles.md](./references/sound-principles.md).

### 1. Shared PBR Palette (`.pal`)
- Line 1 sets grid layout and tile resolution: `grid <C>x<R>@<tilePx> [bg=#RRGGBB]` (e.g. `grid 8x8@16`).
- Each subsequent line assigns a tile: `<name> <#hex> [rough] [metal] [emissive|-] [ao] [at=c,r]`.
- `#RRGGBBAA` with alpha `< FF` automatically routes affected triangles into an optional 2nd `BLEND` draw call while keeping all opaque surfaces (matte, metallic, emissive) batched in **1 draw call**.
- Outputs `<prefix>_basecolor.png`, `<prefix>_orm.png` (`R=AO, G=Roughness, B=Metallic`), and `<prefix>_emissive.png`. Use `--ext-pal <prefix>` when multiple models should share one external material set in-engine, or omit it to embed atlases directly inside `.glb`/`.gltf`.

### 2. 3D Models (`.mdl`)
- **Coordinate space**: Right-handed Y-up (`+X` right, `+Y` up, `+Z` front, `Y=0` ground), outward CCW winding.
- **Structure**: Reference palette via `pal <rel.pal>`. Define `$var = expr` dimensions at the top to prevent placement drift. Tag key sub-parts with `@id` for targeted edits and `--focus <id>` inspection renders. Factor repeating assemblies into `def <name> ... end` and stamp via `use` (with `ov=old:new`, `m=x|y|z|xz`, `rep=`, `rad=`).
- **Primitives**: `box` (`t=`, `o=`, `bev=`), `cyl` (`rt=0` for cone, `cap=0` for open ring, `bev=`), `cap`, `wdg`, `pyr`, `ico` (`sub=`, watertight `jit=`, `mats=`), `tor`, `pri` (2D polygon extrude + `bev=`), `lat` (Y-axis revolve), `tub` (3D polyline sweep), `lof` (elliptical spine sweep with per-section colors), `ter` (noisy/mounded heightfield with `thk=`), `tri`.
- **Anchors & Deformers**: Centered primitives (`box`, `cyl`, `cap`, `wdg`, `pyr`, `ico`, `tor`) default to `a=c`; explicit-coordinate primitives (`pri`, `lat`, `tub`, `lof`, `ter`) default to `a=o`. Use `a=b` to ground upright parts on `Y`. Add `sm=1` for smooth vertex normals, `bev=` for specular-catching chamfers, and `bend=` / `twist=` for curved forms.
- **Technical Visual QA** (inspect `--render` and `--turn` PNGs):
  - Check all four turnaround angles for silhouette, proportions, and alignment against the ground grid and RGB axis triad.
  - Backface culling is always enabled: any dark hole or missing surface indicates an open single-sided shell or inverted face.
  - Offset overlapping coplanar surfaces by at least `0.015` units along the surface normal to prevent Z-fighting.
  - Confirm `valid=PASS` in CLI telemetry.

### 3. Procedural Audio & Music (`.sfx` / `.kit`)
- **Header**: `sfx <name> [dur=S | bpm=B bars=N bpb=4] [sr=44100] [ch=2] [loop=0|1] [xfade=S] [curve=lin|eqp] [norm=-1] [seed=S]`.
- **Generators & Signal Flow**: Compose leaf generators (`osc`, `ssaw`, `fm`, `mod`, `plk`, `noi`) and reusable `def` voices (local or via `use <kit>`) with `ref`, `grp`, `pat` (step grid), `seq` (notes/chords in beats), and `sct` (seeded scatter). Chain stereo FX left-to-right with `|` (`flt`, `drv`, `crush`, `cho`, `dly`, `rev`, `cmp`, `wid`) and master processing via `bus | ...`.
- **Looping & Variations**:
  - Set `loop=1` for automatic 2-cycle pre-warming (filters, delays, and reverbs wrap smoothly into cycle 1).
  - Use `cl=1` on periodic oscillators/LFOs and `curve=lin` for phase-coherent loops; use `curve=eqp` for uncorrelated noise/ambient beds.
  - Prefer placing shared `rev`/`dly` on enclosing `pat`/`seq`/`bus` tracks rather than inside high-frequency per-hit voice `def`s.
  - Use `--var N` on one-shots to emit deterministic `_01.._NN` variations.
- **Technical Audio QA** (check stdout telemetry + `--plot` PNG):
  - `clip=0`, `dc < 0.02`, clean spectral balance without unintended sub-30 Hz rumble (high-pass master bus).
  - For loops: confirm `seam <= 0.03` (ideal) / `< 0.08` (max) and inspect the ±25 ms Loop Seam Zoom panel for smooth continuity.
