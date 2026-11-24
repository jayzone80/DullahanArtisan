/// Compact line-oriented `.ter` DSL parser for procedural + hand-designed landscapes.

use std::path::{Path, PathBuf};

use crate::common::expr::ExprEnv;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StampOp {
    Add,
    Max,
    Set,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseKind {
    Fbm,
    Ridge,
    Billow,
    Warp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseMask {
    All,
    High,
    Low,
    Slope,
}

#[derive(Debug, Clone)]
pub enum TerrainStep {
    BasePng {
        path: PathBuf,
        scale: f32,
        bias: f32,
        blur: usize,
    },
    Island {
        radius: f32,
        falloff: f32,
        height: f32,
    },
    Peak {
        u: f32,
        v: f32,
        radius: f32,
        height: f32,
        sharp: f32,
        op: StampOp,
    },
    Ridge {
        pts: Vec<(f32, f32)>,
        width: f32,
        height: f32,
        sharp: f32,
        op: StampOp,
    },
    Plateau {
        u: f32,
        v: f32,
        radius: f32,
        height: f32,
        flat: f32,
        blend: f32,
    },
    Valley {
        pts: Vec<(f32, f32)>,
        width: f32,
        depth: f32,
        flat: f32,
    },
    Crater {
        u: f32,
        v: f32,
        radius: f32,
        depth: f32,
        rim: f32,
    },
    Slope {
        dir_deg: f32,
        lo: f32,
        hi: f32,
    },
    Noise {
        kind: NoiseKind,
        octaves: u32,
        freq: f32,
        amp: f32,
        lacunarity: f32,
        gain: f32,
        seed_offset: u64,
        mask: NoiseMask,
    },
    Erode {
        drops: usize,
        inertia: f32,
        capacity: f32,
        deposition: f32,
        erosion: f32,
        evaporation: f32,
        radius: usize,
        max_steps: usize,
        thermal_iters: usize,
        talus: f32,
    },
    Smooth {
        iters: usize,
    },
    Clamp {
        lo: f32,
        hi: f32,
    },
    Normalize,
}

#[derive(Debug, Clone)]
pub struct SplatRule {
    pub channel: usize, // 0=R, 1=G, 2=B, 3=A
    pub mat_name: String,
    pub h_min: f32,
    pub h_max: f32,
    pub slope_min: f32, // degrees
    pub slope_max: f32, // degrees
    pub flow_min: f32,
    pub flow_max: f32,
    pub weight: f32,
    pub sharpness: f32,
}

#[derive(Debug, Clone)]
pub struct TerrainDoc {
    pub name: String,
    pub size: usize,            // heightmap width & height (square, e.g. 256)
    pub world: [f32; 3],        // [width_x, max_height_y, depth_z]
    pub seed: u64,
    pub sea_level: f32,         // normalized 0..1
    pub palette_rel: Option<String>,
    pub steps: Vec<TerrainStep>,
    pub splats: Vec<SplatRule>,
}

pub fn load_terrain_file(path: &Path) -> Result<TerrainDoc, String> {
    let raw = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read terrain '{}': {e}", path.display()))?;
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("terrain");
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    parse_terrain(&raw, stem, base_dir)
}

pub fn parse_terrain(src: &str, default_name: &str, base_dir: &Path) -> Result<TerrainDoc, String> {
    let mut doc = TerrainDoc {
        name: default_name.to_string(),
        size: 256,
        world: [100.0, 24.0, 100.0],
        seed: 42,
        sea_level: 0.0,
        palette_rel: None,
        steps: Vec::new(),
        splats: Vec::new(),
    };
    let mut env = ExprEnv::new();

    for (idx, raw_line) in src.strip_prefix('\u{feff}').unwrap_or(src).lines().enumerate() {
        let lineno = idx + 1;
        let clean = strip_comment(raw_line).trim();
        if clean.is_empty() {
            continue;
        }
        let toks = tokenize(clean);
        if toks.is_empty() {
            continue;
        }
        let op = toks[0].to_ascii_lowercase();

        if op.starts_with('$') {
            for t in &toks {
                let (k, v) = t
                    .split_once('=')
                    .ok_or_else(|| format!("terrain line {lineno}: expected $var=expr"))?;
                let val = env.eval(v).map_err(|e| format!("terrain line {lineno}: {e}"))?;
                env.set(k, val);
            }
            continue;
        }

        match op.as_str() {
            "pal" | "palette" => {
                doc.palette_rel = Some(
                    toks.get(1)
                        .ok_or_else(|| format!("terrain line {lineno}: pal requires path"))?
                        .trim_matches('"')
                        .to_string(),
                );
            }
            "terrain" | "ter" => {
                for tok in &toks[1..] {
                    if let Some((k, v)) = tok.split_once('=') {
                        match k.to_ascii_lowercase().as_str() {
                            "size" | "res" => {
                                let s = v.split('x').next().unwrap_or(v);
                                let n = env.eval_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
                                doc.size = (n.round() as usize).clamp(32, 1024);
                            }
                            "world" => {
                                let nums = env.eval_list_f32(v).map_err(|e| format!("line {lineno}: {e}"))?;
                                if nums.len() != 3 {
                                    return Err(format!("line {lineno}: world expects W,H,D"));
                                }
                                doc.world = [nums[0].max(1.0), nums[1].max(0.1), nums[2].max(1.0)];
                            }
                            "seed" => {
                                doc.seed = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?.round().max(0.0) as u64;
                            }
                            "sea" => {
                                doc.sea_level = env.eval_f32(v).map_err(|e| format!("line {lineno}: {e}"))?.clamp(0.0, 0.95);
                            }
                            other => return Err(format!("terrain line {lineno}: unknown header option '{other}'")),
                        }
                    } else {
                        doc.name = tok.clone();
                    }
                }
            }
            "base" => {
                let rel = toks
                    .get(1)
                    .ok_or_else(|| format!("terrain line {lineno}: base requires <sketch.png>"))?
                    .trim_matches('"');
                let kv = collect_kv(&toks[2..]);
                let scale = eval_kv(&kv, "scale", 1.0, &env, lineno)?;
                let bias = eval_kv(&kv, "bias", 0.0, &env, lineno)?;
                let blur = eval_kv(&kv, "blur", 0.0, &env, lineno)?.round().max(0.0) as usize;
                doc.steps.push(TerrainStep::BasePng {
                    path: base_dir.join(rel),
                    scale,
                    bias,
                    blur,
                });
            }
            "island" => {
                let kv = collect_kv(&toks[1..]);
                let radius = eval_kv(&kv, "r", 0.44, &env, lineno)?;
                let falloff = eval_kv(&kv, "falloff", 2.0, &env, lineno)?;
                let height = eval_kv(&kv, "h", 0.28, &env, lineno)?;
                doc.steps.push(TerrainStep::Island { radius, falloff, height });
            }
            "peak" | "hill" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                if pos.is_empty() {
                    return Err(format!("terrain line {lineno}: peak requires <u,v>"));
                }
                let uv = parse_uv(pos[0], &env, lineno)?;
                let radius = if pos.len() >= 2 {
                    env.eval_f32(pos[1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "r", 0.25, &env, lineno)?
                };
                let height = if pos.len() >= 3 {
                    env.eval_f32(pos[2]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "h", 0.7, &env, lineno)?
                };
                let sharp = eval_kv(&kv, "sharp", 1.4, &env, lineno)?;
                let op = parse_stamp_op(kv.get("op").copied().unwrap_or("add"));
                doc.steps.push(TerrainStep::Peak { u: uv.0, v: uv.1, radius, height, sharp, op });
            }
            "ridge" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                let (pts, next_idx) = parse_polyline_or_pair(&pos, &env, lineno)?;
                let width = if pos.len() > next_idx {
                    env.eval_f32(pos[next_idx]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "w", 0.22, &env, lineno)?
                };
                let height = if pos.len() > next_idx + 1 {
                    env.eval_f32(pos[next_idx + 1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "h", 0.72, &env, lineno)?
                };
                let sharp = eval_kv(&kv, "sharp", 1.35, &env, lineno)?;
                let op = parse_stamp_op(kv.get("op").copied().unwrap_or("max"));
                doc.steps.push(TerrainStep::Ridge { pts, width, height, sharp, op });
            }
            "plateau" | "pad" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                if pos.is_empty() {
                    return Err(format!("terrain line {lineno}: plateau requires <u,v>"));
                }
                let uv = parse_uv(pos[0], &env, lineno)?;
                let radius = if pos.len() >= 2 {
                    env.eval_f32(pos[1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "r", 0.18, &env, lineno)?
                };
                let height = if pos.len() >= 3 {
                    env.eval_f32(pos[2]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "h", 0.35, &env, lineno)?
                };
                let flat = eval_kv(&kv, "flat", 0.72, &env, lineno)?.clamp(0.1, 0.98);
                let blend = eval_kv(&kv, "blend", 0.95, &env, lineno)?.clamp(0.05, 1.0);
                doc.steps.push(TerrainStep::Plateau { u: uv.0, v: uv.1, radius, height, flat, blend });
            }
            "valley" | "river" | "cut" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                let (pts, next_idx) = parse_polyline_or_pair(&pos, &env, lineno)?;
                let width = if pos.len() > next_idx {
                    env.eval_f32(pos[next_idx]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "w", 0.14, &env, lineno)?
                };
                let depth = if pos.len() > next_idx + 1 {
                    env.eval_f32(pos[next_idx + 1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "depth", 0.25, &env, lineno)?
                };
                let flat = eval_kv(&kv, "flat", 0.28, &env, lineno)?.clamp(0.0, 0.9);
                doc.steps.push(TerrainStep::Valley { pts, width, depth, flat });
            }
            "crater" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                if pos.is_empty() {
                    return Err(format!("terrain line {lineno}: crater requires <u,v>"));
                }
                let uv = parse_uv(pos[0], &env, lineno)?;
                let radius = if pos.len() >= 2 {
                    env.eval_f32(pos[1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "r", 0.2, &env, lineno)?
                };
                let depth = if pos.len() >= 3 {
                    env.eval_f32(pos[2]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "depth", 0.25, &env, lineno)?
                };
                let rim = eval_kv(&kv, "rim", 0.18, &env, lineno)?;
                doc.steps.push(TerrainStep::Crater { u: uv.0, v: uv.1, radius, depth, rim });
            }
            "slope" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                let dir_deg = if !pos.is_empty() {
                    env.eval_f32(pos[0]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "dir", 0.0, &env, lineno)?
                };
                let lo = if pos.len() >= 2 {
                    env.eval_f32(pos[1]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "lo", 0.0, &env, lineno)?
                };
                let hi = if pos.len() >= 3 {
                    env.eval_f32(pos[2]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "hi", 0.35, &env, lineno)?
                };
                doc.steps.push(TerrainStep::Slope { dir_deg, lo, hi });
            }
            "noise" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                let kind = match pos.first().copied().unwrap_or("fbm").to_ascii_lowercase().as_str() {
                    "ridge" | "ridged" => NoiseKind::Ridge,
                    "billow" => NoiseKind::Billow,
                    "warp" | "warped" => NoiseKind::Warp,
                    _ => NoiseKind::Fbm,
                };
                let octaves = eval_kv(&kv, "oct", 5.0, &env, lineno)?.round().clamp(1.0, 10.0) as u32;
                let freq = eval_kv(&kv, "freq", 3.0, &env, lineno)?.max(0.1);
                let amp = eval_kv(&kv, "amp", 0.18, &env, lineno)?;
                let lacunarity = eval_kv(&kv, "lac", 2.05, &env, lineno)?.max(1.1);
                let gain = eval_kv(&kv, "gain", 0.5, &env, lineno)?.clamp(0.1, 0.9);
                let seed_offset = eval_kv(&kv, "seed", 0.0, &env, lineno)?.round().max(0.0) as u64;
                let mask = match kv.get("mask").copied().unwrap_or("all").to_ascii_lowercase().as_str() {
                    "high" => NoiseMask::High,
                    "low" => NoiseMask::Low,
                    "slope" => NoiseMask::Slope,
                    _ => NoiseMask::All,
                };
                doc.steps.push(TerrainStep::Noise {
                    kind,
                    octaves,
                    freq,
                    amp,
                    lacunarity,
                    gain,
                    seed_offset,
                    mask,
                });
            }
            "erode" | "erosion" => {
                let kv = collect_kv(&toks[1..]);
                let drops = eval_kv(&kv, "drops", 45000.0, &env, lineno)?.round().clamp(0.0, 500_000.0) as usize;
                let inertia = eval_kv(&kv, "inertia", 0.12, &env, lineno)?.clamp(0.0, 0.95);
                let capacity = eval_kv(&kv, "cap", 4.0, &env, lineno)?.max(0.2);
                let deposition = eval_kv(&kv, "dep", 0.25, &env, lineno)?.clamp(0.01, 1.0);
                let erosion = eval_kv(&kv, "ero", 0.30, &env, lineno)?.clamp(0.01, 1.0);
                let evaporation = eval_kv(&kv, "evap", 0.02, &env, lineno)?.clamp(0.001, 0.5);
                let radius = eval_kv(&kv, "rad", 2.0, &env, lineno)?.round().clamp(1.0, 6.0) as usize;
                let max_steps = eval_kv(&kv, "steps", 48.0, &env, lineno)?.round().clamp(8.0, 128.0) as usize;
                let thermal_iters = eval_kv(&kv, "thermal", 8.0, &env, lineno)?.round().clamp(0.0, 64.0) as usize;
                let talus = eval_kv(&kv, "talus", 0.012, &env, lineno)?.clamp(0.001, 0.2);
                doc.steps.push(TerrainStep::Erode {
                    drops,
                    inertia,
                    capacity,
                    deposition,
                    erosion,
                    evaporation,
                    radius,
                    max_steps,
                    thermal_iters,
                    talus,
                });
            }
            "smooth" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                let iters = if !pos.is_empty() {
                    env.eval_f32(pos[0]).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    eval_kv(&kv, "iters", 2.0, &env, lineno)?
                }
                .round()
                .clamp(1.0, 32.0) as usize;
                doc.steps.push(TerrainStep::Smooth { iters });
            }
            "clamp" => {
                let kv = collect_kv(&toks[1..]);
                let lo = eval_kv(&kv, "lo", 0.0, &env, lineno)?;
                let hi = eval_kv(&kv, "hi", 1.0, &env, lineno)?;
                doc.steps.push(TerrainStep::Clamp { lo, hi });
            }
            "norm" | "normalize" => {
                doc.steps.push(TerrainStep::Normalize);
            }
            "splat" | "biome" => {
                let (pos, kv) = split_pos_kv(&toks[1..]);
                if pos.len() < 2 {
                    return Err(format!("terrain line {lineno}: splat requires <r|g|b|a> <mat_name> [h=lo..hi] [slope=lo..hi]"));
                }
                let channel = match pos[0].to_ascii_lowercase().as_str() {
                    "r" | "0" => 0,
                    "g" | "1" => 1,
                    "b" | "2" => 2,
                    "a" | "3" => 3,
                    other => return Err(format!("terrain line {lineno}: splat channel must be r|g|b|a (got '{other}')")),
                };
                let mat_name = pos[1].to_string();
                let (h_min, h_max) = parse_range(kv.get("h").copied(), (0.0, 1.0), &env, lineno)?;
                let (slope_min, slope_max) = parse_range(kv.get("slope").copied(), (0.0, 90.0), &env, lineno)?;
                let (flow_min, flow_max) = parse_range(kv.get("flow").copied(), (0.0, 1.0), &env, lineno)?;
                let weight = eval_kv(&kv, "w", 1.0, &env, lineno)?.max(0.01);
                let sharpness = eval_kv(&kv, "sharp", 10.0, &env, lineno)?.max(1.0);
                doc.splats.push(SplatRule {
                    channel,
                    mat_name,
                    h_min,
                    h_max,
                    slope_min,
                    slope_max,
                    flow_min,
                    flow_max,
                    weight,
                    sharpness,
                });
            }
            other => {
                return Err(format!("terrain line {lineno}: unknown command '{other}'"));
            }
        }
    }

    Ok(doc)
}

