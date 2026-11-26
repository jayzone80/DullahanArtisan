use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use crate::common::expr::ExprEnv;
use crate::common::math::{Aabb, Mat4, Vec2, Vec3};
use crate::common::png::decode_png;
use crate::mesh::anim::{AnimAction, AnimEvent, AnimKeyframe, InterpMode};
use crate::mesh::csg::{apply_csg, CsgOp};
use crate::mesh::geom::{ChildNodeMesh, ColliderMesh, Mesh, TexMaterial};
use crate::mesh::morph::{build_morph_target, MorphKeyframe, MorphOp};
use crate::mesh::palette::{Palette, ResolvedMat};
use crate::mesh::primitives::{
    build_box, build_capsule, build_cylinder, build_icosphere, build_lathe, build_loft,
    build_noisy_plane, build_prism, build_pyramid, build_ribbon, build_torus, build_tube,
    build_wedge, FaceMats, LoftSection,
};
use crate::mesh::sdf::{build_sdf_mesh, SdfOp, SdfPrim, SdfShape};
use crate::mesh::skel::{mirror_bone_name, Bone, IkKeyframe};
use crate::tex::bake::bake_texture;
use crate::tex::parser::load_tex_file;

#[derive(Debug, Clone, Copy)]
struct TexBand {
    tex_id: u16,
    u0: f32,
    v0: f32,
    u1: f32,
    v1: f32,
    /// When true, `u` tiles freely along world length (`tile_w`) while `v` clamps strictly to `[v0..v1]`!
    is_strip: bool,
}

#[derive(Debug, Clone)]
pub struct ParsedModel {
    pub name: String,
    pub palette_rel: Option<String>,
    pub mesh: Mesh,
    pub part_count: usize,
}

#[derive(Debug, Clone)]
struct RawLine {
    lineno: usize,
    tokens: Vec<String>,
}

#[derive(Debug, Clone)]
enum Anchor {
    Center,
    Bottom,
    Top,
    Origin,
}

#[derive(Debug, Clone)]
struct RepeatSpec {
    count: usize,
    step: Vec3,
    rot_step: Vec3,
}

#[derive(Debug, Clone)]
struct RadialSpec {
    count: usize,
    radius: f32,
    axis: char,
    start_deg: f32,
}

#[derive(Debug, Clone)]
struct BendSpec {
    deg: f32,
    along: char,
    toward: char,
}

#[derive(Debug, Clone)]
struct TwistSpec {
    deg: f32,
    axis: char,
}

#[derive(Debug, Clone)]
struct CommonFlags {
    rot: Vec3,
    scale: Vec3,
    anchor: Option<Anchor>,
    mirror: Vec<char>,
    smooth: bool,
    weighted_normals: Option<f32>,
    bone: Option<String>,
    bend: Option<BendSpec>,
    twist: Option<TwistSpec>,
    repeat: Option<RepeatSpec>,
    radial: Option<RadialSpec>,
    face_colors: Vec<(String, String)>,
    color_overrides: HashMap<String, String>,
    extras: HashMap<String, String>,
}

impl Default for CommonFlags {
    fn default() -> Self {
        Self {
            rot: Vec3::ZERO,
            scale: Vec3::ONE,
            anchor: None,
            mirror: Vec::new(),
            smooth: false,
            weighted_normals: None,
            bone: None,
            bend: None,
            twist: None,
            repeat: None,
            radial: None,
            face_colors: Vec::new(),
            color_overrides: HashMap::new(),
            extras: HashMap::new(),
        }
    }
}

/// Extract the `pal <path>` directive (if any) from a `.mdl` file before full compilation.
pub fn peek_palette_path(mdl_text: &str) -> Option<String> {
    for line in mdl_text.lines() {
        let s = strip_mdl_comment(line).trim();
        if s.is_empty() {
            continue;
        }
        let mut it = s.split_whitespace();
        if let Some(op) = it.next() {
            if op.eq_ignore_ascii_case("pal") {
                return it.next().map(|p| p.trim_matches('"').to_string());
            }
        }
    }
    None
}

pub fn load_and_compile_model(
    mdl_path: &Path,
    palette_override: Option<&Path>,
) -> Result<(ParsedModel, Palette, PathBuf), String> {
    let empty = HashMap::new();
    load_and_compile_model_with_overrides(mdl_path, palette_override, &empty)
}

pub fn load_and_compile_model_with_overrides(
    mdl_path: &Path,
    palette_override: Option<&Path>,
    cli_overrides: &HashMap<String, String>,
) -> Result<(ParsedModel, Palette, PathBuf), String> {
    let text = std::fs::read_to_string(mdl_path)
        .map_err(|e| format!("failed to read model '{}': {e}", mdl_path.display()))?;
    let base_dir = mdl_path.parent().unwrap_or_else(|| Path::new("."));
    let pal_rel = peek_palette_path(&text);
    let pal_path = if let Some(ov) = palette_override {
        ov.to_path_buf()
    } else if let Some(ref rel) = pal_rel {
        base_dir.join(rel)
    } else {
        return Err(format!(
            "model '{}' has no 'pal <path>' directive and no -p palette flag was provided",
            mdl_path.display()
        ));
    };
    let palette = Palette::load_file(&pal_path)?;
    // Validate CLI material override targets early so typos fail with a clear error
    for (old_m, new_m) in cli_overrides {
        palette
            .resolve_mat(new_m)
            .map_err(|e| format!("--ov {old_m}:{new_m}: {e}"))?;
    }
    let name = mdl_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let parsed = compile_model_with_dir(&text, &name, &palette, Some(base_dir), cli_overrides)?;
    Ok((parsed, palette, pal_path))
}

pub fn compile_model(text: &str, name: &str, palette: &Palette) -> Result<ParsedModel, String> {
    let empty = HashMap::new();
    compile_model_with_dir(text, name, palette, None, &empty)
}

fn is_kit_include_line(tokens: &[String]) -> Option<&str> {
    if tokens.len() < 2 {
        return None;
    }
    let op = tokens[0].to_ascii_lowercase();
    if op == "inc" || op == "include" {
        return Some(tokens[1].as_str());
    }
    if op == "use" && tokens.len() == 2 && tokens[1].to_ascii_lowercase().ends_with(".mdl") {
        return Some(tokens[1].as_str());
    }
    None
}

fn load_mdl_kit_recursive(
    kit_path: &Path,
    env: &mut ExprEnv,
    defs: &mut HashMap<String, Vec<RawLine>>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let canon = std::fs::canonicalize(kit_path).unwrap_or_else(|_| kit_path.to_path_buf());
    if !visited.insert(canon) {
        return Ok(());
    }
    let text = std::fs::read_to_string(kit_path)
        .map_err(|e| format!("failed to read included model kit '{}': {e}", kit_path.display()))?;
    let kit_dir = kit_path.parent().unwrap_or_else(|| Path::new("."));

    let mut raw_lines = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let clean = strip_mdl_comment(line).trim();
        if clean.is_empty() {
            continue;
        }
        let tokens = tokenize_line(clean);
        if tokens.is_empty() {
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("pal") {
            continue;
        }
        if let Some(rel) = is_kit_include_line(&tokens) {
            let sub_path = kit_dir.join(rel);
            load_mdl_kit_recursive(&sub_path, env, defs, visited)?;
            continue;
        }
        if tokens[0].starts_with('$') {
            parse_var_line(&tokens, env, lineno)
                .map_err(|e| format!("kit '{}': {e}", kit_path.display()))?;
            continue;
        }
        raw_lines.push(RawLine { lineno, tokens });
    }

    let mut i = 0usize;
    while i < raw_lines.len() {
        let op = raw_lines[i].tokens[0].to_ascii_lowercase();
        if op == "def" {
            let lineno = raw_lines[i].lineno;
            if raw_lines[i].tokens.len() < 2 {
                return Err(format!(
                    "kit '{}' line {lineno}: 'def' requires a template name",
                    kit_path.display()
                ));
            }
            let def_name = raw_lines[i].tokens[1].clone();
            let (body, next_i) = collect_block(&raw_lines, i + 1, lineno, "def")
                .map_err(|e| format!("kit '{}': {e}", kit_path.display()))?;
            defs.insert(def_name, body);
            i = next_i;
        } else if matches!(op.as_str(), "grp" | "col" | "node" | "anim" | "action" | "skel" | "rig" | "morph" | "shape" | "csg" | "sdf" | "meta") {
            // Skip any top-level preview blocks inside a kit file
            let (_body, next_i) = collect_block(&raw_lines, i + 1, raw_lines[i].lineno, &op)
                .map_err(|e| format!("kit '{}': {e}", kit_path.display()))?;
            i = next_i;
        } else {
            i += 1;
        }
    }
    Ok(())
}

