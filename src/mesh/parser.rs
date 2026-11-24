use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::common::expr::ExprEnv;
use crate::common::math::{Mat4, Vec2, Vec3};
use crate::mesh::csg::{apply_csg, CsgOp};
use crate::mesh::geom::{ColliderMesh, Mesh};
use crate::mesh::palette::{Palette, ResolvedMat};
use crate::mesh::primitives::{
    build_box, build_capsule, build_cylinder, build_icosphere, build_lathe, build_loft,
    build_noisy_plane, build_prism, build_pyramid, build_torus, build_tube, build_wedge, FaceMats,
    LoftSection,
};
use crate::mesh::sdf::{build_sdf_mesh, SdfOp, SdfPrim, SdfShape};

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
    let text = std::fs::read_to_string(mdl_path)
        .map_err(|e| format!("failed to read model '{}': {e}", mdl_path.display()))?;
    let pal_rel = peek_palette_path(&text);
    let pal_path = if let Some(ov) = palette_override {
        ov.to_path_buf()
    } else if let Some(ref rel) = pal_rel {
        mdl_path.parent().unwrap_or_else(|| Path::new(".")).join(rel)
    } else {
        return Err(format!(
            "model '{}' has no 'pal <path>' directive and no -p palette flag was provided",
            mdl_path.display()
        ));
    };
    let palette = Palette::load_file(&pal_path)?;
    let name = mdl_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("model")
        .to_string();
    let parsed = compile_model(&text, &name, &palette)?;
    Ok((parsed, palette, pal_path))
}

pub fn compile_model(text: &str, name: &str, palette: &Palette) -> Result<ParsedModel, String> {
    let mut env = ExprEnv::new();
    let mut raw_lines = Vec::new();
    let mut pal_rel = None;

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
        // Allow one or multiple `$var=expr` assignments on a single line
        if tokens[0].starts_with('$') {
            parse_var_line(&tokens, &mut env, lineno)?;
            continue;
        }
        raw_lines.push(RawLine { lineno, tokens });
    }

    // Separate top-level `def <name> ... end` blocks from root statements
    let mut defs: HashMap<String, Vec<RawLine>> = HashMap::new();
    let mut root_lines = Vec::new();
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
        } else {
            root_lines.push(raw_lines[i].clone());
            i += 1;
        }
    }

    let mut ctx = EvalCtx {
        palette,
        defs: &defs,
        env,
        part_count: 0,
    };
    let overrides = HashMap::new();
    let mesh = ctx.eval_lines(&root_lines, None, &overrides, 0)?;

    Ok(ParsedModel {
        name: name.to_string(),
        palette_rel: pal_rel,
        mesh,
        part_count: ctx.part_count,
    })
}

struct EvalCtx<'a> {
    palette: &'a Palette,
    defs: &'a HashMap<String, Vec<RawLine>>,
    env: ExprEnv,
    part_count: usize,
}

impl<'a> EvalCtx<'a> {
    fn resolve_mat_name(
        &self,
        raw: &str,
        overrides: &HashMap<String, String>,
        lineno: usize,
    ) -> Result<ResolvedMat, String> {
        let mapped = overrides
            .get(raw)
            .or_else(|| overrides.get(&raw.to_ascii_lowercase()))
            .map(|s| s.as_str())
            .unwrap_or(raw);
        self.palette
            .resolve_mat(mapped)
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
                emit_instanced(&mut placed, &child_mesh, pos, &flags, None);
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
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref());
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
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref());
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
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref());
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
                emit_instanced(&mut out, &child_mesh, pos, &flags, tag.as_deref());
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
                emit_instanced(&mut placed, &m, pos, &flags, None);
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
                emit_instanced(&mut placed, &m, pos, &flags, None);
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

        // First positional arg is material name unless omitted (only when inherited_mat exists & arg0 looks numeric/vec)
        let (mat_str, rest) = if pos_args.is_empty() {
            return Err(format!("line {lineno}: primitive '{op}' missing arguments"));
        } else if inherited_mat.is_some()
            && self.palette.resolve_mat(&pos_args[0]).is_err()
            && !pos_args[0].starts_with("cell:")
            && !pos_args[0].starts_with("uv:")
            && pos_args[0].contains(',')
        {
            (inherited_mat.unwrap(), &pos_args[..])
        } else {
            (pos_args[0].as_str(), &pos_args[1..])
        };

        let mat = self.resolve_mat_name(mat_str, overrides, lineno)?;
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
                let mut m = Mesh::new();
                m.add_tri(p0, p1, p2, mat);
                m
            }
            _ => {
                return Err(format!("line {lineno}: unknown opcode '{op}'"));
            }
        };

        if flags.smooth {
            local.compute_smooth_normals();
        }
        apply_deformers(&mut local, &flags);

        // Apply anchor in local primitive space BEFORE rotation/scale/translation
        let anchor = flags.anchor.as_ref().unwrap_or(&default_anchor);
        let shift = compute_anchor_shift(&local, anchor);
        if shift.len_sq() > 1e-10 {
            local.translate(shift);
        }

        emit_instanced(out, &local, pos, &flags, tag.as_deref());
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
    if let Some(ref tw) = flags.twist {
        mesh.apply_twist(tw.deg, tw.axis, flags.smooth);
    }
    if let Some(ref bd) = flags.bend {
        mesh.apply_bend(bd.deg, bd.along, bd.toward, flags.smooth);
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

fn emit_instanced(out: &mut Mesh, local: &Mesh, pos: Vec3, flags: &CommonFlags, tag: Option<&str>) {
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

    // Step 2: Compose mirror reflections across parent origin planes
    let mut mirror_mats = vec![Mat4::IDENTITY];
    for &ax in &flags.mirror {
        let refl = match ax {
            'x' => Mat4::scale(Vec3::new(-1.0, 1.0, 1.0)),
            'y' => Mat4::scale(Vec3::new(1.0, -1.0, 1.0)),
            'z' => Mat4::scale(Vec3::new(1.0, 1.0, -1.0)),
            _ => continue,
        };
        let cur = mirror_mats.clone();
        for mm in cur {
            mirror_mats.push(refl.mul(mm));
        }
    }

    for mm in &mirror_mats {
        for inst in &instances {
            out.append_transformed(local, mm.mul(*inst), tag);
        }
    }
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
                f.smooth = v == "1" || v.eq_ignore_ascii_case("true");
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
        if op == "def" || op == "grp" || op == "col" || op == "csg" || op == "sdf" || op == "meta" {
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
        | "tub" | "tube" | "lof" | "loft" | "ter" | "terrain" | "tri" | "use" | "grp"
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