fn strip_comment(line: &str) -> &str {
    line.split('#').next().unwrap_or(line)
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_q = false;
    for ch in s.chars() {
        if ch == '"' {
            in_q = !in_q;
        } else if ch.is_whitespace() && !in_q {
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

fn collect_kv<'a>(toks: &'a [String]) -> std::collections::HashMap<String, &'a str> {
    let mut m = std::collections::HashMap::new();
    for t in toks {
        if let Some((k, v)) = t.split_once('=') {
            m.insert(k.to_ascii_lowercase(), v);
        }
    }
    m
}

fn split_pos_kv<'a>(toks: &'a [String]) -> (Vec<&'a str>, std::collections::HashMap<String, &'a str>) {
    let mut pos = Vec::new();
    let mut kv = std::collections::HashMap::new();
    for t in toks {
        if let Some((k, v)) = t.split_once('=') {
            if !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                kv.insert(k.to_ascii_lowercase(), v);
                continue;
            }
        }
        pos.push(t.as_str());
    }
    (pos, kv)
}

fn eval_kv(
    kv: &std::collections::HashMap<String, &str>,
    key: &str,
    default: f32,
    env: &ExprEnv,
    lineno: usize,
) -> Result<f32, String> {
    if let Some(&v) = kv.get(key) {
        env.eval_f32(v).map_err(|e| format!("terrain line {lineno}: {key}={v}: {e}"))
    } else {
        Ok(default)
    }
}