pub fn compile_model_with_dir(
    text: &str,
    name: &str,
    palette: &Palette,
    base_dir: Option<&Path>,
    cli_overrides: &HashMap<String, String>,
) -> Result<ParsedModel, String> {
    let mut env = ExprEnv::new();
    let mut defs: HashMap<String, Vec<RawLine>> = HashMap::new();
    let mut visited_kits: HashSet<PathBuf> = HashSet::new();
    let mut raw_lines = Vec::new();
    let mut pal_rel = None;
    let mut tex_decls: Vec<RawLine> = Vec::new();
    let mut band_decls: Vec<RawLine> = Vec::new();

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let clean = strip_mdl_comment(line).trim();
        if clean.is_empty() {
            continue;
        }
        let tokens = tokenize_line(clean);
        if tokens.is_empty() {
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("pal") {
            if let Some(p) = tokens.get(1) {
                pal_rel = Some(p.clone());
            }
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("tex") || tokens[0].eq_ignore_ascii_case("texture") {
            tex_decls.push(RawLine { lineno, tokens });
            continue;
        }
        if tokens[0].eq_ignore_ascii_case("band") || tokens[0].eq_ignore_ascii_case("trim") {
            band_decls.push(RawLine { lineno, tokens });
            continue;
        }
        if let Some(rel) = is_kit_include_line(&tokens) {
            let kit_path = base_dir
                .map(|d| d.join(rel))
                .unwrap_or_else(|| PathBuf::from(rel));
            load_mdl_kit_recursive(&kit_path, &mut env, &mut defs, &mut visited_kits)?;
            continue;
        }
        // Allow one or multiple `$var=expr` assignments on a single line
        if tokens[0].starts_with('$') {
            parse_var_line(&tokens, &mut env, lineno)?;
            continue;
        }
        raw_lines.push(RawLine { lineno, tokens });
    }

    // Separate top-level `def`, `skel`/`rig`, `morph`/`shape`, and root statements
    let mut root_lines = Vec::new();
    let mut skel_blocks: Vec<Vec<RawLine>> = Vec::new();
    let mut morph_blocks: Vec<(RawLine, Vec<RawLine>)> = Vec::new();
    let mut i = 0usize;
    while i < raw_lines.len() {
        let op = raw_lines[i].tokens[0].to_ascii_lowercase();
        if op == "def" {
            let lineno = raw_lines[i].lineno;
            if raw_lines[i].tokens.len() < 2 {
                return Err(format!("line {lineno}: 'def' requires a template name"));
            }
            let def_name = raw_lines[i].tokens[1].clone();
            let (body, next_i) = collect_block(&raw_lines, i + 1, lineno, "def")?;
            defs.insert(def_name, body);
            i = next_i;
        } else if op == "skel" || op == "rig" {
            let lineno = raw_lines[i].lineno;
            let (body, next_i) = collect_block(&raw_lines, i + 1, lineno, &op)?;
            skel_blocks.push(body);
            i = next_i;
        } else if op == "morph" || op == "shape" {
            let lineno = raw_lines[i].lineno;
            let (body, next_i) = collect_block(&raw_lines, i + 1, lineno, &op)?;
            morph_blocks.push((raw_lines[i].clone(), body));
            i = next_i;
        } else {
            root_lines.push(raw_lines[i].clone());
            i += 1;
        }
    }

    let mut bones: Vec<Bone> = Vec::new();
    let mut bone_map: HashMap<String, u16> = HashMap::new();
    for sbody in &skel_blocks {
        for bl in sbody {
            let bop = bl.tokens[0].to_ascii_lowercase();
            if bop.starts_with('$') {
                parse_var_line(&bl.tokens, &mut env, bl.lineno)?;
                continue;
            }
            if bop != "bone" && bop != "jnt" && bop != "joint" {
                return Err(format!(
                    "line {}: unknown statement '{}' inside skel block (use bone)",
                    bl.lineno, bl.tokens[0]
                ));
            }
            if bl.tokens.len() < 5 {
                return Err(format!(
                    "line {}: bone requires <name> <parent|-> <head_x,y,z> <tail_x,y,z> [r=...] [falloff=...] [m=x]",
                    bl.lineno
                ));
            }
            let bname = bl.tokens[1].strip_prefix('@').unwrap_or(&bl.tokens[1]).to_string();
            let parent_tok = bl.tokens[2].strip_prefix('@').unwrap_or(&bl.tokens[2]);
            let parent_idx = if parent_tok == "-" || parent_tok.eq_ignore_ascii_case("none") || parent_tok.eq_ignore_ascii_case("root") {
                None
            } else {
                Some(*bone_map.get(parent_tok).ok_or_else(|| {
                    format!("line {}: bone '{}' references unknown parent '{}'", bl.lineno, bname, parent_tok)
                })? as usize)
            };
            let hv = env.eval_list_f32(&bl.tokens[3]).map_err(|e| format!("line {}: {e}", bl.lineno))?;
            let tv = env.eval_list_f32(&bl.tokens[4]).map_err(|e| format!("line {}: {e}", bl.lineno))?;
            if hv.len() != 3 || tv.len() != 3 {
                return Err(format!("line {}: bone head & tail must each be x,y,z", bl.lineno));
            }
            let head = Vec3::new(hv[0], hv[1], hv[2]);
            let tail = Vec3::new(tv[0], tv[1], tv[2]);
            let blen = (tail - head).len().max(0.02);
            let mut radius = (blen * 0.35).max(0.07);
            let mut falloff = 3.0f32;
            let mut mirror_x = false;
            for tok in &bl.tokens[5..] {
                if let Some((k, v)) = tok.split_once('=') {
                    match k.to_ascii_lowercase().as_str() {
                        "r" | "rad" | "radius" => {
                            radius = env.eval_f32(v).map_err(|e| format!("line {}: {e}", bl.lineno))?.max(0.01);
                        }
                        "f" | "falloff" => {
                            falloff = env.eval_f32(v).map_err(|e| format!("line {}: {e}", bl.lineno))?.clamp(1.0, 10.0);
                        }
                        "m" | "mirror" => {
                            mirror_x = v.to_ascii_lowercase().contains('x');
                        }
                        _ => {}
                    }
                }
            }
            let idx = bones.len() as u16;
            bone_map.insert(bname.clone(), idx);
            bones.push(Bone {
                name: bname.clone(),
                parent: parent_idx,
                head,
                tail,
                radius,
                falloff,
            });
            if mirror_x {
                let m_name = mirror_bone_name(&bname);
                let m_parent = if let Some(pi) = parent_idx {
                    let p_mir = mirror_bone_name(&bones[pi].name);
                    bone_map.get(&p_mir).map(|&id| id as usize).or(Some(pi))
                } else {
                    None
                };
                let m_idx = bones.len() as u16;
                bone_map.insert(m_name.clone(), m_idx);
                bones.push(Bone {
                    name: m_name,
                    parent: m_parent,
                    head: Vec3::new(-head.x, head.y, head.z),
                    tail: Vec3::new(-tail.x, tail.y, tail.z),
                    radius,
                    falloff,
                });
            }
        }
    }

    // Load declared PBR texture sets (`tex <name> <path.tex|png_prefix> [tile=w,h] [res=N]`)
    let mut textures: Vec<TexMaterial> = Vec::new();
    let mut tex_map: HashMap<String, u16> = HashMap::new();
    for tl in &tex_decls {
        if tl.tokens.len() < 3 {
            return Err(format!(
                "line {}: tex requires <name> <path.tex|png_prefix> [tile=w,h] [res=N]",
                tl.lineno
            ));
        }
        let tname = tl.tokens[1].clone();
        let rel_path = &tl.tokens[2];
        let mut tile_override: Option<(f32, f32)> = None;
        let mut res_override: Option<u32> = None;
        for tok in &tl.tokens[3..] {
            if let Some((k, v)) = tok.split_once('=') {
                match k.to_ascii_lowercase().as_str() {
                    "tile" | "world" => {
                        let vals = env.eval_list_f32(v).map_err(|e| format!("line {}: {e}", tl.lineno))?;
                        match vals.as_slice() {
                            [s] => tile_override = Some((*s, *s)),
                            [w, h] => tile_override = Some((*w, *h)),
                            _ => return Err(format!("line {}: bad tile='{v}'", tl.lineno)),
                        }
                    }
                    "res" | "size" => {
                        res_override = Some(
                            v.parse::<u32>()
                                .map_err(|_| format!("line {}: bad res='{v}'", tl.lineno))?
                                .max(32),
                        );
                    }
                    _ => {}
                }
            }
        }
        let full_path = base_dir
            .map(|d| d.join(rel_path))
            .unwrap_or_else(|| PathBuf::from(rel_path));
        let mut tm = load_tex_material_for_mdl(&tname, &full_path, res_override)
            .map_err(|e| format!("line {}: {e}", tl.lineno))?;
        if let Some((tw, th)) = tile_override {
            tm.tile_w = tw.max(0.05);
            tm.tile_h = th.max(0.05);
        }
        let tid = textures.len() as u16;
        tex_map.insert(tname.to_ascii_lowercase(), tid);
        textures.push(tm);
    }

    // Parse named trim bands (`band <name> <tex_name> <v0..v1|u0,v0,u1,v1>`)
    let mut band_map: HashMap<String, TexBand> = HashMap::new();
    for bl in &band_decls {
        if bl.tokens.len() < 4 {
            return Err(format!(
                "line {}: band requires <name> <tex_name> <v0..v1|u0,v0,u1,v1>",
                bl.lineno
            ));
        }
        let bname = bl.tokens[1].to_ascii_lowercase();
        let tkey = bl.tokens[2].to_ascii_lowercase();
        let Some(&tex_id) = tex_map.get(&tkey) else {
            return Err(format!(
                "line {}: band '{}' references unknown tex '{}' (declare with `tex {} ...` first)",
                bl.lineno, bl.tokens[1], bl.tokens[2], bl.tokens[2]
            ));
        };
        let range_tok = &bl.tokens[3];
        let band = if let Some((a, b)) = range_tok.split_once("..") {
            let v0 = env.eval_f32(a.trim()).map_err(|e| format!("line {}: {e}", bl.lineno))?;
            let v1 = env.eval_f32(b.trim()).map_err(|e| format!("line {}: {e}", bl.lineno))?;
            TexBand { tex_id, u0: 0.0, v0, u1: 1.0, v1, is_strip: true }
        } else {
            let vals = env.eval_list_f32(range_tok).map_err(|e| format!("line {}: {e}", bl.lineno))?;
            if vals.len() != 4 {
                return Err(format!(
                    "line {}: band expects v0..v1 or u0,v0,u1,v1, got '{range_tok}'",
                    bl.lineno
                ));
            }
            TexBand {
                tex_id,
                u0: vals[0],
                v0: vals[1],
                u1: vals[2],
                v1: vals[3],
                is_strip: false,
            }
        };
        band_map.insert(bname, band);
    }

    let mut ctx = EvalCtx {
        palette,
        defs: &defs,
        cli_overrides,
        bones: &bones,
        bone_map: &bone_map,
        textures: &textures,
        tex_map: &tex_map,
        band_map: &band_map,
        env,
        part_count: 0,
    };
    let overrides = HashMap::new();
    let mut mesh = ctx.eval_lines(&root_lines, None, &overrides, 0)?;
    let part_count = ctx.part_count;
    let mut final_env = ctx.env;
    mesh.bones = bones;
    mesh.textures = textures;

    // Evaluate `morph <name> [w=0] ... end` blend shapes against the compiled root mesh!
    for (hdr, mbody) in &morph_blocks {
        let lineno = hdr.lineno;
        if hdr.tokens.len() < 2 {
            return Err(format!("line {lineno}: '{}' requires a morph target name", hdr.tokens[0]));
        }
        let mname = hdr.tokens[1].strip_prefix('@').unwrap_or(&hdr.tokens[1]).to_string();
        let mut default_w = 0.0f32;
        for tok in &hdr.tokens[2..] {
            if let Some((k, v)) = tok.split_once('=') {
                if matches!(k.to_ascii_lowercase().as_str(), "w" | "weight") {
                    default_w = final_env
                        .eval_f32(v)
                        .map_err(|e| format!("line {lineno}: {e}"))?
                        .clamp(0.0, 2.0);
                }
            }
        }
        let mut ops = Vec::new();
        for ml in mbody {
            let mop = ml.tokens[0].to_ascii_lowercase();
            if mop.starts_with('$') {
                parse_var_line(&ml.tokens, &mut final_env, ml.lineno)?;
                continue;
            }
            let eval_v3 = |s: &str, env: &ExprEnv, lno: usize| -> Result<Vec3, String> {
                let v = env.eval_list_f32(s).map_err(|e| format!("line {lno}: {e}"))?;
                if v.len() != 3 {
                    return Err(format!("line {lno}: expected x,y,z in '{s}'"));
                }
                Ok(Vec3::new(v[0], v[1], v[2]))
            };
            let eval_v3_or_s = |s: &str, env: &ExprEnv, lno: usize| -> Result<Vec3, String> {
                let v = env.eval_list_f32(s).map_err(|e| format!("line {lno}: {e}"))?;
                match v.as_slice() {
                    [u] => Ok(Vec3::splat(*u)),
                    [x, y, z] => Ok(Vec3::new(*x, *y, *z)),
                    _ => Err(format!("line {lno}: expected scalar or x,y,z in '{s}'")),
                }
            };
            let parse_tag_tok = |s: &str| -> Option<String> {
                if s.eq_ignore_ascii_case("all") || s == "*" {
                    None
                } else {
                    Some(s.strip_prefix('@').unwrap_or(s).to_string())
                }
            };

            match mop.as_str() {
                "scale" | "s" => {
                    if ml.tokens.len() < 3 {
                        return Err(format!("line {}: morph scale requires <@tag|all> <u|sx,sy,sz> [at=x,y,z] [r=rad]", ml.lineno));
                    }
                    let tag = parse_tag_tok(&ml.tokens[1]);
                    let sc = eval_v3_or_s(&ml.tokens[2], &final_env, ml.lineno)?;
                    let mut pivot = None;
                    let mut radius = None;
                    for tok in &ml.tokens[3..] {
                        if let Some((k, v)) = tok.split_once('=') {
                            match k.to_ascii_lowercase().as_str() {
                                "at" | "p" | "pivot" => pivot = Some(eval_v3(v, &final_env, ml.lineno)?),
                                "r" | "rad" | "radius" => {
                                    radius = Some(final_env.eval_f32(v).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                                }
                                _ => {}
                            }
                        }
                    }
                    ops.push(MorphOp::Scale { tag, scale: sc, pivot, radius });
                }
                "move" | "shift" => {
                    if ml.tokens.len() < 3 {
                        return Err(format!("line {}: morph move requires <@tag|all> <dx,dy,dz> [at=x,y,z] [r=rad] [m=x]", ml.lineno));
                    }
                    let tag = parse_tag_tok(&ml.tokens[1]);
                    let delta = eval_v3(&ml.tokens[2], &final_env, ml.lineno)?;
                    let mut center = None;
                    let mut radius = None;
                    let mut mirror_x = false;
                    for tok in &ml.tokens[3..] {
                        if let Some((k, v)) = tok.split_once('=') {
                            match k.to_ascii_lowercase().as_str() {
                                "at" | "p" | "c" => center = Some(eval_v3(v, &final_env, ml.lineno)?),
                                "r" | "rad" | "radius" => {
                                    radius = Some(final_env.eval_f32(v).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                                }
                                "m" | "mirror" => mirror_x = v.to_ascii_lowercase().contains('x'),
                                _ => {}
                            }
                        }
                    }
                    ops.push(MorphOp::Move { tag, delta, center, radius, mirror_x });
                }
                "pull" => {
                    // pull [@tag] <cx,cy,cz> r=<rad> d=<dx,dy,dz> [tag=@part] [m=x]
                    if ml.tokens.len() < 3 {
                        return Err(format!("line {}: morph pull requires [@tag] <cx,cy,cz> r=<rad> d=<dx,dy,dz> [m=x]", ml.lineno));
                    }
                    let mut idx = 1usize;
                    let mut tag = if ml.tokens[idx].starts_with('@') {
                        let t = parse_tag_tok(&ml.tokens[idx]);
                        idx += 1;
                        t
                    } else {
                        None
                    };
                    if idx >= ml.tokens.len() {
                        return Err(format!("line {}: morph pull requires <cx,cy,cz>", ml.lineno));
                    }
                    let center = Some(eval_v3(&ml.tokens[idx], &final_env, ml.lineno)?);
                    let mut radius = None;
                    let mut delta = None;
                    let mut mirror_x = false;
                    for tok in &ml.tokens[idx + 1..] {
                        if let Some((k, v)) = tok.split_once('=') {
                            match k.to_ascii_lowercase().as_str() {
                                "r" | "rad" | "radius" => {
                                    radius = Some(final_env.eval_f32(v).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                                }
                                "d" | "delta" | "v" => delta = Some(eval_v3(v, &final_env, ml.lineno)?),
                                "tag" | "part" => tag = parse_tag_tok(v),
                                "m" | "mirror" => mirror_x = v.to_ascii_lowercase().contains('x'),
                                _ => {}
                            }
                        }
                    }
                    let d = delta.ok_or_else(|| format!("line {}: morph pull requires d=dx,dy,dz", ml.lineno))?;
                    ops.push(MorphOp::Move { tag, delta: d, center, radius, mirror_x });
                }
                "rot" | "rotate" => {
                    if ml.tokens.len() < 3 {
                        return Err(format!("line {}: morph rot requires <@tag|all> <rx,ry,rz> [at=x,y,z] [m=x]", ml.lineno));
                    }
                    let tag = parse_tag_tok(&ml.tokens[1]);
                    let rot_deg = eval_v3(&ml.tokens[2], &final_env, ml.lineno)?;
                    let mut pivot = None;
                    let mut mirror_x = false;
                    for tok in &ml.tokens[3..] {
                        if let Some((k, v)) = tok.split_once('=') {
                            match k.to_ascii_lowercase().as_str() {
                                "at" | "p" | "pivot" => pivot = Some(eval_v3(v, &final_env, ml.lineno)?),
                                "m" | "mirror" => mirror_x = v.to_ascii_lowercase().contains('x'),
                                _ => {}
                            }
                        }
                    }
                    ops.push(MorphOp::Rotate { tag, rot_deg, pivot, mirror_x });
                }
                "inflate" | "push" | "bulge" => {
                    if ml.tokens.len() < 3 {
                        return Err(format!("line {}: morph inflate requires <@tag|all|cx,cy,cz> [r=rad] amt=<val>", ml.lineno));
                    }
                    let (mut tag, mut center) = if ml.tokens[1].contains(',') {
                        (None, Some(eval_v3(&ml.tokens[1], &final_env, ml.lineno)?))
                    } else {
                        (parse_tag_tok(&ml.tokens[1]), None)
                    };
                    let mut radius = None;
                    let mut amount: Option<f32> = None;
                    let mut mirror_x = false;
                    for tok in &ml.tokens[2..] {
                        if let Some((k, v)) = tok.split_once('=') {
                            match k.to_ascii_lowercase().as_str() {
                                "r" | "rad" | "radius" => {
                                    radius = Some(final_env.eval_f32(v).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                                }
                                "amt" | "a" | "d" => {
                                    amount = Some(final_env.eval_f32(v).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                                }
                                "at" | "c" => center = Some(eval_v3(v, &final_env, ml.lineno)?),
                                "tag" | "part" => tag = parse_tag_tok(v),
                                "m" | "mirror" => mirror_x = v.to_ascii_lowercase().contains('x'),
                                _ => {}
                            }
                        } else if tok.contains(',') && center.is_none() {
                            center = Some(eval_v3(tok, &final_env, ml.lineno)?);
                        } else if amount.is_none() {
                            amount = Some(final_env.eval_f32(tok).map_err(|e| format!("line {}: {e}", ml.lineno))?);
                        }
                    }
                    let amt = amount.ok_or_else(|| format!("line {}: morph inflate requires amt=<delta>", ml.lineno))?;
                    ops.push(MorphOp::Inflate { tag, amount: amt, center, radius, mirror_x });
                }
                _ => {
                    return Err(format!(
                        "line {}: unknown op '{}' inside morph block (use scale|move|pull|rot|inflate)",
                        ml.lineno, ml.tokens[0]
                    ));
                }
            }
        }
        let target = build_morph_target(&mesh, mname, default_w, &ops)?;
        mesh.morphs.push(target);
    }

    Ok(ParsedModel {
        name: name.to_string(),
        palette_rel: pal_rel,
        mesh,
        part_count,
    })
}

struct EvalCtx<'a> {
    palette: &'a Palette,
    defs: &'a HashMap<String, Vec<RawLine>>,
    cli_overrides: &'a HashMap<String, String>,
    bones: &'a [Bone],
    bone_map: &'a HashMap<String, u16>,
    textures: &'a [TexMaterial],
    tex_map: &'a HashMap<String, u16>,
    band_map: &'a HashMap<String, TexBand>,
    env: ExprEnv,
    part_count: usize,
}

impl<'a> EvalCtx<'a> {
    /// Check if `raw` matches a declared `tex` or `band` name (returns `(tex_id, optional_band)`).
    fn resolve_tex_or_band(&self, raw: &str, overrides: &HashMap<String, String>) -> Option<(u16, Option<TexBand>)> {
        let local_mapped = overrides
            .get(raw)
            .or_else(|| overrides.get(&raw.to_ascii_lowercase()))
            .map(|s| s.as_str())
            .unwrap_or(raw);
        let final_mapped = self
            .cli_overrides
            .get(local_mapped)
            .or_else(|| self.cli_overrides.get(&local_mapped.to_ascii_lowercase()))
            .map(|s| s.as_str())
            .unwrap_or(local_mapped);
        let stripped = final_mapped
            .strip_prefix("tex:")
            .or_else(|| final_mapped.strip_prefix("band:"))
            .unwrap_or(final_mapped);
        let key = stripped.to_ascii_lowercase();
        if let Some(&b) = self.band_map.get(&key) {
            return Some((b.tex_id, Some(b)));
        }
        if let Some(&tid) = self.tex_map.get(&key) {
            return Some((tid, None));
        }
        None
    }

    fn resolve_mat_name(
        &self,
        raw: &str,
        overrides: &HashMap<String, String>,
        lineno: usize,
    ) -> Result<ResolvedMat, String> {
        let local_mapped = overrides
            .get(raw)
            .or_else(|| overrides.get(&raw.to_ascii_lowercase()))
            .map(|s| s.as_str())
            .unwrap_or(raw);
        let final_mapped = self
            .cli_overrides
            .get(local_mapped)
            .or_else(|| self.cli_overrides.get(&local_mapped.to_ascii_lowercase()))
            .map(|s| s.as_str())
            .unwrap_or(local_mapped);
        self.palette
            .resolve_mat(final_mapped)
            .map_err(|e| format!("line {lineno}: {e}"))
    }

    fn eval_lines(
        &mut self,
        lines: &[RawLine],
        inherited_mat: Option<&str>,
        overrides: &HashMap<String, String>,
        depth: usize,
    ) -> Result<Mesh, String> {
        if depth > 24 {
            return Err("maximum recursion depth (24) exceeded in def/use or grp".into());
        }
        let mut out = Mesh::new();
        let mut i = 0usize;
        while i < lines.len() {
            let line = &lines[i];
            let lineno = line.lineno;
            let op = line.tokens[0].to_ascii_lowercase();

            if op.starts_with('$') {
                parse_var_line(&line.tokens, &mut self.env, lineno)?;
                i += 1;
                continue;
            }

            if op == "anim" || op == "action" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, &op)?;
                if line.tokens.len() < 2 {
                    return Err(format!("line {lineno}: '{op}' requires an action name (e.g. anim bounce dur=0.8 loop=1)"));
                }
                let action_name = line.tokens[1]
                    .strip_prefix('@')
                    .unwrap_or(&line.tokens[1])
                    .to_string();
                let mut explicit_dur: Option<f32> = None;
                let mut fps: f32 = 24.0;
                let mut looping = false;
                for tok in &line.tokens[2..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "dur" | "duration" | "d" => {
                                explicit_dur = Some(
                                    self.env
                                        .eval_f32(v)
                                        .map_err(|e| format!("line {lineno}: {e}"))?
                                        .max(0.01),
                                );
                            }
                            "fps" => {
                                fps = self
                                    .env
                                    .eval_f32(v)
                                    .map_err(|e| format!("line {lineno}: {e}"))?
                                    .clamp(1.0, 240.0);
                            }
                            "loop" => {
                                looping = v == "1" || v.eq_ignore_ascii_case("true");
                            }
                            _ => {}
                        }
                    }
                }
                let mut keyframes = Vec::new();
                let mut ik_keys = Vec::new();
                let mut morph_keys = Vec::new();
                let mut events = Vec::new();
                let mut max_t = 0.0f32;
                let parse_time = |tok: &str, env: &ExprEnv, lno: usize| -> Result<f32, String> {
                    let s = tok.trim();
                    if let Some(fstr) = s.strip_suffix('f').or_else(|| s.strip_suffix('F')) {
                        let fnum = env.eval_f32(fstr).map_err(|e| format!("line {lno}: {e}"))?;
                        Ok((fnum / fps).max(0.0))
                    } else {
                        let sec = env.eval_f32(s).map_err(|e| format!("line {lno}: {e}"))?;
                        Ok(sec.max(0.0))
                    }
                };
                for al in &body {
                    let aop = al.tokens[0].to_ascii_lowercase();
                    if aop.starts_with('$') {
                        parse_var_line(&al.tokens, &mut self.env, al.lineno)?;
                        continue;
                    }
                    if aop == "key" || aop == "k" || aop == "kf" {
                        if al.tokens.len() < 3 {
                            return Err(format!(
                                "line {}: key requires <time|Nf> <node|bone> [p=...] [dp=...] [r=...] [s=...] [i=...]",
                                al.lineno
                            ));
                        }
                        let time = parse_time(&al.tokens[1], &self.env, al.lineno)?;
                        max_t = max_t.max(time);
                        let target = al.tokens[2]
                            .strip_prefix('@')
                            .unwrap_or(&al.tokens[2])
                            .to_string();
                        let mut pos = None;
                        let mut rot_deg = None;
                        let mut scale = None;
                        let mut interp = InterpMode::Linear;
                        for tok in &al.tokens[3..] {
                            if let Some((k, v)) = tok.split_once('=') {
                                match k.to_ascii_lowercase().as_str() {
                                    "p" | "pos" | "t" => {
                                        pos = Some(self.parse_vec3(v, al.lineno)?);
                                    }
                                    "dp" | "dpos" | "dt" => {
                                        let delta = self.parse_vec3(v, al.lineno)?;
                                        let rest = self
                                            .bone_map
                                            .get(&target)
                                            .map(|&bi| self.bones[bi as usize].rest_local_translation(self.bones))
                                            .unwrap_or(Vec3::ZERO);
                                        pos = Some(rest + delta);
                                    }
                                    "r" | "rot" => {
                                        rot_deg = Some(self.parse_vec3(v, al.lineno)?);
                                    }
                                    "s" | "scale" => {
                                        scale = Some(self.parse_vec3_or_scalar(v, al.lineno)?);
                                    }
                                    "i" | "interp" | "ease" => {
                                        interp = InterpMode::parse(v).ok_or_else(|| {
                                            format!(
                                                "line {}: unknown interp '{v}' (use step|lin|ease|easein|easeout|cubic)",
                                                al.lineno
                                            )
                                        })?;
                                    }
                                    _ => {
                                        return Err(format!(
                                            "line {}: unknown key flag '{tok}'",
                                            al.lineno
                                        ));
                                    }
                                }
                            }
                        }
                        keyframes.push(AnimKeyframe {
                            time,
                            target,
                            pos,
                            rot_deg,
                            scale,
                            interp,
                        });
                    } else if aop == "ik" {
                        if al.tokens.len() < 4 {
                            return Err(format!(
                                "line {}: ik requires <time|Nf> <effector_bone> p=x,y,z [pole=x,y,z] [chain=2] [tip=0|1] [i=...]",
                                al.lineno
                            ));
                        }
                        let time = parse_time(&al.tokens[1], &self.env, al.lineno)?;
                        max_t = max_t.max(time);
                        let effector = al.tokens[2]
                            .strip_prefix('@')
                            .unwrap_or(&al.tokens[2])
                            .to_string();
                        if !self.bone_map.contains_key(&effector) {
                            return Err(format!(
                                "line {}: ik references unknown bone '{}' (define it inside `skel ... end`)",
                                al.lineno, effector
                            ));
                        }
                        let mut target_pt = None;
                        let mut pole_pt = None;
                        let mut chain_len = 2usize;
                        let mut use_tip = false;
                        let mut interp = InterpMode::Linear;
                        for tok in &al.tokens[3..] {
                            if let Some((k, v)) = tok.split_once('=') {
                                match k.to_ascii_lowercase().as_str() {
                                    "p" | "pos" | "t" | "target" => {
                                        target_pt = Some(self.parse_vec3(v, al.lineno)?);
                                    }
                                    "pole" => {
                                        pole_pt = Some(self.parse_vec3(v, al.lineno)?);
                                    }
                                    "chain" | "len" | "n" => {
                                        chain_len = self
                                            .env
                                            .eval_f32(v)
                                            .map_err(|e| format!("line {}: {e}", al.lineno))?
                                            .round()
                                            .clamp(1.0, 16.0) as usize;
                                    }
                                    "tip" => {
                                        use_tip = v == "1" || v.eq_ignore_ascii_case("true");
                                    }
                                    "i" | "interp" | "ease" => {
                                        interp = InterpMode::parse(v).ok_or_else(|| {
                                            format!(
                                                "line {}: unknown interp '{v}' (use step|lin|ease|easein|easeout|cubic)",
                                                al.lineno
                                            )
                                        })?;
                                    }
                                    _ => {
                                        return Err(format!(
                                            "line {}: unknown ik flag '{tok}'",
                                            al.lineno
                                        ));
                                    }
                                }
                            }
                        }
                        let target = target_pt.ok_or_else(|| {
                            format!("line {}: ik requires target position p=x,y,z", al.lineno)
                        })?;
                        ik_keys.push(IkKeyframe {
                            time,
                            effector,
                            target,
                            pole: pole_pt,
                            chain_len,
                            use_tip,
                            interp,
                        });
                    } else if aop == "mw" || aop == "morph" || aop == "shape" {
                        if al.tokens.len() < 4 {
                            return Err(format!(
                                "line {}: mw requires <time|Nf> <morph_name> <weight> [i=...]",
                                al.lineno
                            ));
                        }
                        let time = parse_time(&al.tokens[1], &self.env, al.lineno)?;
                        max_t = max_t.max(time);
                        let target = al.tokens[2]
                            .strip_prefix('@')
                            .unwrap_or(&al.tokens[2])
                            .to_string();
                        let weight = self
                            .env
                            .eval_f32(&al.tokens[3])
                            .map_err(|e| format!("line {}: {e}", al.lineno))?;
                        let mut interp = InterpMode::Linear;
                        for tok in &al.tokens[4..] {
                            if let Some((k, v)) = tok.split_once('=') {
                                if matches!(k.to_ascii_lowercase().as_str(), "i" | "interp" | "ease") {
                                    interp = InterpMode::parse(v).ok_or_else(|| {
                                        format!(
                                            "line {}: unknown interp '{v}' (use step|lin|ease|easein|easeout|cubic)",
                                            al.lineno
                                        )
                                    })?;
                                }
                            }
                        }
                        morph_keys.push(MorphKeyframe {
                            time,
                            target,
                            weight,
                            interp,
                        });
                    } else if aop == "ev" || aop == "event" {
                        if al.tokens.len() < 3 {
                            return Err(format!(
                                "line {}: ev requires <time|Nf> <event_name> [data]",
                                al.lineno
                            ));
                        }
                        let time = parse_time(&al.tokens[1], &self.env, al.lineno)?;
                        max_t = max_t.max(time);
                        let name = al.tokens[2].clone();
                        let data = if al.tokens.len() > 3 {
                            Some(al.tokens[3..].join(" "))
                        } else {
                            None
                        };
                        events.push(AnimEvent { time, name, data });
                    } else {
                        return Err(format!(
                            "line {}: unknown statement '{}' inside anim block (use key, ik, mw, or ev)",
                            al.lineno, al.tokens[0]
                        ));
                    }
                }
                let duration = explicit_dur.unwrap_or_else(|| max_t.max(0.1));
                out.actions.push(AnimAction {
                    name: action_name,
                    duration,
                    fps,
                    looping,
                    keyframes,
                    ik_keys,
                    morph_keys,
                    events,
                });
                i = next_i;
                continue;
            }

            if op == "node" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, "node")?;
                if line.tokens.len() < 2 {
                    return Err(format!(
                        "line {lineno}: 'node' requires a child node name (e.g. node LeverPivot 0,1,0)"
                    ));
                }
                let node_name = line.tokens[1]
                    .strip_prefix('@')
                    .unwrap_or(&line.tokens[1])
                    .to_string();
                let (_tag, pos, node_mat, flags) = self.parse_grp_header(&line.tokens[2..], lineno)?;
                let child_mat = node_mat.as_deref().or(inherited_mat);
                let mut child_mesh = self.eval_lines(&body, child_mat, overrides, depth + 1)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(
                    &child_mesh,
                    flags.anchor.as_ref().unwrap_or(&Anchor::Origin),
                );
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }

                // Helper to register one ChildNodeMesh (supports nested children & pure pivot nodes)
                let mut push_node = |n_name: String, n_pos: Vec3, n_rot: Vec3, n_scale: Vec3, local_m: &Mesh| {
                    if local_m.tris.is_empty() && local_m.nodes.is_empty() {
                        return;
                    }
                    let xform = Mat4::from_trs(n_pos, n_rot, n_scale);
                    let mut n_bb = Aabb::empty();
                    let local_bb = local_m.compute_bounds();
                    if local_bb.is_valid() {
                        for sx in [local_bb.min.x, local_bb.max.x] {
                            for sy in [local_bb.min.y, local_bb.max.y] {
                                for sz in [local_bb.min.z, local_bb.max.z] {
                                    n_bb.expand(xform.transform_point(Vec3::new(sx, sy, sz)));
                                }
                            }
                        }
                    }
                    if n_bb.is_valid() {
                        out.part_bounds
                            .entry(n_name.clone())
                            .and_modify(|e| e.expand_aabb(&n_bb))
                            .or_insert(n_bb);
                    }
                    for (sub_id, sub_bb) in &local_m.part_bounds {
                        let mut xbb = Aabb::empty();
                        for sx in [sub_bb.min.x, sub_bb.max.x] {
                            for sy in [sub_bb.min.y, sub_bb.max.y] {
                                for sz in [sub_bb.min.z, sub_bb.max.z] {
                                    xbb.expand(xform.transform_point(Vec3::new(sx, sy, sz)));
                                }
                            }
                        }
                        out.part_bounds
                            .entry(sub_id.clone())
                            .and_modify(|e| e.expand_aabb(&xbb))
                            .or_insert(xbb);
                    }
                    // Propagate any nested colliders transformed into root space
                    for col in &local_m.colliders {
                        let mut cm = Mesh::new();
                        let src_c = Mesh {
                            tris: col.tris.clone(),
                            ..Mesh::default()
                        };
                        cm.append_transformed(&src_c, xform, None);
                        if !cm.tris.is_empty() {
                            out.colliders.push(ColliderMesh {
                                name: col.name.clone(),
                                tris: cm.tris,
                            });
                        }
                    }
                    out.nodes.push(ChildNodeMesh {
                        name: n_name,
                        translation: n_pos,
                        rotation_deg: n_rot,
                        scale: n_scale,
                        tris: local_m.tris.clone(),
                        children: local_m.nodes.clone(),
                    });
                };

                push_node(node_name.clone(), pos, flags.rot, flags.scale, &child_mesh);
                // If mirror flags are set on a node, emit mirrored sibling nodes (`<name>_m<axes>`)
                let m_count = 1usize << flags.mirror.len();
                for mask in 1..m_count {
                    let mut m_pos = pos;
                    let mut m_scale = Vec3::ONE;
                    let mut suffix = String::new();
                    for (bit, &axis) in flags.mirror.iter().enumerate() {
                        if (mask & (1 << bit)) != 0 {
                            suffix.push(axis);
                            match axis {
                                'x' => { m_pos.x = -m_pos.x; m_scale.x = -1.0; }
                                'y' => { m_pos.y = -m_pos.y; m_scale.y = -1.0; }
                                _ => { m_pos.z = -m_pos.z; m_scale.z = -1.0; }
                            }
                        }
                    }
                    let mut mirrored = Mesh::new();
                    let local_mat = Mat4::scale(m_scale).mul(Mat4::from_trs(Vec3::ZERO, flags.rot, flags.scale));
                    mirrored.append_transformed(&child_mesh, local_mat, None);
                    push_node(format!("{node_name}_m{suffix}"), m_pos, Vec3::ZERO, Vec3::ONE, &mirrored);
                }

                i = next_i;
                continue;
            }

            if op == "col" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, "col")?;
                if line.tokens.len() < 2 {
                    return Err(format!("line {lineno}: 'col' requires a collider node name (e.g. hull-convcolonly or UBX_model_01)"));
                }
                let col_name = line.tokens[1].strip_prefix('@').unwrap_or(&line.tokens[1]).to_string();
                let (_tag, pos, _mat, flags) = self.parse_grp_header(&line.tokens[2..], lineno)?;
                let mut child_mesh = self.eval_lines(&body, Some("cell:0,0"), overrides, depth + 1)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(
                    &child_mesh,
                    flags.anchor.as_ref().unwrap_or(&Anchor::Origin),
                );
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }
                let mut placed = Mesh::new();
                emit_instanced(&mut placed, &child_mesh, pos, &flags, None, self.bones, self.bone_map, lineno)?;
                if !placed.tris.is_empty() {
                    out.colliders.push(ColliderMesh {
                        name: col_name,
                        tris: placed.tris,
                    });
                }
                i = next_i;
                continue;
            }

            if op == "csg" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, "csg")?;
                let (tag, pos, grp_mat, flags) =
                    self.parse_grp_header(&line.tokens[1..], lineno)?;
                let child_mat = grp_mat.as_deref().or(inherited_mat);
                let mut child_mesh = self.eval_csg_block(&body, child_mat, overrides, depth + 1)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(
                    &child_mesh,
                    flags.anchor.as_ref().unwrap_or(&Anchor::Origin),
                );
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref(), self.bones, self.bone_map, lineno)?;
                self.part_count += 1;
                i = next_i;
                continue;
            }

            if op == "sdf" || op == "meta" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, &op)?;
                let (tag, pos, grp_mat, flags) =
                    self.parse_grp_header(&line.tokens[1..], lineno)?;
                let child_mat = grp_mat.as_deref().or(inherited_mat);
                let res = flags
                    .extras
                    .get("res")
                    .map(|v| self.env.eval_f32(v))
                    .transpose()
                    .map_err(|e| format!("line {lineno}: {e}"))?
                    .map(|v| v.round().clamp(8.0, 96.0) as usize)
                    .unwrap_or(28);
                // Default smooth normals = true for SDF blocks unless explicitly `sm=0`
                let explicit_flat = line.tokens[1..]
                    .iter()
                    .any(|t| matches!(t.to_ascii_lowercase().as_str(), "sm=0" | "sm=false" | "smooth=0"));
                let smooth = !explicit_flat;
                let mut child_mesh = self.eval_sdf_block(&body, child_mat, overrides, res, smooth)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(
                    &child_mesh,
                    flags.anchor.as_ref().unwrap_or(&Anchor::Origin),
                );
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref(), self.bones, self.bone_map, lineno)?;
                self.part_count += 1;
                i = next_i;
                continue;
            }

            if op == "grp" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, "grp")?;
                let (tag, pos, grp_mat, flags) =
                    self.parse_grp_header(&line.tokens[1..], lineno)?;
                let child_mat = grp_mat.as_deref().or(inherited_mat);
                let mut child_mesh = self.eval_lines(&body, child_mat, overrides, depth + 1)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(&child_mesh, flags.anchor.as_ref().unwrap_or(&Anchor::Origin));
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref(), self.bones, self.bone_map, lineno)?;
                i = next_i;
                continue;
            }

            if op == "use" {
                let (tag, def_name, pos, flags) =
                    self.parse_use_line(&line.tokens[1..], lineno)?;
                let body = self.defs.get(&def_name).ok_or_else(|| {
                    format!("line {lineno}: unknown def template '{def_name}'")
                })?;
                let mut merged_ov = overrides.clone();
                for (k, v) in &flags.color_overrides {
                    merged_ov.insert(k.clone(), v.clone());
                }
                let mut child_mesh = self.eval_lines(body, inherited_mat, &merged_ov, depth + 1)?;
                apply_deformers(&mut child_mesh, &flags);
                let anchor_shift = compute_anchor_shift(&child_mesh, flags.anchor.as_ref().unwrap_or(&Anchor::Origin));
                if anchor_shift.len_sq() > 1e-10 {
                    child_mesh.translate(anchor_shift);
                }
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref(), self.bones, self.bone_map, lineno)?;
                i += 1;
                continue;
            }

            // Primitive statement
            self.eval_primitive(&mut out, &op, &line.tokens[1..], inherited_mat, overrides, lineno)?;
            self.part_count += 1;
            i += 1;
        }
        Ok(out)
    }

    fn eval_csg_block(
        &mut self,
        lines: &[RawLine],
        inherited_mat: Option<&str>,
        overrides: &HashMap<String, String>,
        depth: usize,
    ) -> Result<Mesh, String> {
        let mut acc = Mesh::new();
        let mut i = 0usize;
        while i < lines.len() {
            let line = &lines[i];
            let lineno = line.lineno;
            let op0 = line.tokens[0].to_ascii_lowercase();
            if op0.starts_with('$') {
                parse_var_line(&line.tokens, &mut self.env, lineno)?;
                i += 1;
                continue;
            }

            let (csg_op, rest_start) = match op0.as_str() {
                "add" | "uni" | "union" => (CsgOp::Union, 1usize),
                "sub" | "cut" | "diff" => (CsgOp::Subtract, 1usize),
                "int" | "intersect" => (CsgOp::Intersect, 1usize),
                _ => (CsgOp::Union, 0usize),
            };
            if rest_start >= line.tokens.len() {
                return Err(format!("line {lineno}: CSG op '{op0}' requires a primitive or block"));
            }

            // Support both `sub cyl mat ...` and `sub mat cyl ...`
            let mut sub_tokens = line.tokens[rest_start..].to_vec();
            let mut item_mat = inherited_mat;
            let owned_mat: String;
            if rest_start == 1
                && sub_tokens.len() >= 2
                && !is_mesh_op(&sub_tokens[0])
                && is_mesh_op(&sub_tokens[1])
            {
                owned_mat = sub_tokens.remove(0);
                item_mat = Some(owned_mat.as_str());
            }

            let head_op = sub_tokens[0].to_ascii_lowercase();
            let operand = if head_op == "grp" {
                let (body, next_i) = collect_block(lines, i + 1, lineno, "grp")?;
                let (_tag, pos, grp_mat, flags) =
                    self.parse_grp_header(&sub_tokens[1..], lineno)?;
                let cm = grp_mat.as_deref().or(item_mat);
                let mut m = self.eval_lines(&body, cm, overrides, depth + 1)?;
                apply_deformers(&mut m, &flags);
                let ash = compute_anchor_shift(&m, flags.anchor.as_ref().unwrap_or(&Anchor::Origin));
                if ash.len_sq() > 1e-10 {
                    m.translate(ash);
                }
                let mut placed = Mesh::new();
                emit_instanced(&mut placed, &m, pos, &flags, None, self.bones, self.bone_map, lineno)?;
                i = next_i;
                placed
            } else if head_op == "use" {
                let (_tag, def_name, pos, flags) = self.parse_use_line(&sub_tokens[1..], lineno)?;
                let body = self.defs.get(&def_name).ok_or_else(|| {
                    format!("line {lineno}: unknown def template '{def_name}'")
                })?;
                let mut merged_ov = overrides.clone();
                for (k, v) in &flags.color_overrides {
                    merged_ov.insert(k.clone(), v.clone());
                }
                let mut m = self.eval_lines(body, item_mat, &merged_ov, depth + 1)?;
                apply_deformers(&mut m, &flags);
                let ash = compute_anchor_shift(&m, flags.anchor.as_ref().unwrap_or(&Anchor::Origin));
                if ash.len_sq() > 1e-10 {
                    m.translate(ash);
                }
                let mut placed = Mesh::new();
                emit_instanced(&mut placed, &m, pos, &flags, None, self.bones, self.bone_map, lineno)?;
                i += 1;
                placed
            } else {
                let mut m = Mesh::new();
                self.eval_primitive(&mut m, &head_op, &sub_tokens[1..], item_mat, overrides, lineno)?;
                i += 1;
                m
            };

            if acc.tris.is_empty() && csg_op == CsgOp::Union {
                acc = operand;
            } else {
                acc = apply_csg(&acc, &operand, csg_op);
            }
        }
        Ok(acc)
    }

    fn eval_sdf_block(
        &mut self,
        lines: &[RawLine],
        inherited_mat: Option<&str>,
        overrides: &HashMap<String, String>,
        res: usize,
        smooth: bool,
    ) -> Result<Mesh, String> {
        let mut prims: Vec<SdfPrim> = Vec::new();
        for line in lines {
            let lineno = line.lineno;
            let op0 = line.tokens[0].to_ascii_lowercase();
            if op0.starts_with('$') {
                parse_var_line(&line.tokens, &mut self.env, lineno)?;
                continue;
            }

            let (mut sdf_op, rest_start) = match op0.as_str() {
                "add" | "uni" | "union" => (SdfOp::Union, 1usize),
                "sub" | "cut" | "diff" => (SdfOp::Subtract, 1usize),
                "int" | "intersect" => (SdfOp::Intersect, 1usize),
                "paint" | "col" => (SdfOp::Paint, 1usize),
                _ => (SdfOp::Union, 0usize),
            };
            if rest_start >= line.tokens.len() {
                return Err(format!("line {lineno}: SDF op '{op0}' requires a primitive"));
            }

            // Support both `sub sph mat ...` and `sub mat sph ...`
            let mut toks = line.tokens[rest_start..].to_vec();
            let mut item_mat = inherited_mat;
            let owned_mat: String;
            if rest_start == 1
                && toks.len() >= 2
                && !is_sdf_prim(&toks[0])
                && is_sdf_prim(&toks[1])
            {
                owned_mat = toks.remove(0);
                item_mat = Some(owned_mat.as_str());
            }

            let shape_op = toks[0].to_ascii_lowercase();
            let mut pos_args = Vec::new();
            let mut flag_tokens = Vec::new();
            for tok in &toks[1..] {
                if is_flag_token(tok) {
                    flag_tokens.push(tok.clone());
                } else {
                    pos_args.push(tok.clone());
                }
            }
            let flags = parse_flags(&flag_tokens, &self.env, lineno)?;
            if let Some(op_s) = flags.extras.get("op") {
                sdf_op = match op_s.to_ascii_lowercase().as_str() {
                    "sub" | "cut" => SdfOp::Subtract,
                    "int" | "intersect" => SdfOp::Intersect,
                    "paint" => SdfOp::Paint,
                    _ => SdfOp::Union,
                };
            }
            let smooth_k = flags
                .extras
                .get("k")
                .map(|v| self.env.eval_f32(v))
                .transpose()
                .map_err(|e| format!("line {lineno}: {e}"))?
                .unwrap_or(0.0)
                .max(0.0);
            let round = flags
                .extras
                .get("rnd")
                .or_else(|| flags.extras.get("bev"))
                .map(|v| self.env.eval_f32(v))
                .transpose()
                .map_err(|e| format!("line {lineno}: {e}"))?
                .unwrap_or(0.0)
                .max(0.0);

            let (mat_str, geom_args) = if let Some(im) = item_mat {
                if !pos_args.is_empty() && !pos_args[0].contains(',') {
                    (pos_args[0].as_str(), &pos_args[1..])
                } else {
                    (im, &pos_args[..])
                }
            } else {
                if pos_args.is_empty() {
                    return Err(format!("line {lineno}: SDF '{shape_op}' requires a material"));
                }
                (pos_args[0].as_str(), &pos_args[1..])
            };
            let mat = self.resolve_mat_name(mat_str, overrides, lineno)?;

            // Collect mirrored variants (identity + mirrored axes)
            let mut mirror_signs = vec![Vec3::ONE];
            for &ax in &flags.mirror {
                let cur_len = mirror_signs.len();
                for j in 0..cur_len {
                    let mut s = mirror_signs[j];
                    match ax {
                        'x' => s.x = -s.x,
                        'y' => s.y = -s.y,
                        'z' => s.z = -s.z,
                        _ => {}
                    }
                    mirror_signs.push(s);
                }
            }

            let make_inv_rot = |rot_deg: Vec3, ms: Vec3| -> Mat4 {
                let rx = if ms.y * ms.z < 0.0 { -rot_deg.x } else { rot_deg.x };
                let ry = if ms.x * ms.z < 0.0 { -rot_deg.y } else { rot_deg.y };
                let rz = if ms.x * ms.y < 0.0 { -rot_deg.z } else { rot_deg.z };
                Mat4::rot_x(-rx).mul(Mat4::rot_y(-ry)).mul(Mat4::rot_z(-rz))
            };

            let mul_sign = |v: Vec3, ms: Vec3| -> Vec3 { Vec3::new(v.x * ms.x, v.y * ms.y, v.z * ms.z) };

            for ms in mirror_signs {
                let shape = match shape_op.as_str() {
                    "sph" | "sphere" => {
                        if geom_args.len() < 2 {
                            return Err(format!("line {lineno}: sdf sph requires <x,y,z> <r>"));
                        }
                        let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                        let r = self.env.eval_f32(&geom_args[1]).map_err(|e| format!("line {lineno}: {e}"))?;
                        SdfShape::Sphere { center: c, radius: r.max(1e-4) }
                    }
                    "ell" | "ellipsoid" => {
                        if geom_args.len() < 2 {
                            return Err(format!("line {lineno}: sdf ell requires <x,y,z> <rx,ry,rz>"));
                        }
                        let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                        let r = self.parse_vec3(&geom_args[1], lineno)?;
                        SdfShape::Ellipsoid {
                            center: c,
                            radii: r,
                            inv_rot: make_inv_rot(flags.rot, ms),
                        }
                    }
                    "cap" | "capsule" => {
                        if geom_args.len() >= 3 && geom_args[1].split(',').count() == 3 {
                            // Point-to-point tapered capsule: cap <mat> <x0,y0,z0> <x1,y1,z1> <r0[,r1]>
                            let a = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                            let b = mul_sign(self.parse_vec3(&geom_args[1], lineno)?, ms);
                            let rv = self.parse_f32_list(&geom_args[2], lineno)?;
                            let r0 = rv[0].max(1e-4);
                            let r1 = rv.get(1).copied().unwrap_or(r0).max(1e-4);
                            SdfShape::Capsule { a, b, r0, r1 }
                        } else if geom_args.len() >= 2 {
                            // Centered capsule: cap <mat> <x,y,z> <r,h|r0,r1,h> [r=...]
                            let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                            let rv = self.parse_f32_list(&geom_args[1], lineno)?;
                            let (r0, r1, h) = match rv.len() {
                                2 => (rv[0], rv[0], rv[1]),
                                3 => (rv[0], rv[1], rv[2]),
                                _ => return Err(format!("line {lineno}: sdf cap expects <r,h> or <x1,y1,z1> <r0[,r1]>")),
                            };
                            let half = ((h - r0 - r1) * 0.5).max(0.0);
                            let rot_m = Mat4::from_trs(Vec3::ZERO, flags.rot, Vec3::ONE);
                            let off = mul_sign(rot_m.transform_point(Vec3::new(0.0, half, 0.0)), ms);
                            SdfShape::Capsule { a: c - off, b: c + off, r0: r0.max(1e-4), r1: r1.max(1e-4) }
                        } else {
                            return Err(format!("line {lineno}: sdf cap requires <x0,y0,z0> <x1,y1,z1> <r0[,r1]> or <x,y,z> <r,h>"));
                        }
                    }
                    "box" | "cube" => {
                        if geom_args.len() < 2 {
                            return Err(format!("line {lineno}: sdf box requires <x,y,z> <sx,sy,sz>"));
                        }
                        let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                        let sz = self.parse_vec3(&geom_args[1], lineno)?;
                        SdfShape::RoundBox {
                            center: c,
                            half: sz * 0.5,
                            round,
                            inv_rot: make_inv_rot(flags.rot, ms),
                        }
                    }
                    "cyl" | "cylinder" => {
                        if geom_args.len() < 2 {
                            return Err(format!("line {lineno}: sdf cyl requires <x,y,z> <r,h>"));
                        }
                        let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                        let rv = self.parse_f32_list(&geom_args[1], lineno)?;
                        if rv.len() < 2 {
                            return Err(format!("line {lineno}: sdf cyl expects <r,h>"));
                        }
                        SdfShape::RoundCyl {
                            center: c,
                            radius: rv[0].max(1e-4),
                            half_h: (rv[1] * 0.5).max(1e-4),
                            round,
                            inv_rot: make_inv_rot(flags.rot, ms),
                        }
                    }
                    "tor" | "torus" => {
                        if geom_args.len() < 2 {
                            return Err(format!("line {lineno}: sdf tor requires <x,y,z> <R,r>"));
                        }
                        let c = mul_sign(self.parse_vec3(&geom_args[0], lineno)?, ms);
                        let rv = self.parse_f32_list(&geom_args[1], lineno)?;
                        if rv.len() < 2 {
                            return Err(format!("line {lineno}: sdf tor expects <R,r>"));
                        }
                        SdfShape::Torus {
                            center: c,
                            major: rv[0].max(1e-4),
                            minor: rv[1].max(1e-4),
                            inv_rot: make_inv_rot(flags.rot, ms),
                        }
                    }
                    other => {
                        return Err(format!(
                            "line {lineno}: unknown SDF primitive '{other}' (use: sph, ell, cap, box, cyl, tor)"
                        ));
                    }
                };
                prims.push(SdfPrim { shape, op: sdf_op, smooth_k, mat });
            }
        }
        Ok(build_sdf_mesh(&prims, res, smooth))
    }

    fn parse_f32_list(&self, s: &str, lineno: usize) -> Result<Vec<f32>, String> {
        self.env
            .eval_list_f32(s)
            .map_err(|e| format!("line {lineno}: {e}"))
    }

    fn parse_grp_header(
        &self,
        args: &[String],
        lineno: usize,
    ) -> Result<(Option<String>, Vec3, Option<String>, CommonFlags), String> {
        let mut idx = 0usize;
        let tag = take_tag(args, &mut idx);
        let mut pos_args = Vec::new();
        let mut flag_tokens = Vec::new();
        for tok in &args[idx..] {
            if is_flag_token(tok) {
                flag_tokens.push(tok.clone());
            } else {
                pos_args.push(tok.clone());
            }
        }
        let flags = parse_flags(&flag_tokens, &self.env, lineno)?;
        let mut pos = Vec3::ZERO;
        let mut mat: Option<String> = None;
        for a in pos_args {
            if a.contains(',') && self.parse_vec3(&a, lineno).is_ok() {
                pos = self.parse_vec3(&a, lineno)?;
            } else {
                mat = Some(a);
            }
        }
        Ok((tag, pos, mat, flags))
    }

    fn parse_use_line(
        &self,
        args: &[String],
        lineno: usize,
    ) -> Result<(Option<String>, String, Vec3, CommonFlags), String> {
        let mut idx = 0usize;
        let tag = take_tag(args, &mut idx);
        let mut pos_args = Vec::new();
        let mut flag_tokens = Vec::new();
        for tok in &args[idx..] {
            if is_flag_token(tok) {
                flag_tokens.push(tok.clone());
            } else {
                pos_args.push(tok.clone());
            }
        }
        if pos_args.is_empty() {
            return Err(format!("line {lineno}: 'use' requires a template name"));
        }
        let def_name = pos_args[0].clone();
        let pos = if pos_args.len() >= 2 {
            self.parse_vec3(&pos_args[1], lineno)?
        } else {
            Vec3::ZERO
        };
        let flags = parse_flags(&flag_tokens, &self.env, lineno)?;
        Ok((tag, def_name, pos, flags))
    }

    fn eval_primitive(
        &self,
        out: &mut Mesh,
        op: &str,
        args: &[String],
        inherited_mat: Option<&str>,
        overrides: &HashMap<String, String>,
        lineno: usize,
    ) -> Result<(), String> {
        let mut idx = 0usize;
        let tag = take_tag(args, &mut idx);
        let mut pos_args = Vec::new();
        let mut flag_tokens = Vec::new();
        for tok in &args[idx..] {
            if is_flag_token(tok) {
                flag_tokens.push(tok.clone());
            } else {
                pos_args.push(tok.clone());
            }
        }
        let flags = parse_flags(&flag_tokens, &self.env, lineno)?;

        // First positional arg is material/tex/band name unless omitted (only when inherited_mat exists & arg0 looks numeric/vec)
        let (mat_str, rest) = if pos_args.is_empty() {
            return Err(format!("line {lineno}: primitive '{op}' missing arguments"));
        } else if inherited_mat.is_some()
            && self.resolve_tex_or_band(&pos_args[0], overrides).is_none()
            && self.palette.resolve_mat(&pos_args[0]).is_err()
            && !pos_args[0].starts_with("cell:")
            && !pos_args[0].starts_with("uv:")
            && pos_args[0].contains(',')
        {
            (inherited_mat.unwrap(), &pos_args[..])
        } else {
            (pos_args[0].as_str(), &pos_args[1..])
        };

        let tex_binding = self.resolve_tex_or_band(mat_str, overrides);
        let mat = if tex_binding.is_some() {
            ResolvedMat { uv: Vec2::new(0.5, 0.5), is_translucent: false }
        } else {
            self.resolve_mat_name(mat_str, overrides, lineno)?
        };
        let mut fc = FaceMats::default();
        for (face_k, mat_v) in &flags.face_colors {
            fc.map
                .insert(face_k.clone(), self.resolve_mat_name(mat_v, overrides, lineno)?);
        }

        let mut variants = Vec::new();
        if let Some(mats_str) = flags.extras.get("mats") {
            for m_tok in mats_str.split(',') {
                if !m_tok.is_empty() {
                    variants.push(self.resolve_mat_name(m_tok, overrides, lineno)?);
                }
            }
        }

        let mut pos = Vec3::ZERO;
        let default_anchor;
        let mut local: Mesh = match op {
            "box" | "cube" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: box requires <pos> <sx,sy,sz>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let sz = self.parse_vec3_or_scalar(&rest[1], lineno)?;
                let taper = match flags.extras.get("t") {
                    Some(s) => self.parse_vec2_or_scalar(s, lineno)?,
                    None => Vec2::new(1.0, 1.0),
                };
                let top_off = match flags.extras.get("o") {
                    Some(s) => self.parse_vec2(s, lineno)?,
                    None => Vec2::ZERO,
                };
                let bev = self.opt_f32(&flags, "bev", 0.0, lineno)?;
                build_box(sz, taper, top_off, bev, mat, &fc)
            }
            "cyl" | "cone" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: {op} requires <pos> <rb,rt,h> or <r,h>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let dims = self
                    .env
                    .eval_list_f32(&rest[1])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let (rb, rt, h) = match (op, dims.as_slice()) {
                    ("cone", [r, h]) => (*r, 0.0, *h),
                    (_, [r, h]) => (*r, *r, *h),
                    (_, [rb, rt, h]) => (*rb, *rt, *h),
                    _ => {
                        return Err(format!(
                            "line {lineno}: {op} dims must be <r,h> or <rb,rt,h>"
                        ))
                    }
                };
                let seg = self.opt_u32(&flags, "seg", 12, lineno)?;
                let cap = self.opt_bool(&flags, "cap", true);
                let bev = self.opt_f32(&flags, "bev", 0.0, lineno)?;
                build_cylinder(rb, rt, h, seg, cap, bev, mat, &fc)
            }
            "cap" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: cap requires <pos> <r,h> or <rb,rt,h>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let dims = self
                    .env
                    .eval_list_f32(&rest[1])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let (rb, rt, h) = match dims.as_slice() {
                    [r, h] => (*r, *r, *h),
                    [rb, rt, h] => (*rb, *rt, *h),
                    _ => return Err(format!("line {lineno}: cap dims must be <r,h> or <rb,rt,h>")),
                };
                let seg = self.opt_u32(&flags, "seg", 10, lineno)?;
                let rings = self.opt_u32(&flags, "rings", 4, lineno)?;
                build_capsule(rb, rt, h, seg, rings, mat)
            }
            "wdg" | "wedge" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: wdg requires <pos> <sx,sy,sz>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let sz = self.parse_vec3_or_scalar(&rest[1], lineno)?;
                build_wedge(sz, mat, &fc)
            }
            "pyr" | "pyramid" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: pyr requires <pos> <sx,sy,sz>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let sz = self.parse_vec3_or_scalar(&rest[1], lineno)?;
                let sides = self.opt_u32(&flags, "sides", 4, lineno)?;
                let top_off = match flags.extras.get("o") {
                    Some(s) => self.parse_vec2(s, lineno)?,
                    None => Vec2::ZERO,
                };
                build_pyramid(sz, sides, top_off, mat, &fc)
            }
            "ico" | "sphere" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: ico requires <pos> <radius>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let r = self
                    .env
                    .eval_f32(&rest[1])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let sub = self.opt_u32(&flags, "sub", 1, lineno)?;
                let jit = self.opt_f32(&flags, "jit", 0.0, lineno)?;
                let seed = self.opt_u32(&flags, "seed", 1, lineno)? as u64;
                build_icosphere(r, sub, jit, seed, mat, &variants)
            }
            "tor" | "torus" => {
                default_anchor = Anchor::Center;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: tor requires <pos> <R,r>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let rr = self.parse_vec2(&rest[1], lineno)?;
                let seg = self.opt_u32(&flags, "seg", 16, lineno)?;
                let tseg = self.opt_u32(&flags, "tseg", 8, lineno)?;
                build_torus(rr.x, rr.y, seg, tseg, mat)
            }
            "pri" | "prism" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 3 {
                    return Err(format!(
                        "line {lineno}: pri requires <pos> <depth> <x0:y0,x1:y1,...>"
                    ));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let depth = self
                    .env
                    .eval_f32(&rest[1])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let pts = self.parse_vec2_pairs(&rest[2], lineno)?;
                let bev = self.opt_f32(&flags, "bev", 0.0, lineno)?;
                build_prism(&pts, depth, bev, mat, &fc)
            }
            "lat" | "lathe" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: lat requires <pos> <r0:y0,r1:y1,...>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let pairs = self.parse_vec2_pairs(&rest[1], lineno)?;
                let prof: Vec<(f32, f32)> = pairs.into_iter().map(|v| (v.x, v.y)).collect();
                let seg = self.opt_u32(&flags, "seg", 12, lineno)?;
                let cap = self.opt_bool(&flags, "cap", true);
                build_lathe(&prof, seg, cap, mat, &fc)
            }
            "tub" | "tube" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 3 {
                    return Err(format!(
                        "line {lineno}: tub requires <pos> <r|r0,r1,...> <x:y:z,...>"
                    ));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let radii = self
                    .env
                    .eval_list_f32(&rest[1])
                    .map_err(|e| format!("line {lineno}: {e}"))?;
                let pts = self.parse_vec3_list(&rest[2], lineno)?;
                let seg = self.opt_u32(&flags, "seg", 8, lineno)?;
                let cap = self.opt_bool(&flags, "cap", true);
                build_tube(&pts, &radii, seg, cap, mat)
            }
            "lof" | "loft" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 2 {
                    return Err(format!(
                        "line {lineno}: lof requires <pos> <x:y:z/rx:ry[/tw][/mat],...>"
                    ));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let sections = self.parse_loft_sections(&rest[1], overrides, lineno)?;
                let seg = self.opt_u32(&flags, "seg", 10, lineno)?;
                let ph = self.opt_f32(&flags, "ph", 0.0, lineno)?;
                let cap = self.opt_bool(&flags, "cap", true);
                build_loft(&sections, seg, ph, cap, mat)
            }
            "ter" | "terrain" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: ter requires <pos> <sx,sz>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let sz = self.parse_vec2_or_scalar(&rest[1], lineno)?;
                let (sub_x, sub_z) = match flags.extras.get("sub") {
                    Some(s) => {
                        let v = self
                            .env
                            .eval_list_f32(s)
                            .map_err(|e| format!("line {lineno}: {e}"))?;
                        match v.as_slice() {
                            [n] => ((*n as u32).max(1), (*n as u32).max(1)),
                            [nx, nz] => ((*nx as u32).max(1), (*nz as u32).max(1)),
                            _ => return Err(format!("line {lineno}: invalid sub={s}")),
                        }
                    }
                    None => (8, 8),
                };
                let jit = self.opt_f32(&flags, "jit", 0.08, lineno)?;
                let mnd = self.opt_f32(&flags, "mnd", 0.0, lineno)?;
                let pin = self.opt_bool(&flags, "pin", true);
                let thk = self.opt_f32(&flags, "thk", 0.0, lineno)?;
                let seed = self.opt_u32(&flags, "seed", 1, lineno)? as u64;
                build_noisy_plane(sz, sub_x, sub_z, jit, mnd, pin, thk, seed, mat, &variants, &fc)
            }
            "tri" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 3 {
                    return Err(format!("line {lineno}: tri requires <p0> <p1> <p2>"));
                }
                let p0 = self.parse_vec3_colon(&rest[0], lineno)?;
                let p1 = self.parse_vec3_colon(&rest[1], lineno)?;
                let p2 = self.parse_vec3_colon(&rest[2], lineno)?;
                let ds = self.opt_bool(&flags, "ds", false);
                let off = self.opt_f32(&flags, "off", 0.001, lineno)?.abs().max(1e-4);
                let mut m = Mesh::new();
                if ds {
                    let n = (p1 - p0).cross(p2 - p0).normalized();
                    let shift = if n.len_sq() > 0.5 { n * off } else { Vec3::ZERO };
                    let m_back = fc.pick(mat, &["k", "back", "b", "bot", "bottom"]);
                    m.add_tri(p0 + shift, p1 + shift, p2 + shift, mat);
                    m.add_tri(p0 - shift, p2 - shift, p1 - shift, m_back);
                } else {
                    m.add_tri(p0, p1, p2, mat);
                }
                m
            }
            "quad" => {
                default_anchor = Anchor::Origin;
                if rest.len() < 4 {
                    return Err(format!("line {lineno}: quad requires <x0:y0:z0> <x1:y1:z1> <x2:y2:z2> <x3:y3:z3>"));
                }
                let p0 = self.parse_vec3_colon(&rest[0], lineno)?;
                let p1 = self.parse_vec3_colon(&rest[1], lineno)?;
                let p2 = self.parse_vec3_colon(&rest[2], lineno)?;
                let p3 = self.parse_vec3_colon(&rest[3], lineno)?;
                let ds = self.opt_bool(&flags, "ds", false);
                let off = self.opt_f32(&flags, "off", 0.001, lineno)?.abs().max(1e-4);
                let mut m = Mesh::new();
                if let Some((tid, band_opt)) = tex_binding {
                    let (u0, v0, u1, v1) = self.resolve_quad_uv_rect(p0, p1, p3, tid, band_opt, &flags, lineno)?;
                    let (ua, ub, uc, ud) = transform_quad_uvs(
                        Vec2::new(u0, v1),
                        Vec2::new(u1, v1),
                        Vec2::new(u1, v0),
                        Vec2::new(u0, v0),
                        &flags,
                        &self.env,
                        lineno,
                    )?;
                    let n = (p1 - p0).cross(p2 - p0).normalized();
                    if ds {
                        let shift = if n.len_sq() > 0.5 { n * off } else { Vec3::ZERO };
                        m.add_quad_uv(p0 + shift, p1 + shift, p2 + shift, p3 + shift, ua, ub, uc, ud, Some(tid), mat.is_translucent);
                        m.add_quad_uv(p1 - shift, p0 - shift, p3 - shift, p2 - shift, ub, ua, ud, uc, Some(tid), mat.is_translucent);
                    } else {
                        m.add_quad_uv(p0, p1, p2, p3, ua, ub, uc, ud, Some(tid), mat.is_translucent);
                    }
                } else if ds {
                    let n = (p1 - p0).cross(p2 - p0).normalized();
                    let shift = if n.len_sq() > 0.5 { n * off } else { Vec3::ZERO };
                    let m_back = fc.pick(mat, &["k", "back", "b", "bot", "bottom"]);
                    m.add_quad(p0 + shift, p1 + shift, p2 + shift, p3 + shift, mat);
                    m.add_quad(p1 - shift, p0 - shift, p3 - shift, p2 - shift, m_back);
                } else {
                    m.add_quad(p0, p1, p2, p3, mat);
                }
                m
            }
            "rib" | "ribbon" => {
                default_anchor = Anchor::Bottom;
                if rest.len() < 2 {
                    return Err(format!("line {lineno}: rib requires <pos> <w0,w1,h[,curve]>"));
                }
                pos = self.parse_vec3(&rest[0], lineno)?;
                let dims = self.parse_f32_list(&rest[1], lineno)?;
                if dims.len() < 3 || dims.len() > 4 {
                    return Err(format!("line {lineno}: rib expects w0,w1,h or w0,w1,h,curve"));
                }
                let curve = dims.get(3).copied().unwrap_or(0.0);
                let seg = self.opt_u32(&flags, "seg", 4, lineno)?;
                let ds = self.opt_bool(&flags, "ds", true);
                let off = self.opt_f32(&flags, "off", 0.001, lineno)?;
                build_ribbon(dims[0], dims[1], dims[2], curve, seg, ds, off, mat, &fc)
            }
            _ => {
                return Err(format!("line {lineno}: unknown opcode '{op}'"));
            }
        };

        if let Some((tid, band_opt)) = tex_binding {
            if op != "quad" {
                self.apply_tex_uvs(&mut local, op, tid, band_opt, &flags, lineno)?;
            }
        }

        if flags.smooth && flags.weighted_normals.is_none() {
            local.compute_smooth_normals();
        }
        apply_deformers(&mut local, &flags);

        // Apply anchor in local primitive space BEFORE rotation/scale/translation
        let anchor = flags.anchor.as_ref().unwrap_or(&default_anchor);
        let shift = compute_anchor_shift(&local, anchor);
        if shift.len_sq() > 1e-10 {
            local.translate(shift);
        }

        emit_instanced(out, &local, pos, &flags, tag.as_deref(), self.bones, self.bone_map, lineno)?;
        Ok(())
    }

    fn parse_vec3(&self, s: &str, lineno: usize) -> Result<Vec3, String> {
        let v = self
            .env
            .eval_list_f32(s)
            .map_err(|e| format!("line {lineno}: {e}"))?;
        if v.len() != 3 {
            return Err(format!("line {lineno}: expected 3 values in '{s}'"));
        }
        Ok(Vec3::new(v[0], v[1], v[2]))
    }

    fn parse_vec3_colon(&self, s: &str, lineno: usize) -> Result<Vec3, String> {
        let replaced = s.replace(':', ",");
        self.parse_vec3(&replaced, lineno)
    }

    fn parse_vec3_or_scalar(&self, s: &str, lineno: usize) -> Result<Vec3, String> {
        let v = self
            .env
            .eval_list_f32(s)
            .map_err(|e| format!("line {lineno}: {e}"))?;
        match v.as_slice() {
            [a] => Ok(Vec3::new(*a, *a, *a)),
            [a, b, c] => Ok(Vec3::new(*a, *b, *c)),
            _ => Err(format!("line {lineno}: expected scalar or x,y,z in '{s}'")),
        }
    }

    fn parse_vec2(&self, s: &str, lineno: usize) -> Result<Vec2, String> {
        let v = self
            .env
            .eval_list_f32(s)
            .map_err(|e| format!("line {lineno}: {e}"))?;
        if v.len() != 2 {
            return Err(format!("line {lineno}: expected 2 values in '{s}'"));
        }
        Ok(Vec2::new(v[0], v[1]))
    }

    fn parse_vec2_or_scalar(&self, s: &str, lineno: usize) -> Result<Vec2, String> {
        let v = self
            .env
            .eval_list_f32(s)
            .map_err(|e| format!("line {lineno}: {e}"))?;
        match v.as_slice() {
            [a] => Ok(Vec2::new(*a, *a)),
            [a, b] => Ok(Vec2::new(*a, *b)),
            _ => Err(format!("line {lineno}: expected scalar or a,b in '{s}'")),
        }
    }

    fn parse_vec2_pairs(&self, s: &str, lineno: usize) -> Result<Vec<Vec2>, String> {
        let mut out = Vec::new();
        for item in split_top_level(s, ',') {
            let parts: Vec<&str> = item.split(':').collect();
            if parts.len() != 2 {
                return Err(format!("line {lineno}: expected x:y pair, got '{item}'"));
            }
            let x = self
                .env
                .eval_f32(parts[0])
                .map_err(|e| format!("line {lineno}: {e}"))?;
            let y = self
                .env
                .eval_f32(parts[1])
                .map_err(|e| format!("line {lineno}: {e}"))?;
            out.push(Vec2::new(x, y));
        }
        Ok(out)
    }

    fn resolve_uv_spec(
        &self,
        band_opt: Option<TexBand>,
        flags: &CommonFlags,
        lineno: usize,
    ) -> Result<Option<(f32, f32, f32, f32, bool)>, String> {
        if let Some(uv_str) = flags.extras.get("uv") {
            let clean = uv_str.strip_prefix("band:").unwrap_or(uv_str);
            if let Some(b) = self.band_map.get(&clean.to_ascii_lowercase()) {
                return Ok(Some((b.u0, b.v0, b.u1, b.v1, b.is_strip)));
            }
            if let Some((a, b)) = clean.split_once("..") {
                let v0 = self.env.eval_f32(a.trim()).map_err(|e| format!("line {lineno}: {e}"))?;
                let v1 = self.env.eval_f32(b.trim()).map_err(|e| format!("line {lineno}: {e}"))?;
                return Ok(Some((0.0, v0, 1.0, v1, true)));
            }
            let vals = self.env.eval_list_f32(clean).map_err(|e| format!("line {lineno}: {e}"))?;
            if vals.len() != 4 {
                return Err(format!("line {lineno}: uv= expects u0,v0,u1,v1, v0..v1, or band name"));
            }
            return Ok(Some((vals[0], vals[1], vals[2], vals[3], false)));
        }
        Ok(band_opt.map(|b| (b.u0, b.v0, b.u1, b.v1, b.is_strip)))
    }

    fn resolve_quad_uv_rect(
        &self,
        p0: Vec3,
        p1: Vec3,
        p3: Vec3,
        tid: u16,
        band_opt: Option<TexBand>,
        flags: &CommonFlags,
        lineno: usize,
    ) -> Result<(f32, f32, f32, f32), String> {
        let tm = &self.textures[tid as usize];
        let len_u = (p1 - p0).len();
        let len_v = (p3 - p0).len();
        if let Some((u0, v0, u1, v1, is_strip)) = self.resolve_uv_spec(band_opt, flags, lineno)? {
            if is_strip {
                Ok((0.0, v0, len_u / tm.tile_w, v1))
            } else {
                Ok((u0, v0, u1, v1))
            }
        } else {
            Ok((0.0, 0.0, len_u / tm.tile_w, len_v / tm.tile_h))
        }
    }

    fn apply_tex_uvs(
        &self,
        mesh: &mut Mesh,
        op: &str,
        tid: u16,
        band_opt: Option<TexBand>,
        flags: &CommonFlags,
        lineno: usize,
    ) -> Result<(), String> {
        let tm = &self.textures[tid as usize];
        let bb = mesh.compute_bounds();
        if !bb.is_valid() {
            return Ok(());
        }
        let uv_spec = self.resolve_uv_spec(band_opt, flags, lineno)?;
        let uvs = match flags.extras.get("uvs") {
            Some(s) => self.parse_vec2_or_scalar(s, lineno)?,
            None => Vec2::new(1.0, 1.0),
        };
        let uvo = match flags.extras.get("uvo") {
            Some(s) => self.parse_vec2(s, lineno)?,
            None => Vec2::ZERO,
        };
        let uvr_deg = self.opt_f32(flags, "uvr", 0.0, lineno)?;
        let rot_rad = uvr_deg.to_radians();
        let (rs, rc) = rot_rad.sin_cos();

        let finalize_uv = |u: f32, v: f32| -> Vec2 {
            let su = u * uvs.x + uvo.x;
            let sv = v * uvs.y + uvo.y;
            if uvr_deg.abs() > 1e-4 {
                Vec2::new(su * rc - sv * rs, su * rs + sv * rc)
            } else {
                Vec2::new(su, sv)
            }
        };

        let y_span = (bb.max.y - bb.min.y).max(1e-4);
        let is_cyl = matches!(op, "cyl" | "cone" | "cap" | "lat");
        let cyl_r = ((bb.max.x - bb.min.x).max(bb.max.z - bb.min.z) * 0.5).max(0.05);
        let cyl_rep = ((std::f32::consts::TAU * cyl_r) / tm.tile_w).round().max(1.0);

        for t in &mut mesh.tris {
            t.tex_id = Some(tid);
            let fn_v = t.face_normal;
            if is_cyl && fn_v.y.abs() < 0.72 {
                // Seamless cylindrical unwrap around Y axis (fixing atan2 seam wrap per triangle!)
                let mut us = [0.0f32; 3];
                let mut vs = [0.0f32; 3];
                let pts = [t.a, t.b, t.c];
                for k in 0..3 {
                    let ang = pts[k].x.atan2(pts[k].z); // -pi..+pi
                    let u_raw = (ang / std::f32::consts::TAU) + 0.5;
                    us[k] = u_raw;
                    vs[k] = if let Some((_, v0, _, v1, _)) = uv_spec {
                        let frac = ((bb.max.y - pts[k].y) / y_span).clamp(0.0, 1.0);
                        v0 + (v1 - v0) * frac
                    } else {
                        (bb.max.y - pts[k].y) / tm.tile_h
                    };
                }
                // Unwrap seam-straddling triangle so U increases monotonically across the 0/1 wrap
                let u_min = us[0].min(us[1]).min(us[2]);
                let u_max = us[0].max(us[1]).max(us[2]);
                if u_max - u_min > 0.5 {
                    for u in &mut us {
                        if *u < 0.5 {
                            *u += 1.0;
                        }
                    }
                }
                let u_scale = if let Some((u0, _, u1, _, is_strip)) = uv_spec {
                    if is_strip { cyl_rep } else { u1 - u0 }
                } else {
                    cyl_rep
                };
                let u_base = if let Some((u0, _, _, _, is_strip)) = uv_spec {
                    if is_strip { 0.0 } else { u0 }
                } else {
                    0.0
                };
                t.uv = finalize_uv(u_base + us[0] * u_scale, vs[0]);
                t.uv_b = finalize_uv(u_base + us[1] * u_scale, vs[1]);
                t.uv_c = finalize_uv(u_base + us[2] * u_scale, vs[2]);
            } else {
                // Triplanar / box projection with automatic band V-mapping on vertical walls!
                let ax = fn_v.x.abs();
                let ay = fn_v.y.abs();
                let az = fn_v.z.abs();
                let map_pt = |p: Vec3| -> Vec2 {
                    let (u_world, v_world, is_vert) = if ay >= ax && ay >= az {
                        // Top / bottom horizontal cap
                        let sgn = if fn_v.y >= 0.0 { 1.0 } else { -1.0 };
                        ((p.x - bb.min.x) / tm.tile_w, (sgn * (p.z - bb.min.z)) / tm.tile_h, false)
                    } else if az >= ax {
                        // +Z front / -Z back wall
                        let u_w = if fn_v.z >= 0.0 { p.x - bb.min.x } else { bb.max.x - p.x };
                        (u_w / tm.tile_w, (bb.max.y - p.y) / tm.tile_h, true)
                    } else {
                        // +X right / -X left wall
                        let u_w = if fn_v.x >= 0.0 { bb.max.z - p.z } else { p.z - bb.min.z };
                        (u_w / tm.tile_w, (bb.max.y - p.y) / tm.tile_h, true)
                    };

                    if let Some((u0, v0, u1, v1, is_strip)) = uv_spec {
                        if is_strip {
                            if is_vert {
                                let frac = ((bb.max.y - p.y) / y_span).clamp(0.0, 1.0);
                                finalize_uv(u_world, v0 + (v1 - v0) * frac)
                            } else {
                                // Map horizontal top/bottom caps cleanly inside the middle of the trim band
                                let vm = 0.5 * (v0 + v1);
                                let vh = (v1 - v0) * 0.45;
                                let zf = ((p.z - bb.min.z) / (bb.max.z - bb.min.z).max(1e-4) - 0.5) * 2.0;
                                finalize_uv(u_world, (vm + zf * vh).clamp(v0, v1))
                            }
                        } else {
                            let uf = if is_vert {
                                if az >= ax {
                                    (p.x - bb.min.x) / (bb.max.x - bb.min.x).max(1e-4)
                                } else {
                                    (p.z - bb.min.z) / (bb.max.z - bb.min.z).max(1e-4)
                                }
                            } else {
                                (p.x - bb.min.x) / (bb.max.x - bb.min.x).max(1e-4)
                            };
                            let vf = if is_vert {
                                (bb.max.y - p.y) / y_span
                            } else {
                                (p.z - bb.min.z) / (bb.max.z - bb.min.z).max(1e-4)
                            };
                            finalize_uv(u0 + (u1 - u0) * uf.clamp(0.0, 1.0), v0 + (v1 - v0) * vf.clamp(0.0, 1.0))
                        }
                    } else {
                        finalize_uv(u_world, v_world)
                    }
                };
                t.uv = map_pt(t.a);
                t.uv_b = map_pt(t.b);
                t.uv_c = map_pt(t.c);
            }
        }
        Ok(())
    }

    fn parse_vec3_list(&self, s: &str, lineno: usize) -> Result<Vec<Vec3>, String> {
        let mut out = Vec::new();
        for item in split_top_level(s, ',') {
            let parts: Vec<&str> = item.split(':').collect();
            if parts.len() != 3 {
                return Err(format!("line {lineno}: expected x:y:z triple, got '{item}'"));
            }
            let x = self.env.eval_f32(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?;
            let y = self.env.eval_f32(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
            let z = self.env.eval_f32(parts[2]).map_err(|e| format!("line {lineno}: {e}"))?;
            out.push(Vec3::new(x, y, z));
        }
        Ok(out)
    }

    /// Format per section: `x:y:z/rx:ry[/twist_deg][/mat]` or `x:y:z/r[/mat]`
    fn parse_loft_sections(
        &self,
        s: &str,
        overrides: &HashMap<String, String>,
        lineno: usize,
    ) -> Result<Vec<LoftSection>, String> {
        let mut out = Vec::new();
        for item in split_top_level(s, ',') {
            let slash: Vec<&str> = item.split('/').collect();
            if slash.len() < 2 {
                return Err(format!(
                    "line {lineno}: loft section expects x:y:z/rx:ry[/tw][/mat], got '{item}'"
                ));
            }
            let p_parts: Vec<&str> = slash[0].split(':').collect();
            if p_parts.len() != 3 {
                return Err(format!("line {lineno}: bad loft pos '{}'", slash[0]));
            }
            let pos = Vec3::new(
                self.env.eval_f32(p_parts[0]).map_err(|e| format!("line {lineno}: {e}"))?,
                self.env.eval_f32(p_parts[1]).map_err(|e| format!("line {lineno}: {e}"))?,
                self.env.eval_f32(p_parts[2]).map_err(|e| format!("line {lineno}: {e}"))?,
            );
            let r_parts: Vec<&str> = slash[1].split(':').collect();
            let (rx, ry) = match r_parts.as_slice() {
                [r] => {
                    let v = self.env.eval_f32(r).map_err(|e| format!("line {lineno}: {e}"))?;
                    (v, v)
                }
                [a, b] => (
                    self.env.eval_f32(a).map_err(|e| format!("line {lineno}: {e}"))?,
                    self.env.eval_f32(b).map_err(|e| format!("line {lineno}: {e}"))?,
                ),
                _ => return Err(format!("line {lineno}: bad loft radii '{}'", slash[1])),
            };
            let mut twist_deg = 0.0f32;
            let mut mat: Option<ResolvedMat> = None;
            for &extra in &slash[2..] {
                if extra.is_empty() {
                    continue;
                }
                if let Ok(v) = self.env.eval_f32(extra) {
                    twist_deg = v;
                } else {
                    mat = Some(self.resolve_mat_name(extra, overrides, lineno)?);
                }
            }
            out.push(LoftSection {
                pos,
                rx,
                ry,
                twist_deg,
                mat,
            });
        }
        Ok(out)
    }

    fn opt_f32(&self, flags: &CommonFlags, key: &str, def: f32, lineno: usize) -> Result<f32, String> {
        match flags.extras.get(key) {
            Some(s) => self.env.eval_f32(s).map_err(|e| format!("line {lineno}: {e}")),
            None => Ok(def),
        }
    }

    fn opt_u32(&self, flags: &CommonFlags, key: &str, def: u32, lineno: usize) -> Result<u32, String> {
        match flags.extras.get(key) {
            Some(s) => {
                let v = self.env.eval_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
                Ok(v.round().max(0.0) as u32)
            }
            None => Ok(def),
        }
    }

    fn opt_bool(&self, flags: &CommonFlags, key: &str, def: bool) -> bool {
        match flags.extras.get(key).map(|s| s.as_str()) {
            Some("1") | Some("true") | Some("yes") => true,
            Some("0") | Some("false") | Some("no") => false,
            _ => def,
        }
    }
}

fn apply_deformers(mesh: &mut Mesh, flags: &CommonFlags) {
    let smooth_any = flags.smooth || flags.weighted_normals.is_some();
    if let Some(ref tw) = flags.twist {
        mesh.apply_twist(tw.deg, tw.axis, smooth_any);
    }
    if let Some(ref bd) = flags.bend {
        mesh.apply_bend(bd.deg, bd.along, bd.toward, smooth_any);
    }
    if let Some(power) = flags.weighted_normals {
        mesh.compute_weighted_normals(power);
    }
}

fn compute_anchor_shift(mesh: &Mesh, anchor: &Anchor) -> Vec3 {
    let bb = mesh.compute_bounds();
    if !bb.is_valid() {
        return Vec3::ZERO;
    }
    match anchor {
        Anchor::Origin => Vec3::ZERO,
        Anchor::Center => -bb.center(),
        Anchor::Bottom => Vec3::new(0.0, -bb.min.y, 0.0),
        Anchor::Top => Vec3::new(0.0, -bb.max.y, 0.0),
    }
}

#[allow(clippy::too_many_arguments)]
fn emit_instanced(
    out: &mut Mesh,
    local: &Mesh,
    pos: Vec3,
    flags: &CommonFlags,
    tag: Option<&str>,
    bones: &[Bone],
    bone_map: &HashMap<String, u16>,
    lineno: usize,
) -> Result<(), String> {
    let explicit_bone = if let Some(ref bname) = flags.bone {
        let idx = *bone_map.get(bname).ok_or_else(|| {
            format!("line {lineno}: unknown bone '{bname}' (define it inside `skel ... end`)")
        })?;
        Some(idx)
    } else {
        None
    };

    let base_trs = Mat4::from_trs(pos, flags.rot, flags.scale);

    // Step 1: Collect linear repeat or radial array matrices
    let mut instances: Vec<Mat4> = Vec::new();
    if let Some(ref rep) = flags.repeat {
        for i in 0..rep.count.max(1) {
            let fi = i as f32;
            let t = Mat4::from_trs(pos + rep.step * fi, flags.rot + rep.rot_step * fi, flags.scale);
            instances.push(t);
        }
    } else if let Some(ref rad) = flags.radial {
        let n = rad.count.max(1);
        for i in 0..n {
            let ang = rad.start_deg + 360.0 * (i as f32) / (n as f32);
            let a = ang.to_radians();
            let (s, c) = a.sin_cos();
            let (offset, rot_mat) = match rad.axis {
                'x' => (Vec3::new(0.0, rad.radius * c, -rad.radius * s), Mat4::rot_x(ang)),
                'z' => (Vec3::new(rad.radius * c, rad.radius * s, 0.0), Mat4::rot_z(ang)),
                _ => (Vec3::new(rad.radius * c, 0.0, -rad.radius * s), Mat4::rot_y(ang)),
            };
            let m = Mat4::translation(pos + offset)
                .mul(rot_mat)
                .mul(Mat4::from_trs(Vec3::ZERO, flags.rot, flags.scale));
            instances.push(m);
        }
    } else {
        instances.push(base_trs);
    }

    // Step 2: Compose mirror reflections across parent origin planes (tracking X mirror for L/R bone swapping!)
    let mut mirror_mats: Vec<(Mat4, bool)> = vec![(Mat4::IDENTITY, false)];
    for &ax in &flags.mirror {
        let (refl, is_x) = match ax {
            'x' => (Mat4::scale(Vec3::new(-1.0, 1.0, 1.0)), true),
            'y' => (Mat4::scale(Vec3::new(1.0, -1.0, 1.0)), false),
            'z' => (Mat4::scale(Vec3::new(1.0, 1.0, -1.0)), false),
            _ => continue,
        };
        let cur = mirror_mats.clone();
        for (mm, mx) in cur {
            mirror_mats.push((refl.mul(mm), mx ^ is_x));
        }
    }

    for (mm, flip_x) in &mirror_mats {
        for inst in &instances {
            let start_len = out.tris.len();
            out.append_transformed(local, mm.mul(*inst), tag);
            for t in &mut out.tris[start_len..] {
                if let Some(eb) = explicit_bone {
                    t.rigid_bone = Some(eb);
                }
                if *flip_x {
                    if let Some(bi) = t.rigid_bone {
                        if let Some(b) = bones.get(bi as usize) {
                            let mir_name = mirror_bone_name(&b.name);
                            if let Some(&mbi) = bone_map.get(&mir_name) {
                                t.rigid_bone = Some(mbi);
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

fn parse_flags(tokens: &[String], env: &ExprEnv, lineno: usize) -> Result<CommonFlags, String> {
    let mut f = CommonFlags::default();
    for tok in tokens {
        let Some((k, v)) = tok.split_once('=') else {
            continue;
        };
        match k {
            "r" | "rot" => {
                let vals = env.eval_list_f32(v).map_err(|e| format!("line {lineno}: {e}"))?;
                if vals.len() != 3 {
                    return Err(format!("line {lineno}: rot expects rx,ry,rz"));
                }
                f.rot = Vec3::new(vals[0], vals[1], vals[2]);
            }
            "s" | "scale" => {
                let vals = env.eval_list_f32(v).map_err(|e| format!("line {lineno}: {e}"))?;
                f.scale = match vals.as_slice() {
                    [u] => Vec3::new(*u, *u, *u),
                    [x, y, z] => Vec3::new(*x, *y, *z),
                    _ => return Err(format!("line {lineno}: scale expects scalar or sx,sy,sz")),
                };
            }
            "a" | "anchor" => {
                f.anchor = Some(match v.to_ascii_lowercase().as_str() {
                    "c" | "center" => Anchor::Center,
                    "b" | "bot" | "bottom" => Anchor::Bottom,
                    "t" | "top" => Anchor::Top,
                    "o" | "origin" => Anchor::Origin,
                    _ => return Err(format!("line {lineno}: unknown anchor '{v}' (use c|b|t|o)")),
                });
            }
            "m" | "mirror" => {
                for ch in v.to_ascii_lowercase().chars() {
                    if (ch == 'x' || ch == 'y' || ch == 'z') && !f.mirror.contains(&ch) {
                        f.mirror.push(ch);
                    }
                }
            }
            "sm" | "smooth" => {
                let vl = v.to_ascii_lowercase();
                if matches!(vl.as_str(), "w" | "wn" | "weighted") {
                    f.weighted_normals = Some(4.0);
                    f.smooth = true;
                } else {
                    f.smooth = vl == "1" || vl == "true";
                }
            }
            "wn" | "weighted" => {
                let vl = v.to_ascii_lowercase();
                if vl == "0" || vl == "false" || vl == "no" {
                    f.weighted_normals = None;
                } else if vl == "1" || vl == "true" || vl == "yes" {
                    f.weighted_normals = Some(4.0);
                } else {
                    let p = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?;
                    if p > 0.0 {
                        f.weighted_normals = Some(p.clamp(1.0, 12.0));
                    } else {
                        f.weighted_normals = None;
                    }
                }
            }
            "bone" | "jnt" => {
                f.bone = Some(v.strip_prefix('@').unwrap_or(v).to_string());
            }
            "bend" => {
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() < 2 {
                    return Err(format!("line {lineno}: bend expects deg:axis[:toward]"));
                }
                let deg = env.eval_f32(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?;
                let along = parts[1].chars().next().unwrap_or('y').to_ascii_lowercase();
                let toward = parts
                    .get(2)
                    .and_then(|s| s.chars().next())
                    .map(|c| c.to_ascii_lowercase())
                    .unwrap_or_else(|| if along == 'z' { 'y' } else { 'z' });
                f.bend = Some(BendSpec { deg, along, toward });
            }
            "twist" => {
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() < 2 {
                    return Err(format!("line {lineno}: twist expects deg:axis"));
                }
                let deg = env.eval_f32(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?;
                let axis = parts[1].chars().next().unwrap_or('y').to_ascii_lowercase();
                f.twist = Some(TwistSpec { deg, axis });
            }
            "rep" => {
                // rep=N:dx,dy,dz[:rx,ry,rz]
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() < 2 {
                    return Err(format!("line {lineno}: rep expects N:dx,dy,dz[:rx,ry,rz]"));
                }
                let count = env.eval_f32(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?.round().max(1.0) as usize;
                let step_v = env.eval_list_f32(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
                if step_v.len() != 3 {
                    return Err(format!("line {lineno}: rep step expects dx,dy,dz"));
                }
                let rot_step = if let Some(rs) = parts.get(2) {
                    let rv = env.eval_list_f32(rs).map_err(|e| format!("line {lineno}: {e}"))?;
                    if rv.len() != 3 {
                        return Err(format!("line {lineno}: rep rotStep expects rx,ry,rz"));
                    }
                    Vec3::new(rv[0], rv[1], rv[2])
                } else {
                    Vec3::ZERO
                };
                f.repeat = Some(RepeatSpec {
                    count,
                    step: Vec3::new(step_v[0], step_v[1], step_v[2]),
                    rot_step,
                });
            }
            "rad" => {
                // rad=N:R[:axis][:startDeg]
                let parts: Vec<&str> = v.split(':').collect();
                if parts.len() < 2 {
                    return Err(format!("line {lineno}: rad expects N:R[:axis][:startDeg]"));
                }
                let count = env.eval_f32(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?.round().max(1.0) as usize;
                let radius = env.eval_f32(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
                let axis = parts
                    .get(2)
                    .and_then(|s| s.chars().next())
                    .map(|c| c.to_ascii_lowercase())
                    .unwrap_or('y');
                let start_deg = if let Some(sd) = parts.get(3) {
                    env.eval_f32(sd).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    0.0
                };
                f.radial = Some(RadialSpec {
                    count,
                    radius,
                    axis,
                    start_deg,
                });
            }
            "fc" => {
                for pair in v.split(',') {
                    if let Some((fk, mv)) = pair.split_once(':') {
                        f.face_colors.push((fk.to_ascii_lowercase(), mv.to_string()));
                    }
                }
            }
            "ov" => {
                for pair in v.split(',') {
                    if let Some((old_m, new_m)) = pair.split_once(':') {
                        f.color_overrides
                            .insert(old_m.to_ascii_lowercase(), new_m.to_string());
                    }
                }
            }
            _ => {
                f.extras.insert(k.to_string(), v.to_string());
            }
        }
    }
    Ok(f)
}

fn collect_block(
    lines: &[RawLine],
    start: usize,
    open_lineno: usize,
    kind: &str,
) -> Result<(Vec<RawLine>, usize), String> {
    let mut body = Vec::new();
    let mut depth = 1usize;
    let mut i = start;
    while i < lines.len() {
        let op = lines[i].tokens[0].to_ascii_lowercase();
        if matches!(op.as_str(), "def" | "grp" | "col" | "node" | "anim" | "action" | "skel" | "rig" | "morph" | "shape" | "csg" | "sdf" | "meta") {
            depth += 1;
            body.push(lines[i].clone());
        } else if op == "end" {
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
            body.push(lines[i].clone());
        } else {
            body.push(lines[i].clone());
        }
        i += 1;
    }
    Err(format!("unclosed '{kind}' block opened at line {open_lineno}"))
}

fn parse_var_line(tokens: &[String], env: &mut ExprEnv, lineno: usize) -> Result<(), String> {
    // Handles `$a=1.5 $b=$a*2` or `$a = 1.5 + 2`
    let joined = tokens.join(" ");
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for tok in tokens {
        if tok.starts_with('$') && tok.contains('=') && !cur.is_empty() {
            chunks.push(cur);
            cur = tok.clone();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(tok);
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    let _ = joined;
    for chunk in chunks {
        let Some((lhs, rhs)) = chunk.split_once('=') else {
            return Err(format!("line {lineno}: invalid variable assignment '{chunk}'"));
        };
        let name = lhs.trim();
        let val = env
            .eval(rhs.trim())
            .map_err(|e| format!("line {lineno}: {e}"))?;
        env.set(name, val);
    }
    Ok(())
}

fn take_tag(args: &[String], idx: &mut usize) -> Option<String> {
    if let Some(tok) = args.get(*idx) {
        if let Some(name) = tok.strip_prefix('@') {
            *idx += 1;
            return Some(name.to_string());
        }
    }
    None
}

fn is_flag_token(tok: &str) -> bool {
    if let Some((k, _)) = tok.split_once('=') {
        !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
    } else {
        false
    }
}

fn is_mesh_op(tok: &str) -> bool {
    matches!(
        tok.to_ascii_lowercase().as_str(),
        "box" | "cube" | "cyl" | "cone" | "cap" | "wdg" | "wedge" | "pyr" | "pyramid"
        | "ico" | "sphere" | "tor" | "torus" | "pri" | "prism" | "lat" | "lathe"
        | "tub" | "tube" | "lof" | "loft" | "ter" | "terrain" | "tri" | "quad" | "rib" | "ribbon" | "use" | "grp" | "node"
    )
}

fn is_sdf_prim(tok: &str) -> bool {
    matches!(
        tok.to_ascii_lowercase().as_str(),
        "sph" | "sphere" | "ell" | "ellipsoid" | "cap" | "capsule"
        | "box" | "cube" | "cyl" | "cylinder" | "tor" | "torus"
    )
}

fn strip_mdl_comment(line: &str) -> &str {
    if let Some(i) = line.find('#') {
        &line[..i]
    } else {
        line
    }
}

fn tokenize_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in line.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch.is_whitespace() && !in_quotes {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

fn split_top_level(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let mut start = 0usize;
    let mut depth = 0i32;
    for (i, ch) in s.char_indices() {
        match ch {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    if start <= s.len() {
        out.push(&s[start..]);
    }
    out
}

fn transform_quad_uvs(
    ua: Vec2,
    ub: Vec2,
    uc: Vec2,
    ud: Vec2,
    flags: &CommonFlags,
    env: &ExprEnv,
    lineno: usize,
) -> Result<(Vec2, Vec2, Vec2, Vec2), String> {
    let uvs = match flags.extras.get("uvs") {
        Some(s) => {
            let v = env.eval_list_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
            match v.as_slice() {
                [a] => Vec2::new(*a, *a),
                [a, b] => Vec2::new(*a, *b),
                _ => return Err(format!("line {lineno}: bad uvs='{s}'")),
            }
        }
        None => Vec2::new(1.0, 1.0),
    };
    let uvo = match flags.extras.get("uvo") {
        Some(s) => {
            let v = env.eval_list_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
            if v.len() != 2 {
                return Err(format!("line {lineno}: bad uvo='{s}'"));
            }
            Vec2::new(v[0], v[1])
        }
        None => Vec2::ZERO,
    };
    let uvr = flags
        .extras
        .get("uvr")
        .map(|s| env.eval_f32(s))
        .transpose()
        .map_err(|e| format!("line {lineno}: {e}"))?
        .unwrap_or(0.0);
    let rot_steps = ((uvr / 90.0).round() as i32).rem_euclid(4);
    let mut pts = [ua, ub, uc, ud];
    for _ in 0..rot_steps {
        pts = [pts[3], pts[0], pts[1], pts[2]];
    }
    for p in &mut pts {
        p.x = p.x * uvs.x + uvo.x;
        p.y = p.y * uvs.y + uvo.y;
    }
    Ok((pts[0], pts[1], pts[2], pts[3]))
}

fn rgba_to_rgb(rgba: &[u8]) -> Vec<u8> {
    let mut rgb = Vec::with_capacity(rgba.len() / 4 * 3);
    for px in rgba.chunks_exact(4) {
        rgb.push(px[0]);
        rgb.push(px[1]);
        rgb.push(px[2]);
    }
    rgb
}

fn load_tex_material_for_mdl(
    name: &str,
    path: &Path,
    res_override: Option<u32>,
) -> Result<TexMaterial, String> {
    let is_tex_dsl = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("tex"))
        .unwrap_or(false);

    if is_tex_dsl {
        let mut spec = load_tex_file(path)?;
        if let Some(r) = res_override {
            spec.width = r;
            spec.height = r;
        }
        let baked = bake_texture(&spec, None)?;
        let bc_png = baked.encode_basecolor_png();
        let nm_png = baked.encode_normal_png();
        let orm_png = baked.encode_orm_png();

        let mut bc_rgba = Vec::with_capacity(baked.width * baked.height * 4);
        for &c in &baked.albedo {
            let enc = |v: f32| (v.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0).round().clamp(0.0, 255.0) as u8;
            bc_rgba.push(enc(c[0]));
            bc_rgba.push(enc(c[1]));
            bc_rgba.push(enc(c[2]));
            bc_rgba.push(255);
        }
        let mut nm_rgb = Vec::with_capacity(baked.width * baked.height * 3);
        for &n in &baked.normal {
            let enc = |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            nm_rgb.push(enc(n[0]));
            nm_rgb.push(enc(n[1]));
            nm_rgb.push(enc(n[2].max(0.0)));
        }
        let mut orm_rgb = Vec::with_capacity(baked.width * baked.height * 3);
        for i in 0..(baked.width * baked.height) {
            orm_rgb.push((baked.ao[i].clamp(0.0, 1.0) * 255.0).round() as u8);
            orm_rgb.push((baked.roughness[i].clamp(0.0, 1.0) * 255.0).round() as u8);
            orm_rgb.push((baked.metallic[i].clamp(0.0, 1.0) * 255.0).round() as u8);
        }

        return Ok(TexMaterial {
            name: name.to_string(),
            width: baked.width as u32,
            height: baked.height as u32,
            tile_w: baked.tile_w,
            tile_h: baked.tile_h,
            basecolor_rgba: bc_rgba,
            normal_rgb: Some(nm_rgb),
            orm_rgb: Some(orm_rgb),
            basecolor_png: bc_png,
            normal_png: Some(nm_png),
            orm_png: Some(orm_png),
        });
    }

    // Load pre-baked PNG set (`<prefix>_basecolor.png`, optional `_normal.png`, `_orm.png`) or standalone `.png`
    let p_str = path.to_string_lossy();
    let (bc_path, nm_path, orm_path) = if p_str.ends_with(".png") {
        let pref = p_str
            .strip_suffix("_basecolor.png")
            .or_else(|| p_str.strip_suffix(".png"))
            .unwrap_or(&p_str);
        (
            path.to_path_buf(),
            PathBuf::from(format!("{pref}_normal.png")),
            PathBuf::from(format!("{pref}_orm.png")),
        )
    } else {
        (
            PathBuf::from(format!("{p_str}_basecolor.png")),
            PathBuf::from(format!("{p_str}_normal.png")),
            PathBuf::from(format!("{p_str}_orm.png")),
        )
    };

    let bc_png = std::fs::read(&bc_path)
        .map_err(|e| format!("failed to read texture basecolor '{}': {e}", bc_path.display()))?;
    let bc_dec = decode_png(&bc_png)?;

    let (nm_png, nm_rgb) = if nm_path.exists() {
        let bytes = std::fs::read(&nm_path)
            .map_err(|e| format!("failed to read normal map '{}': {e}", nm_path.display()))?;
        let dec = decode_png(&bytes)?;
        (Some(bytes), Some(rgba_to_rgb(&dec.rgba)))
    } else {
        (None, None)
    };

    let (orm_png, orm_rgb) = if orm_path.exists() {
        let bytes = std::fs::read(&orm_path)
            .map_err(|e| format!("failed to read ORM map '{}': {e}", orm_path.display()))?;
        let dec = decode_png(&bytes)?;
        (Some(bytes), Some(rgba_to_rgb(&dec.rgba)))
    } else {
        (None, None)
    };

    Ok(TexMaterial {
        name: name.to_string(),
        width: bc_dec.width,
        height: bc_dec.height,
        tile_w: 2.0,
        tile_h: 2.0,
        basecolor_rgba: bc_dec.rgba,
        normal_rgb: nm_rgb,
        orm_rgb,
        basecolor_png: bc_png,
        normal_png: nm_png,
        orm_png,
    })
}
