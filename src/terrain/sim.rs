/// Zero-dependency procedural terrain synthesizer:
/// - Designed macro layout stamps (ridge, peak, plateau, valley, crater, island, PNG base sketch)
/// - Multi-octave fBm, Ridged Multifractal, Billow, and Domain-Warped 2D noise
/// - Fast particle-based hydraulic erosion + thermal talus relaxation
/// - 4-channel normalized biome weight/splatmaps + 16-bit PNG & tangent-space normal map export

use crate::common::math::Vec3;
use crate::common::png::{decode_png, encode_gray16, encode_rgb, encode_rgba};
use crate::terrain::parser::{NoiseKind, NoiseMask, SplatRule, StampOp, TerrainDoc, TerrainStep};

#[derive(Debug, Clone)]
pub struct TerrainData {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub world: [f32; 3], // [world_x, world_y, world_z]
    pub sea_level: f32,
    pub heights: Vec<f32>,         // 0..=1 normalized elevation
    pub flow: Vec<f32>,            // 0..=1 normalized cumulative hydraulic water flow
    pub erosion_delta: Vec<f32>,   // signed erosion (-carved, +deposited)
    pub slopes_deg: Vec<f32>,      // 0..=90 slope angle in degrees
    pub splats: Vec<[f32; 4]>,     // normalized 4-channel biome weights
    pub channel_mats: [String; 4], // palette swatch name per channel R,G,B,A
    pub drops_simulated: usize,
}

impl TerrainData {
    #[inline]
    pub fn sample_height(&self, u: f32, v: f32) -> f32 {
        let x = (u.clamp(0.0, 1.0) * (self.width - 1) as f32).clamp(0.0, (self.width - 1) as f32);
        let y = (v.clamp(0.0, 1.0) * (self.height - 1) as f32).clamp(0.0, (self.height - 1) as f32);
        let x0 = x.floor() as usize;
        let y0 = y.floor() as usize;
        let x1 = (x0 + 1).min(self.width - 1);
        let y1 = (y0 + 1).min(self.height - 1);
        let fx = x - x0 as f32;
        let fy = y - y0 as f32;
        let h00 = self.heights[y0 * self.width + x0];
        let h10 = self.heights[y0 * self.width + x1];
        let h01 = self.heights[y1 * self.width + x0];
        let h11 = self.heights[y1 * self.width + x1];
        let hx0 = h00 + (h10 - h00) * fx;
        let hx1 = h01 + (h11 - h01) * fx;
        hx0 + (hx1 - hx0) * fy
    }

    #[inline]
    pub fn sample_splat(&self, u: f32, v: f32) -> [f32; 4] {
        let x = ((u.clamp(0.0, 1.0) * (self.width - 1) as f32).round() as usize).min(self.width - 1);
        let y = ((v.clamp(0.0, 1.0) * (self.height - 1) as f32).round() as usize).min(self.height - 1);
        self.splats[y * self.width + x]
    }