fn parse_uv(s: &str, env: &ExprEnv, lineno: usize) -> Result<(f32, f32), String> {
    let sep = if s.contains(':') { ':' } else { ',' };
    let (a, b) = s
        .split_once(sep)
        .ok_or_else(|| format!("terrain line {lineno}: expected u,v coordinate (got '{s}')"))?;
    let u = env.eval_f32(a).map_err(|e| format!("line {lineno}: {e}"))?;
    let v = env.eval_f32(b).map_err(|e| format!("line {lineno}: {e}"))?;
    Ok((u, v))
}

fn parse_polyline_or_pair(
    pos: &[&str],
    env: &ExprEnv,
    lineno: usize,
) -> Result<(Vec<(f32, f32)>, usize), String> {
    if pos.is_empty() {
        return Err(format!("terrain line {lineno}: expected at least 2 coordinates"));
    }
    // Polyline syntax: `u0:v0,u1:v1,u2:v2`
    if pos[0].contains(':') && pos[0].contains(',') {
        let mut pts = Vec::new();
        for seg in pos[0].split(',') {
            pts.push(parse_uv(seg, env, lineno)?);
        }
        if pts.len() < 2 {
            return Err(format!("terrain line {lineno}: polyline needs >= 2 points"));
        }
        return Ok((pts, 1));
    }
    if pos.len() < 2 {
        return Err(format!("terrain line {lineno}: expected two <u,v> points"));
    }
    let p0 = parse_uv(pos[0], env, lineno)?;
    let p1 = parse_uv(pos[1], env, lineno)?;
    Ok((vec![p0, p1], 2))
}

fn parse_stamp_op(s: &str) -> StampOp {
    match s.to_ascii_lowercase().as_str() {
        "max" => StampOp::Max,
        "set" => StampOp::Set,
        _ => StampOp::Add,
    }
}

fn parse_range(
    raw: Option<&str>,
    default: (f32, f32),
    env: &ExprEnv,
    lineno: usize,
) -> Result<(f32, f32), String> {
    let Some(s) = raw else {
        return Ok(default);
    };
    if let Some((a, b)) = s.split_once("..") {
        let lo = env.eval_f32(a).map_err(|e| format!("line {lineno}: {e}"))?;
        let hi = env.eval_f32(b).map_err(|e| format!("line {lineno}: {e}"))?;
        Ok((lo, hi))
    } else {
        let v = env.eval_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
        Ok((v, default.1))
    }
}
