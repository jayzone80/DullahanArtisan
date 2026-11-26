use std::path::{Path, PathBuf};

use crate::common::expr::ExprEnv;
use crate::common::math::parse_hex_rgba;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WrapMode {
    XY,
    X,
    Y,
    None,
}

impl WrapMode {
    pub fn wrap_x(self) -> bool {
        matches!(self, WrapMode::XY | WrapMode::X)
    }
    pub fn wrap_y(self) -> bool {
        matches!(self, WrapMode::XY | WrapMode::Y)
    }
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "xy" | "both" | "tile" | "1" | "true" => Some(WrapMode::XY),
            "x" | "h" | "horiz" | "trim" => Some(WrapMode::X),
            "y" | "v" | "vert" => Some(WrapMode::Y),
            "none" | "0" | "false" | "off" => Some(WrapMode::None),
            _ => None,
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            WrapMode::XY => "xy",
            WrapMode::X => "x",
            WrapMode::Y => "y",
            WrapMode::None => "none",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NormalConvention {
    OpenGL,  // +Y up (Godot, Blender, Three.js, glTF 2.0)
    DirectX, // -Y down (Unreal Engine, Direct3D)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerMask {
    All,
    Cavity,
    Edge,
    Up,
    High,
    Low,
}

impl LayerMask {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "all" | "none" | "-" => Some(LayerMask::All),
            "cavity" | "cav" | "mortar" | "crevice" => Some(LayerMask::Cavity),
            "edge" | "edges" | "convex" | "wear" => Some(LayerMask::Edge),
            "up" | "top" | "ledge" => Some(LayerMask::Up),
            "high" | "raised" | "stone" => Some(LayerMask::High),
            "low" | "recess" | "deep" => Some(LayerMask::Low),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlendMode {
    Over,
    Multiply,
    Add,
    Screen,
    HeightOnly,
}

impl BlendMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "over" | "normal" | "alpha" => Some(BlendMode::Over),
            "mul" | "multiply" => Some(BlendMode::Multiply),
            "add" | "plus" => Some(BlendMode::Add),
            "screen" => Some(BlendMode::Screen),
            "height" | "bump" | "carve" => Some(BlendMode::HeightOnly),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct StonesSpec {
    pub rows: usize,
    pub cols: usize,
    pub mats: Vec<String>,
    pub mortar: Option<String>,
    pub gap: f32,
    pub bevel: f32,
    pub h_min: f32,
    pub h_max: f32,
    pub tilt_deg: f32,
    pub stagger: f32,
    pub jitter_x: f32,
    pub jitter_y: f32,
    pub y_band: Option<(f32, f32)>,
    pub vary: f32,
    pub seed: u64,
}

#[derive(Debug, Clone)]
pub enum TexLayer {
    Grain {
        scale: f32,
        amp: f32,
        rough: f32,
        bump: f32,
        mask: LayerMask,
        seed: u64,
    },
    EdgeWear {
        color: [f32; 3],
        strength: f32,
        threshold: f32,
        chip: f32,
        rough: f32,
        bump: f32,
        scale: f32,
        seed: u64,
    },
    Cavity {
        color: [f32; 3],
        strength: f32,
        depth: f32,
        rough: f32,
    },
    Moss {
        color: [f32; 3],
        alt_color: [f32; 3],
        strength: f32,
        coverage: f32,
        up_weight: f32,
        cavity_weight: f32,
        y_band: Option<(f32, f32)>,
        rough: f32,
        bump: f32,
        scale: f32,
        seed: u64,
    },
    Svg {
        rel_path: String,
        blend: BlendMode,
        opacity: f32,
        mask: LayerMask,
        rough: Option<f32>,
        metal: Option<f32>,
        height: f32,
        blur_px: f32,
        ao_strength: f32,
        wrap: Option<WrapMode>,
    },
}

#[derive(Debug, Clone)]
pub struct TexSpec {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub tile_w: f32,
    pub tile_h: f32,
    pub depth: f32,
    pub wrap: WrapMode,
    pub normal_conv: NormalConvention,
    pub ao_radius: f32,
    pub ao_strength: f32,
    pub vary: f32,
    pub seed: u64,
    pub palette_rel: Option<String>,
    pub base_dir: PathBuf,
    pub mesh_rel: Option<String>,
    pub inline_mdl: Option<String>,
    pub var_lines: Vec<String>,
    pub stones: Vec<StonesSpec>,
    pub layers: Vec<TexLayer>,
}

fn srgb_hex_to_linear(hex: &str) -> Result<[f32; 3], String> {
    let rgba = parse_hex_rgba(hex)?;
    let to_lin = |u: u8| (u as f32 / 255.0).powf(2.2);
    Ok([to_lin(rgba[0]), to_lin(rgba[1]), to_lin(rgba[2])])
}

fn parse_range_f32(s: &str, env: &ExprEnv) -> Result<(f32, f32), String> {
    if let Some((a, b)) = s.split_once("..") {
        let va = env.eval_f32(a.trim())?;
        let vb = env.eval_f32(b.trim())?;
        Ok((va.min(vb), va.max(vb)))
    } else {
        let v = env.eval_f32(s.trim())?;
        Ok((v, v))
    }
}

fn is_hex_color_start(rest: &str) -> bool {
    let tok = rest.split_whitespace().next().unwrap_or("");
    matches!(tok.len(), 3 | 4 | 6 | 8) && tok.chars().all(|c| c.is_ascii_hexdigit())
}

fn tokenize_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quote = false;
    let mut chars = line.char_indices().peekable();
    while let Some((byte_idx, ch)) = chars.next() {
        if ch == '"' {
            in_quote = !in_quote;
            continue;
        }
        if !in_quote && ch == '#' && cur.is_empty() {
            let after = &line[byte_idx + 1..];
            if !is_hex_color_start(after) {
                break;
            }
        }
        if !in_quote && ch.is_whitespace() {
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

pub fn load_tex_file(path: &Path) -> Result<TexSpec, String> {
    let src = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    let default_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("texture")
        .to_string();
    let base_dir = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
    parse_tex(&src, &default_name, &base_dir)
}

pub fn parse_tex(src: &str, default_name: &str, base_dir: &Path) -> Result<TexSpec, String> {
    let mut spec = TexSpec {
        name: default_name.to_string(),
        width: 1024,
        height: 1024,
        tile_w: 2.0,
        tile_h: 2.0,
        depth: 0.20,
        wrap: WrapMode::XY,
        normal_conv: NormalConvention::OpenGL,
        ao_radius: 0.14,
        ao_strength: 1.0,
        vary: 0.05,
        seed: 42,
        palette_rel: None,
        base_dir: base_dir.to_path_buf(),
        mesh_rel: None,
        inline_mdl: None,
        var_lines: Vec::new(),
        stones: Vec::new(),
        layers: Vec::new(),
    };

    let mut env = ExprEnv::new();
    let raw_lines: Vec<&str> = src.lines().collect();
    let mut i = 0usize;

    while i < raw_lines.len() {
        let lineno = i + 1;
        let trimmed = raw_lines[i].trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            i += 1;
            continue;
        }
        let tokens = tokenize_line(trimmed);
        if tokens.is_empty() {
            i += 1;
            continue;
        }
        let op = tokens[0].to_ascii_lowercase();

        if op.starts_with('$') {
            spec.var_lines.push(trimmed.to_string());
            let joined = tokens.join(" ");
            if let Some((lhs, rhs)) = joined.split_once('=') {
                let vname = lhs.trim().trim_start_matches('$');
                let val = env.eval_f32(rhs.trim()).map_err(|e| format!("line {lineno}: {e}"))?;
                env.set(vname, val as f64);
            }
            i += 1;
            continue;
        }

        match op.as_str() {
            "tex" | "texture" | "mat" => {
                let mut idx = 1usize;
                if idx < tokens.len() && !tokens[idx].contains('=') {
                    spec.name = tokens[idx].clone();
                    idx += 1;
                }
                for tok in &tokens[idx..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "size" | "res" => {
                                if let Some((ws, hs)) = v.split_once('x').or_else(|| v.split_once('X')) {
                                    spec.width = ws.parse().map_err(|_| format!("line {lineno}: bad size '{v}'"))?;
                                    spec.height = hs.parse().map_err(|_| format!("line {lineno}: bad size '{v}'"))?;
                                } else {
                                    let s: u32 = v.parse().map_err(|_| format!("line {lineno}: bad size '{v}'"))?;
                                    spec.width = s;
                                    spec.height = s;
                                }
                            }
                            "tile" | "world" => {
                                if let Some((ws, hs)) = v.split_once(',') {
                                    spec.tile_w = env.eval_f32(ws).map_err(|e| format!("line {lineno}: {e}"))?;
                                    spec.tile_h = env.eval_f32(hs).map_err(|e| format!("line {lineno}: {e}"))?;
                                } else {
                                    let s = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?;
                                    spec.tile_w = s;
                                    spec.tile_h = s;
                                }
                            }
                            "depth" | "d" | "z" => {
                                spec.depth = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(0.005);
                            }
                            "wrap" => {
                                spec.wrap = WrapMode::parse(v)
                                    .ok_or_else(|| format!("line {lineno}: unknown wrap '{v}' (use xy|x|y|none)"))?;
                            }
                            "normal" | "norm" => {
                                spec.normal_conv = match v.to_ascii_lowercase().as_str() {
                                    "gl" | "opengl" | "+y" => NormalConvention::OpenGL,
                                    "dx" | "directx" | "ue" | "unreal" | "-y" => NormalConvention::DirectX,
                                    _ => return Err(format!("line {lineno}: unknown normal '{v}' (use gl|dx)")),
                                };
                            }
                            "ao_rad" | "aorad" => {
                                spec.ao_radius = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(0.01);
                            }
                            "ao_str" | "ao" => {
                                spec.ao_strength = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 2.5);
                            }
                            "vary" | "var" => {
                                spec.vary = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.5);
                            }
                            "seed" => {
                                spec.seed = v.parse().map_err(|_| format!("line {lineno}: bad seed '{v}'"))?;
                            }
                            _ => return Err(format!("line {lineno}: unknown tex option '{tok}'")),
                        }
                    }
                }
            }
            "pal" | "palette" => {
                if tokens.len() < 2 {
                    return Err(format!("line {lineno}: pal requires <rel.pal>"));
                }
                spec.palette_rel = Some(tokens[1].clone());
            }
            "mesh" | "mdl" => {
                if tokens.len() >= 2 && !tokens[1].contains('=') {
                    spec.mesh_rel = Some(tokens[1].clone());
                } else {
                    // Collect inline .mdl block until matching `end` (accounting for nested .mdl blocks!)
                    let mut depth = 1usize;
                    let mut body = Vec::new();
                    i += 1;
                    while i < raw_lines.len() {
                        let lt = raw_lines[i].trim();
                        let first = lt
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        if matches!(
                            first.as_str(),
                            "def" | "grp" | "node" | "col" | "csg" | "sdf" | "meta" | "skel" | "rig" | "anim" | "action" | "morph" | "shape"
                        ) {
                            depth += 1;
                        } else if first == "end" {
                            depth -= 1;
                            if depth == 0 {
                                break;
                            }
                        }
                        body.push(raw_lines[i]);
                        i += 1;
                    }
                    if depth != 0 {
                        return Err(format!("line {lineno}: unclosed `mesh ... end` block"));
                    }
                    let existing = spec.inline_mdl.get_or_insert_with(String::new);
                    if !existing.is_empty() {
                        existing.push('\n');
                    }
                    existing.push_str(&body.join("\n"));
                }
            }
            "stones" | "bricks" => {
                let mut st = StonesSpec {
                    rows: 6,
                    cols: 4,
                    mats: Vec::new(),
                    mortar: None,
                    gap: 0.032,
                    bevel: 0.022,
                    h_min: 0.085,
                    h_max: 0.135,
                    tilt_deg: 3.0,
                    stagger: 0.5,
                    jitter_x: 0.14,
                    jitter_y: 0.08,
                    y_band: None,
                    vary: spec.vary.max(0.06),
                    seed: spec.seed.wrapping_add(101),
                };
                for tok in &tokens[1..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "rows" | "r" => st.rows = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.round().max(1.0) as usize,
                            "cols" | "c" => st.cols = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.round().max(1.0) as usize,
                            "mats" | "mat" => {
                                st.mats = v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
                            }
                            "mortar" | "bg" => st.mortar = Some(v.to_string()),
                            "gap" | "g" => st.gap = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(0.0),
                            "bev" | "bevel" => st.bevel = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(0.0),
                            "h" | "height" => {
                                let (a, b) = parse_range_f32(v, &env).map_err(|e| format!("line {lineno}: {e}"))?;
                                st.h_min = a.max(0.005);
                                st.h_max = b.max(st.h_min);
                            }
                            "tilt" => st.tilt_deg = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 20.0),
                            "stagger" | "stag" => st.stagger = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "jx" => st.jitter_x = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.45),
                            "jy" => st.jitter_y = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.45),
                            "y" | "band" => {
                                st.y_band = Some(parse_range_f32(v, &env).map_err(|e| format!("line {lineno}: {e}"))?);
                            }
                            "vary" => st.vary = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.5),
                            "seed" => st.seed = v.parse().map_err(|_| format!("line {lineno}: bad seed"))?,
                            _ => return Err(format!("line {lineno}: unknown stones flag '{tok}'")),
                        }
                    }
                }
                if st.mats.is_empty() {
                    return Err(format!("line {lineno}: stones requires mats=mat1[,mat2,...]"));
                }
                spec.stones.push(st);
            }
            "grain" | "noise" => {
                let mut scale = 16.0f32;
                let mut amp = 0.06f32;
                let mut rough = 0.08f32;
                let mut bump = 0.004f32;
                let mut mask = LayerMask::All;
                let mut seed = spec.seed.wrapping_add(11);
                for tok in &tokens[1..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "scale" | "freq" => scale = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(1.0),
                            "amp" | "str" => amp = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.6),
                            "rough" | "r" => rough = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.6),
                            "bump" | "h" => bump = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "mask" => mask = LayerMask::parse(v).ok_or_else(|| format!("line {lineno}: unknown mask '{v}'"))?,
                            "seed" => seed = v.parse().map_err(|_| format!("line {lineno}: bad seed"))?,
                            _ => return Err(format!("line {lineno}: unknown grain flag '{tok}'")),
                        }
                    }
                }
                spec.layers.push(TexLayer::Grain { scale, amp, rough, bump, mask, seed });
            }
            "edge" | "edge_wear" | "wear" => {
                let mut color = srgb_hex_to_linear("#d8d2c6")?;
                let mut strength = 0.45f32;
                let mut threshold = 0.14f32;
                let mut chip = 0.35f32;
                let mut rough = 0.88f32;
                let mut bump = -0.004f32;
                let mut scale = 14.0f32;
                let mut seed = spec.seed.wrapping_add(22);
                for tok in &tokens[1..] {
                    if tok.starts_with('#') {
                        color = srgb_hex_to_linear(tok).map_err(|e| format!("line {lineno}: {e}"))?;
                    } else if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "col" | "color" => color = srgb_hex_to_linear(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "str" | "amp" => strength = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.5),
                            "thresh" | "th" => threshold = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.01, 0.9),
                            "chip" => chip = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0),
                            "rough" | "r" => rough = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.04, 1.0),
                            "bump" | "h" => bump = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "scale" => scale = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(1.0),
                            "seed" => seed = v.parse().map_err(|_| format!("line {lineno}: bad seed"))?,
                            _ => return Err(format!("line {lineno}: unknown edge flag '{tok}'")),
                        }
                    }
                }
                spec.layers.push(TexLayer::EdgeWear { color, strength, threshold, chip, rough, bump, scale, seed });
            }
            "cavity" | "grime" | "dirt" => {
                let mut color = srgb_hex_to_linear("#26221d")?;
                let mut strength = 0.55f32;
                let mut depth = 0.45f32;
                let mut rough = 0.96f32;
                for tok in &tokens[1..] {
                    if tok.starts_with('#') {
                        color = srgb_hex_to_linear(tok).map_err(|e| format!("line {lineno}: {e}"))?;
                    } else if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "col" | "color" => color = srgb_hex_to_linear(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "str" | "amp" => strength = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0),
                            "depth" | "d" => depth = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.05, 1.0),
                            "rough" | "r" => rough = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.04, 1.0),
                            _ => return Err(format!("line {lineno}: unknown cavity flag '{tok}'")),
                        }
                    }
                }
                spec.layers.push(TexLayer::Cavity { color, strength, depth, rough });
            }
            "moss" | "snow" | "lichen" => {
                let default_hex = if op == "snow" { "#e6eef6" } else { "#49682e" };
                let default_alt = if op == "snow" { "#cfdce8" } else { "#6c8736" };
                let mut color = srgb_hex_to_linear(default_hex)?;
                let mut alt_color = srgb_hex_to_linear(default_alt)?;
                let mut strength = 0.85f32;
                let mut coverage = 0.48f32;
                let mut up_weight = if op == "snow" { 0.95 } else { 0.65 };
                let mut cavity_weight = if op == "snow" { 0.25 } else { 0.78 };
                let mut y_band = None;
                let mut rough = 0.94f32;
                let mut bump = 0.006f32;
                let mut scale = 6.0f32;
                let mut seed = spec.seed.wrapping_add(33);
                for tok in &tokens[1..] {
                    if tok.starts_with('#') {
                        color = srgb_hex_to_linear(tok).map_err(|e| format!("line {lineno}: {e}"))?;
                    } else if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "col" | "color" => color = srgb_hex_to_linear(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "alt" => alt_color = srgb_hex_to_linear(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "str" | "amp" => strength = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0),
                            "cov" | "coverage" => coverage = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0),
                            "up" => up_weight = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 2.0),
                            "cav" | "cavity" => cavity_weight = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 2.0),
                            "y" | "band" => y_band = Some(parse_range_f32(v, &env).map_err(|e| format!("line {lineno}: {e}"))?),
                            "rough" | "r" => rough = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.04, 1.0),
                            "bump" | "h" => bump = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "scale" => scale = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.max(1.0),
                            "seed" => seed = v.parse().map_err(|_| format!("line {lineno}: bad seed"))?,
                            _ => return Err(format!("line {lineno}: unknown {op} flag '{tok}'")),
                        }
                    }
                }
                spec.layers.push(TexLayer::Moss {
                    color,
                    alt_color,
                    strength,
                    coverage,
                    up_weight,
                    cavity_weight,
                    y_band,
                    rough,
                    bump,
                    scale,
                    seed,
                });
            }
            "svg" | "decal" | "overlay" => {
                if tokens.len() < 2 {
                    return Err(format!("line {lineno}: svg requires <rel.svg>"));
                }
                let rel_path = tokens[1].clone();
                let mut blend = BlendMode::Over;
                let mut opacity = 1.0f32;
                let mut mask = LayerMask::All;
                let mut rough = None;
                let mut metal = None;
                let mut height = 0.0f32;
                let mut blur_px = 1.2f32;
                let mut ao_strength = 0.55f32;
                let mut wrap = None;
                for tok in &tokens[2..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "blend" | "mode" => {
                                blend = BlendMode::parse(v).ok_or_else(|| format!("line {lineno}: unknown blend '{v}'"))?;
                            }
                            "opacity" | "op" | "str" => {
                                opacity = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0);
                            }
                            "mask" => {
                                mask = LayerMask::parse(v).ok_or_else(|| format!("line {lineno}: unknown mask '{v}'"))?;
                            }
                            "rough" | "r" => rough = Some(env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.04, 1.0)),
                            "metal" | "m" => metal = Some(env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 1.0)),
                            "height" | "h" | "bump" => height = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?,
                            "blur" => blur_px = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 12.0),
                            "ao" => ao_strength = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 2.0),
                            "wrap" => {
                                if !v.eq_ignore_ascii_case("auto") {
                                    wrap = Some(WrapMode::parse(v).ok_or_else(|| format!("line {lineno}: unknown wrap '{v}'"))?);
                                }
                            }
                            _ => return Err(format!("line {lineno}: unknown svg flag '{tok}'")),
                        }
                    }
                }
                spec.layers.push(TexLayer::Svg {
                    rel_path,
                    blend,
                    opacity,
                    mask,
                    rough,
                    metal,
                    height,
                    blur_px,
                    ao_strength,
                    wrap,
                });
            }
            _ => {
                return Err(format!(
                    "line {lineno}: unknown tex directive '{}' (use tex, pal, mesh, stones, grain, edge, cavity, moss, svg)",
                    tokens[0]
                ));
            }
        }
        i += 1;
    }

    Ok(spec)
}