    pub fn encode_height16_png(&self) -> Vec<u8> {
        let gray: Vec<u16> = self
            .heights
            .iter()
            .map(|&h| (h.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect();
        encode_gray16(self.width as u32, self.height as u32, &gray)
    }

    pub fn encode_splat_rgba_png(&self) -> Vec<u8> {
        let mut rgba = Vec::with_capacity(self.splats.len() * 4);
        for &w in &self.splats {
            rgba.push((w[0].clamp(0.0, 1.0) * 255.0).round() as u8);
            rgba.push((w[1].clamp(0.0, 1.0) * 255.0).round() as u8);
            rgba.push((w[2].clamp(0.0, 1.0) * 255.0).round() as u8);
            rgba.push((w[3].clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        encode_rgba(self.width as u32, self.height as u32, &rgba)
    }

    pub fn encode_normal_rgb_png(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        let dx_world = self.world[0] / (self.width.saturating_sub(1).max(1)) as f32;
        let dz_world = self.world[2] / (self.height.saturating_sub(1).max(1)) as f32;
        for y in 0..self.height {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(self.height - 1);
            for x in 0..self.width {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(self.width - 1);
                let hl = self.heights[y * self.width + xm] * self.world[1];
                let hr = self.heights[y * self.width + xp] * self.world[1];
                let hu = self.heights[ym * self.width + x] * self.world[1];
                let hd = self.heights[yp * self.width + x] * self.world[1];
                let gx = (hr - hl) / (2.0 * dx_world);
                let gz = (hd - hu) / (2.0 * dz_world);
                let n = Vec3::new(-gx, 1.0, -gz).normalized();
                // Standard tangent-space normal encoding (X=R, Z=G, Up=B)
                rgb.push(((n.x * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
                rgb.push(((n.z * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
                rgb.push(((n.y * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8);
            }
        }
        encode_rgb(self.width as u32, self.height as u32, &rgb)
    }

    pub fn min_max_height(&self) -> (f32, f32) {
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for &h in &self.heights {
            lo = lo.min(h);
            hi = hi.max(h);
        }
        (lo, hi)
    }

    pub fn max_slope_deg(&self) -> f32 {
        self.slopes_deg.iter().copied().fold(0.0f32, f32::max)
    }
}

/// Build a TerrainData directly from an external grayscale heightmap PNG and optional RGBA weightmap PNG.
pub fn build_from_heightmap_png(
    height_bytes: &[u8],
    weight_bytes: Option<&[u8]>,
    world: [f32; 3],
    name: &str,
    channel_mats: [String; 4],
) -> Result<TerrainData, String> {
    let h_img = decode_png(height_bytes)?;
    let w = h_img.width as usize;
    let h = h_img.height as usize;
    if w < 4 || h < 4 {
        return Err("heightmap PNG must be at least 4x4".into());
    }
    let heights = h_img.gray_f32;
    let slopes_deg = compute_slopes_deg(&heights, w, h, world);
    let flow = vec![0.0f32; w * h];
    let erosion_delta = vec![0.0f32; w * h];

    let splats = if let Some(wb) = weight_bytes {
        let w_img = decode_png(wb)?;
        let ww = w_img.width as usize;
        let wh = w_img.height as usize;
        let mut out = Vec::with_capacity(w * h);
        for y in 0..h {
            let sy = (y * wh / h).min(wh - 1);
            for x in 0..w {
                let sx = (x * ww / w).min(ww - 1);
                let o = (sy * ww + sx) * 4;
                let mut arr = [
                    w_img.rgba[o] as f32 / 255.0,
                    w_img.rgba[o + 1] as f32 / 255.0,
                    w_img.rgba[o + 2] as f32 / 255.0,
                    w_img.rgba[o + 3] as f32 / 255.0,
                ];
                let sum = arr[0] + arr[1] + arr[2] + arr[3];
                if sum > 1e-5 {
                    for c in &mut arr {
                        *c /= sum;
                    }
                } else {
                    arr = [0.0, 1.0, 0.0, 0.0];
                }
                out.push(arr);
            }
        }
        out
    } else {
        compute_default_splats(&heights, &slopes_deg, &flow)
    };

    Ok(TerrainData {
        name: name.to_string(),
        width: w,
        height: h,
        world,
        sea_level: 0.0,
        heights,
        flow,
        erosion_delta,
        slopes_deg,
        splats,
        channel_mats,
        drops_simulated: 0,
    })
}

/// Evaluate a parsed `.ter` document into a complete TerrainData (heightfield, erosion, slopes, splats).
pub fn simulate_terrain(doc: &TerrainDoc, seed_override: Option<u64>) -> Result<TerrainData, String> {
    let w = doc.size;
    let h = doc.size;
    let seed = seed_override.unwrap_or(doc.seed);
    let mut heights = vec![0.0f32; w * h];
    let mut flow = vec![0.0f32; w * h];
    let mut erosion_delta = vec![0.0f32; w * h];
    let mut drops_simulated = 0usize;

    for step in &doc.steps {
        match step {
            TerrainStep::BasePng { path, scale, bias, blur } => {
                let bytes = std::fs::read(path)
                    .map_err(|e| format!("failed to read base PNG '{}': {e}", path.display()))?;
                let img = decode_png(&bytes)?;
                let iw = img.width as usize;
                let ih = img.height as usize;
                for y in 0..h {
                    let fy = (y as f32 / (h - 1).max(1) as f32) * (ih - 1).max(1) as f32;
                    let y0 = fy.floor() as usize;
                    let y1 = (y0 + 1).min(ih - 1);
                    let ty = fy - y0 as f32;
                    for x in 0..w {
                        let fx = (x as f32 / (w - 1).max(1) as f32) * (iw - 1).max(1) as f32;
                        let x0 = fx.floor() as usize;
                        let x1 = (x0 + 1).min(iw - 1);
                        let tx = fx - x0 as f32;
                        let a = img.gray_f32[y0 * iw + x0];
                        let b = img.gray_f32[y0 * iw + x1];
                        let c = img.gray_f32[y1 * iw + x0];
                        let d = img.gray_f32[y1 * iw + x1];
                        let v = (a + (b - a) * tx) * (1.0 - ty) + (c + (d - c) * tx) * ty;
                        heights[y * w + x] = v * *scale + *bias;
                    }
                }
                if *blur > 0 {
                    smooth_field(&mut heights, w, h, *blur);
                }
            }
            TerrainStep::Island { radius, falloff, height } => {
                let r = radius.max(0.05);
                for y in 0..h {
                    let v = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let u = x as f32 / (w - 1) as f32 - 0.5;
                        let d = (u * u + v * v).sqrt() / r;
                        let mask = (1.0 - d.clamp(0.0, 1.0).powf(*falloff)).max(0.0);
                        let cur = heights[y * w + x];
                        heights[y * w + x] = if cur.abs() < 1e-6 {
                            mask * *height
                        } else {
                            cur * mask + mask * *height * 0.25
                        };
                    }
                }
            }
            TerrainStep::Peak { u, v, radius, height, sharp, op } => {
                let r = radius.max(0.01);
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let d = ((gx - *u).powi(2) + (gy - *v).powi(2)).sqrt() / r;
                        if d < 1.0 {
                            let val = (1.0 - d).powf(*sharp) * *height;
                            apply_stamp(&mut heights[y * w + x], val, *op);
                        }
                    }
                }
            }
            TerrainStep::Ridge { pts, width, height, sharp, op } => {
                let rad = width.max(0.01);
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let d = dist_to_polyline(gx, gy, pts) / rad;
                        if d < 1.0 {
                            let val = (1.0 - d).powf(*sharp) * *height;
                            apply_stamp(&mut heights[y * w + x], val, *op);
                        }
                    }
                }
            }
            TerrainStep::Plateau { u, v, radius, height, flat, blend } => {
                let r = radius.max(0.01);
                let inner = r * *flat;
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let d = ((gx - *u).powi(2) + (gy - *v).powi(2)).sqrt();
                        if d < r {
                            let t = if d <= inner {
                                1.0
                            } else {
                                let k = ((r - d) / (r - inner).max(1e-5)).clamp(0.0, 1.0);
                                k * k * (3.0 - 2.0 * k)
                            };
                            let wgt = t * *blend;
                            let cur = heights[y * w + x];
                            heights[y * w + x] = cur * (1.0 - wgt) + *height * wgt;
                        }
                    }
                }
            }
            TerrainStep::Valley { pts, width, depth, flat } => {
                let rad = width.max(0.01);
                let inner = rad * *flat;
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let d = dist_to_polyline(gx, gy, pts);
                        if d < rad {
                            let t = if d <= inner {
                                1.0
                            } else {
                                let k = ((rad - d) / (rad - inner).max(1e-5)).clamp(0.0, 1.0);
                                k * k * (3.0 - 2.0 * k)
                            };
                            heights[y * w + x] -= t * *depth;
                        }
                    }
                }
            }
            TerrainStep::Crater { u, v, radius, depth, rim } => {
                let r = radius.max(0.02);
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let d = ((gx - *u).powi(2) + (gy - *v).powi(2)).sqrt() / r;
                        if d < 1.35 {
                            if d < 0.78 {
                                let k = d / 0.78;
                                let bowl = (1.0 - k * k) * *depth;
                                heights[y * w + x] -= bowl;
                            }
                            let rim_d = ((d - 0.92) / 0.38).abs();
                            if rim_d < 1.0 {
                                let k = 1.0 - rim_d;
                                heights[y * w + x] += k * k * (3.0 - 2.0 * k) * *rim;
                            }
                        }
                    }
                }
            }
            TerrainStep::Slope { dir_deg, lo, hi } => {
                let rad = dir_deg.to_radians();
                let (sd, cd) = rad.sin_cos();
                for y in 0..h {
                    let gy = y as f32 / (h - 1) as f32 - 0.5;
                    for x in 0..w {
                        let gx = x as f32 / (w - 1) as f32 - 0.5;
                        let t = (gx * cd + gy * sd + 0.5).clamp(0.0, 1.0);
                        heights[y * w + x] += *lo + (*hi - *lo) * t;
                    }
                }
            }
            TerrainStep::Noise { kind, octaves, freq, amp, lacunarity, gain, seed_offset, mask } => {
                let slopes_tmp = if *mask == NoiseMask::Slope {
                    Some(compute_slopes_deg(&heights, w, h, doc.world))
                } else {
                    None
                };
                let nseed = seed.wrapping_add(*seed_offset).wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                for y in 0..h {
                    let v = y as f32 / (h - 1) as f32;
                    for x in 0..w {
                        let u = x as f32 / (w - 1) as f32;
                        let n = eval_noise(u, v, *kind, *octaves, *freq, *lacunarity, *gain, nseed);
                        let cur = heights[y * w + x];
                        let m = match mask {
                            NoiseMask::All => 1.0,
                            NoiseMask::High => cur.clamp(0.0, 1.0),
                            NoiseMask::Low => 1.0 - cur.clamp(0.0, 1.0),
                            NoiseMask::Slope => {
                                let s = slopes_tmp.as_ref().unwrap()[y * w + x];
                                (s / 45.0).clamp(0.0, 1.0)
                            }
                        };
                        heights[y * w + x] += n * *amp * m;
                    }
                }
            }
            TerrainStep::Erode {
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
            } => {
                let before = heights.clone();
                run_hydraulic_erosion(
                    &mut heights,
                    &mut flow,
                    w,
                    h,
                    *drops,
                    *inertia,
                    *capacity,
                    *deposition,
                    *erosion,
                    *evaporation,
                    *radius,
                    *max_steps,
                    seed.wrapping_add(0x9E37_79B9_7F4A_7C15),
                );
                if *thermal_iters > 0 {
                    run_thermal_erosion(&mut heights, w, h, *thermal_iters, *talus);
                }
                for i in 0..heights.len() {
                    erosion_delta[i] += heights[i] - before[i];
                }
                drops_simulated += *drops;
            }
            TerrainStep::Smooth { iters } => {
                smooth_field(&mut heights, w, h, *iters);
            }
            TerrainStep::Clamp { lo, hi } => {
                for v in &mut heights {
                    *v = v.clamp(*lo, *hi);
                }
            }
            TerrainStep::Normalize => {
                let mut lo = f32::INFINITY;
                let mut hi = f32::NEG_INFINITY;
                for &v in &heights {
                    lo = lo.min(v);
                    hi = hi.max(v);
                }
                let span = (hi - lo).max(1e-6);
                for v in &mut heights {
                    *v = (*v - lo) / span;
                }
            }
        }
    }

    // Clamp final heightfield to [0, 1]
    for v in &mut heights {
        *v = v.clamp(0.0, 1.0);
    }

    // Normalize flow log-scale into [0, 1] so rivers and drainage basins read crisply
    let max_flow = flow.iter().copied().fold(0.0f32, f32::max);
    if max_flow > 1e-6 {
        let denom = (1.0 + max_flow).ln();
        for f in &mut flow {
            *f = ((1.0 + *f).ln() / denom).clamp(0.0, 1.0);
        }
    }

    let slopes_deg = compute_slopes_deg(&heights, w, h, doc.world);

    let mut channel_mats = [
        "sand_warm".to_string(),
        "grass_mid".to_string(),
        "stone_mid".to_string(),
        "roof_cream".to_string(),
    ];
    for r in &doc.splats {
        channel_mats[r.channel] = r.mat_name.clone();
    }

    let splats = if doc.splats.is_empty() {
        compute_default_splats(&heights, &slopes_deg, &flow)
    } else {
        compute_rule_splats(&heights, &slopes_deg, &flow, &doc.splats)
    };

    Ok(TerrainData {
        name: doc.name.clone(),
        width: w,
        height: h,
        world: doc.world,
        sea_level: doc.sea_level,
        heights,
        flow,
        erosion_delta,
        slopes_deg,
        splats,
        channel_mats,
        drops_simulated,
    })
}

fn apply_stamp(cur: &mut f32, val: f32, op: StampOp) {
    match op {
        StampOp::Add => *cur += val,
        StampOp::Max => *cur = cur.max(val),
        StampOp::Set => *cur = val,
    }
}

fn dist_to_polyline(x: f32, y: f32, pts: &[(f32, f32)]) -> f32 {
    let mut best = f32::INFINITY;
    for seg in pts.windows(2) {
        let (ax, ay) = seg[0];
        let (bx, by) = seg[1];
        let dx = bx - ax;
        let dy = by - ay;
        let len2 = dx * dx + dy * dy;
        let t = if len2 > 1e-10 {
            (((x - ax) * dx + (y - ay) * dy) / len2).clamp(0.0, 1.0)
        } else {
            0.0
        };
        let px = ax + dx * t;
        let py = ay + dy * t;
        best = best.min(((x - px).powi(2) + (y - py).powi(2)).sqrt());
    }
    best
}

fn smooth_field(h: &mut [f32], w: usize, height: usize, iters: usize) {
    let mut tmp = vec![0.0f32; h.len()];
    for _ in 0..iters {
        for y in 0..height {
            let ym = y.saturating_sub(1);
            let yp = (y + 1).min(height - 1);
            for x in 0..w {
                let xm = x.saturating_sub(1);
                let xp = (x + 1).min(w - 1);
                let c = h[y * w + x] * 4.0
                    + (h[y * w + xm] + h[y * w + xp] + h[ym * w + x] + h[yp * w + x]) * 2.0
                    + (h[ym * w + xm] + h[ym * w + xp] + h[yp * w + xm] + h[yp * w + xp]);
                tmp[y * w + x] = c * (1.0 / 16.0);
            }
        }
        h.copy_from_slice(&tmp);
    }
}

fn hash2(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = (x as u64).wrapping_mul(374761393)
        ^ (y as u64).wrapping_mul(668265263)
        ^ seed;
    h = (h ^ (h >> 13)).wrapping_mul(1274126177);
    h ^= h >> 16;
    ((h & 0x00FF_FFFF) as f32) / 16_777_215.0 * 2.0 - 1.0
}

fn value_noise_2d(x: f32, y: f32, seed: u64) -> f32 {
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let ux = fx * fx * fx * (fx * (fx * 6.0 - 15.0) + 10.0);
    let uy = fy * fy * fy * (fy * (fy * 6.0 - 15.0) + 10.0);
    let v00 = hash2(x0, y0, seed);
    let v10 = hash2(x0 + 1, y0, seed);
    let v01 = hash2(x0, y0 + 1, seed);
    let v11 = hash2(x0 + 1, y0 + 1, seed);
    let a = v00 + (v10 - v00) * ux;
    let b = v01 + (v11 - v01) * ux;
    a + (b - a) * uy
}

fn eval_noise(
    u: f32,
    v: f32,
    kind: NoiseKind,
    octaves: u32,
    freq: f32,
    lac: f32,
    gain: f32,
    seed: u64,
) -> f32 {
    let (su, sv) = if kind == NoiseKind::Warp {
        let wx = value_noise_2d(u * freq * 0.8 + 11.3, v * freq * 0.8 - 4.7, seed ^ 0xA5A5);
        let wy = value_noise_2d(u * freq * 0.8 - 7.1, v * freq * 0.8 + 19.2, seed ^ 0x5A5A);
        (u + wx * 0.14, v + wy * 0.14)
    } else {
        (u, v)
    };

    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    let mut f = freq;
    let mut a = 1.0f32;
    for oct in 0..octaves {
        let s = seed.wrapping_add((oct as u64).wrapping_mul(0x9E37_79B9));
        let n = value_noise_2d(su * f, sv * f, s);
        let term = match kind {
            NoiseKind::Fbm | NoiseKind::Warp => n,
            NoiseKind::Ridge => (1.0 - n.abs()).powi(2) * 2.0 - 1.0,
            NoiseKind::Billow => n.abs() * 2.0 - 1.0,
        };
        sum += term * a;
        norm += a;
        f *= lac;
        a *= gain;
    }
    if norm > 1e-6 { sum / norm } else { 0.0 }
}

/// Particle-based hydraulic erosion with precomputed circular brush and bilinear gradient interpolation.
#[allow(clippy::too_many_arguments)]
fn run_hydraulic_erosion(
    heights: &mut [f32],
    flow: &mut [f32],
    w: usize,
    h: usize,
    drops: usize,
    inertia: f32,
    capacity_factor: f32,
    deposition_rate: f32,
    erosion_rate: f32,
    evaporation_rate: f32,
    radius: usize,
    max_steps: usize,
    seed: u64,
) {
    if drops == 0 || w < 4 || h < 4 {
        return;
    }

    // Precompute circular erosion brush offsets & normalized weights
    let r_i = radius as isize;
    let mut brush: Vec<(isize, isize, f32)> = Vec::new();
    let mut w_sum = 0.0f32;
    for dy in -r_i..=r_i {
        for dx in -r_i..=r_i {
            let d = ((dx * dx + dy * dy) as f32).sqrt();
            if d < radius as f32 + 0.5 {
                let wt = (1.0 - d / (radius as f32 + 0.5)).max(0.0);
                brush.push((dx, dy, wt));
                w_sum += wt;
            }
        }
    }
    if w_sum > 0.0 {
        for b in &mut brush {
            b.2 /= w_sum;
        }
    }

    let mut rng = seed | 1;
    let mut next_f32 = || -> f32 {
        rng ^= rng << 13;
        rng ^= rng >> 7;
        rng ^= rng << 17;
        ((rng >> 40) as f32) / 16_777_215.0
    };

    let sample_hg = |heights: &[f32], px: f32, py: f32| -> (f32, f32, f32) {
        let x0 = (px.floor() as usize).min(w - 2);
        let y0 = (py.floor() as usize).min(h - 2);
        let fx = (px - x0 as f32).clamp(0.0, 1.0);
        let fy = (py - y0 as f32).clamp(0.0, 1.0);
        let h00 = heights[y0 * w + x0];
        let h10 = heights[y0 * w + x0 + 1];
        let h01 = heights[(y0 + 1) * w + x0];
        let h11 = heights[(y0 + 1) * w + x0 + 1];
        let gx = (h10 - h00) * (1.0 - fy) + (h11 - h01) * fy;
        let gy = (h01 - h00) * (1.0 - fx) + (h11 - h10) * fx;
        let hv = h00 * (1.0 - fx) * (1.0 - fy)
            + h10 * fx * (1.0 - fy)
            + h01 * (1.0 - fx) * fy
            + h11 * fx * fy;
        (hv, gx, gy)
    };

    for _ in 0..drops {
        let mut px = 1.0 + next_f32() * (w - 3) as f32;
        let mut py = 1.0 + next_f32() * (h - 3) as f32;
        let mut dx = 0.0f32;
        let mut dy = 0.0f32;
        let mut speed = 1.0f32;
        let mut water = 1.0f32;
        let mut sediment = 0.0f32;

        for _ in 0..max_steps {
            let nx0 = (px.floor() as usize).min(w - 2);
            let ny0 = (py.floor() as usize).min(h - 2);
            let fx = (px - nx0 as f32).clamp(0.0, 1.0);
            let fy = (py - ny0 as f32).clamp(0.0, 1.0);

            let (old_h, gx, gy) = sample_hg(heights, px, py);
            flow[ny0 * w + nx0] += water;

            dx = dx * inertia - gx * (1.0 - inertia);
            dy = dy * inertia - gy * (1.0 - inertia);
            let len = (dx * dx + dy * dy).sqrt();
            if len < 1e-6 {
                let ang = next_f32() * std::f32::consts::TAU;
                dx = ang.cos();
                dy = ang.sin();
            } else {
                dx /= len;
                dy /= len;
            }

            let new_px = px + dx;
            let new_py = py + dy;
            if new_px < 1.0 || new_px >= (w - 2) as f32 || new_py < 1.0 || new_py >= (h - 2) as f32 {
                break;
            }

            let (new_h, _, _) = sample_hg(heights, new_px, new_py);
            let dh = new_h - old_h;

            let cap = (-dh).max(0.004) * speed * water * capacity_factor;
            if sediment > cap || dh > 0.0 {
                let deposit = if dh > 0.0 {
                    dh.min(sediment)
                } else {
                    (sediment - cap) * deposition_rate
                };
                sediment -= deposit;
                heights[ny0 * w + nx0] += deposit * (1.0 - fx) * (1.0 - fy);
                heights[ny0 * w + nx0 + 1] += deposit * fx * (1.0 - fy);
                heights[(ny0 + 1) * w + nx0] += deposit * (1.0 - fx) * fy;
                heights[(ny0 + 1) * w + nx0 + 1] += deposit * fx * fy;
            } else {
                let erode = ((cap - sediment) * erosion_rate).min(-dh * 0.95);
                for &(bx, by, bwt) in &brush {
                    let tx = nx0 as isize + bx;
                    let ty = ny0 as isize + by;
                    if tx >= 0 && ty >= 0 && tx < w as isize && ty < h as isize {
                        let idx = ty as usize * w + tx as usize;
                        let delta = (erode * bwt).min(heights[idx]);
                        heights[idx] -= delta;
                        sediment += delta;
                    }
                }
            }

            speed = (speed * speed + (-dh).max(0.0) * 4.0).sqrt().clamp(0.1, 8.0);
            water *= 1.0 - evaporation_rate;
            if water < 0.01 {
                break;
            }
            px = new_px;
            py = new_py;
        }
    }
}

fn run_thermal_erosion(heights: &mut [f32], w: usize, h: usize, iters: usize, talus: f32) {
    let mut delta = vec![0.0f32; heights.len()];
    const NBRS: [(isize, isize); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for _ in 0..iters {
        delta.fill(0.0);
        for y in 1..(h - 1) {
            for x in 1..(w - 1) {
                let hc = heights[y * w + x];
                let mut max_diff = 0.0f32;
                let mut total_excess = 0.0f32;
                let mut diffs = [0.0f32; 4];
                for (k, &(dx, dy)) in NBRS.iter().enumerate() {
                    let nh = heights[(y as isize + dy) as usize * w + (x as isize + dx) as usize];
                    let d = hc - nh;
                    if d > talus {
                        diffs[k] = d - talus;
                        total_excess += d - talus;
                        max_diff = max_diff.max(d);
                    }
                }
                if total_excess > 1e-6 {
                    let move_amt = 0.35 * (max_diff - talus);
                    delta[y * w + x] -= move_amt;
                    for (k, &(dx, dy)) in NBRS.iter().enumerate() {
                        if diffs[k] > 0.0 {
                            let ni = (y as isize + dy) as usize * w + (x as isize + dx) as usize;
                            delta[ni] += move_amt * (diffs[k] / total_excess);
                        }
                    }
                }
            }
        }
        for i in 0..heights.len() {
            heights[i] += delta[i];
        }
    }
}

fn compute_slopes_deg(heights: &[f32], w: usize, h: usize, world: [f32; 3]) -> Vec<f32> {
    let dx_world = world[0] / (w.saturating_sub(1).max(1)) as f32;
    let dz_world = world[2] / (h.saturating_sub(1).max(1)) as f32;
    let mut out = vec![0.0f32; w * h];
    for y in 0..h {
        let ym = y.saturating_sub(1);
        let yp = (y + 1).min(h - 1);
        for x in 0..w {
            let xm = x.saturating_sub(1);
            let xp = (x + 1).min(w - 1);
            let hl = heights[y * w + xm] * world[1];
            let hr = heights[y * w + xp] * world[1];
            let hu = heights[ym * w + x] * world[1];
            let hd = heights[yp * w + x] * world[1];
            let gx = (hr - hl) / (2.0 * dx_world);
            let gz = (hd - hu) / (2.0 * dz_world);
            out[y * w + x] = (gx * gx + gz * gz).sqrt().atan().to_degrees();
        }
    }
    out
}

fn soft_band(val: f32, lo: f32, hi: f32, sharpness: f32) -> f32 {
    if lo <= 0.0 && hi >= 1.0 && sharpness > 0.0 {
        return 1.0;
    }
    let s = sharpness.max(1.0);
    let a = if lo <= 0.0 { 1.0 } else { 1.0 / (1.0 + (-(val - lo) * s).exp()) };
    let b = if hi >= 1.0 { 1.0 } else { 1.0 / (1.0 + ((val - hi) * s).exp()) };
    a * b
}

fn compute_rule_splats(
    heights: &[f32],
    slopes_deg: &[f32],
    flow: &[f32],
    rules: &[SplatRule],
) -> Vec<[f32; 4]> {
    let mut out = vec![[0.0f32; 4]; heights.len()];
    for i in 0..heights.len() {
        let hv = heights[i];
        let sv = slopes_deg[i] / 90.0;
        let fv = flow[i];
        let mut acc = [0.0f32; 4];
        for r in rules {
            let wh = soft_band(hv, r.h_min, r.h_max, r.sharpness);
            let ws = soft_band(sv, r.slope_min / 90.0, r.slope_max / 90.0, r.sharpness);
            let wf = soft_band(fv, r.flow_min, r.flow_max, r.sharpness);
            acc[r.channel] += wh * ws * wf * r.weight;
        }
        let sum = acc[0] + acc[1] + acc[2] + acc[3];
        if sum > 1e-6 {
            for c in &mut acc {
                *c /= sum;
            }
        } else {
            acc = [0.0, 1.0, 0.0, 0.0];
        }
        out[i] = acc;
    }
    out
}

fn compute_default_splats(heights: &[f32], slopes_deg: &[f32], flow: &[f32]) -> Vec<[f32; 4]> {
    let rules = vec![
        SplatRule {
            channel: 0,
            mat_name: "sand_warm".into(),
            h_min: 0.0,
            h_max: 0.16,
            slope_min: 0.0,
            slope_max: 26.0,
            flow_min: 0.0,
            flow_max: 1.0,
            weight: 1.2,
            sharpness: 14.0,
        },
        SplatRule {
            channel: 1,
            mat_name: "grass_mid".into(),
            h_min: 0.10,
            h_max: 0.68,
            slope_min: 0.0,
            slope_max: 30.0,
            flow_min: 0.0,
            flow_max: 1.0,
            weight: 1.0,
            sharpness: 12.0,
        },
        SplatRule {
            channel: 2,
            mat_name: "stone_mid".into(),
            h_min: 0.0,
            h_max: 1.0,
            slope_min: 26.0,
            slope_max: 90.0,
            flow_min: 0.0,
            flow_max: 1.0,
            weight: 1.5,
            sharpness: 14.0,
        },
        SplatRule {
            channel: 3,
            mat_name: "roof_cream".into(),
            h_min: 0.72,
            h_max: 1.0,
            slope_min: 0.0,
            slope_max: 42.0,
            flow_min: 0.0,
            flow_max: 1.0,
            weight: 1.3,
            sharpness: 14.0,
        },
    ];
    compute_rule_splats(heights, slopes_deg, flow, &rules)
}
