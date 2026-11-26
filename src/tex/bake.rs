use std::collections::HashMap;
use std::path::Path;

use crate::common::math::{Mat4, Vec2, Vec3};
use crate::common::png::{encode_gray16, encode_rgb};
use crate::mesh::geom::Mesh;
use crate::mesh::palette::Palette;
use crate::mesh::parser::compile_model_with_dir;
use crate::mesh::primitives::{build_box, FaceMats};
use crate::svg::parser::load_svg_file;
use crate::svg::raster::render_svg_rgba_tiled;
use crate::tex::parser::{
    BlendMode, LayerMask, NormalConvention, StonesSpec, TexLayer, TexSpec, WrapMode,
};

#[derive(Debug, Clone)]
pub struct BakedTexture {
    pub name: String,
    pub width: usize,
    pub height: usize,
    pub tile_w: f32,
    pub tile_h: f32,
    pub depth: f32,
    pub wrap: WrapMode,
    pub normal_conv: NormalConvention,
    /// Linear RGB albedo `[r, g, b]` in `0..=1`
    pub albedo: Vec<[f32; 3]>,
    /// Tangent-space unit normal `[nx, ny, nz]` in OpenGL convention (`+X` right, `+Y` up, `+Z` out)
    pub normal: Vec<[f32; 3]>,
    /// World-space relief height in meters (`0..depth`)
    pub height_m: Vec<f32>,
    /// Normalized height `0..=1`
    pub height_norm: Vec<f32>,
    /// Final ambient occlusion `0..=1`
    pub ao: Vec<f32>,
    /// Roughness `0..=1`
    pub roughness: Vec<f32>,
    /// Metallic `0..=1`
    pub metallic: Vec<f32>,
    pub tri_count: usize,
    pub seam_albedo: f32,
    pub seam_normal: f32,
    pub seam_height: f32,
    pub ao_min: f32,
    pub ao_max: f32,
}

#[derive(Clone)]
struct BakeTri {
    a: Vec3,
    b: Vec3,
    c: Vec3,
    na: Vec3,
    nb: Vec3,
    nc: Vec3,
    albedo: [f32; 3], // linear RGB
    rough: f32,
    metal: f32,
    mat_ao: f32,
}

struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_add(0x9E37_79B9_7F4A_7C15))
    }
    fn next_u64(&mut self) -> u64 {
        let mut z = self.0;
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn f32(&mut self) -> f32 {
        ((self.next_u64() >> 40) as f32) * (1.0 / 16777216.0)
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * self.f32()
    }
}

#[inline]
fn srgb_to_linear(u: u8) -> f32 {
    (u as f32 / 255.0).powf(2.2)
}

#[inline]
fn linear_to_srgb_u8(x: f32) -> u8 {
    let c = x.clamp(0.0, 1.0).powf(1.0 / 2.2);
    (c * 255.0).round().clamp(0.0, 255.0) as u8
}

#[inline]
fn hash2i(x: i32, y: i32, seed: u64) -> f32 {
    let mut h = (x as i64 as u64)
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (y as i64 as u64).wrapping_mul(0xBF58_476D_1CE4_E5B9)
        ^ seed.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h >> 33;
    h = h.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
    h ^= h >> 33;
    ((h >> 40) as f32) * (1.0 / 16777216.0)
}

/// Strictly periodic 2D value noise over `[0..1) x [0..1)` when `period_x` / `period_y` > 0.
fn periodic_noise(u: f32, v: f32, freq_x: i32, freq_y: i32, wrap_x: bool, wrap_y: bool, seed: u64) -> f32 {
    let fx = freq_x.max(1);
    let fy = freq_y.max(1);
    let x = u * fx as f32;
    let y = v * fy as f32;
    let x0 = x.floor() as i32;
    let y0 = y.floor() as i32;
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let sx = tx * tx * (3.0 - 2.0 * tx);
    let sy = ty * ty * (3.0 - 2.0 * ty);

    let wx = |ix: i32| if wrap_x { ix.rem_euclid(fx) } else { ix };
    let wy = |iy: i32| if wrap_y { iy.rem_euclid(fy) } else { iy };

    let c00 = hash2i(wx(x0), wy(y0), seed);
    let c10 = hash2i(wx(x0 + 1), wy(y0), seed);
    let c01 = hash2i(wx(x0), wy(y0 + 1), seed);
    let c11 = hash2i(wx(x0 + 1), wy(y0 + 1), seed);

    let nx0 = c00 + (c10 - c00) * sx;
    let nx1 = c01 + (c11 - c01) * sx;
    nx0 + (nx1 - nx0) * sy
}

fn periodic_fbm(
    u: f32,
    v: f32,
    base_freq: f32,
    octaves: usize,
    wrap_x: bool,
    wrap_y: bool,
    seed: u64,
) -> f32 {
    let f0 = base_freq.round().max(1.0) as i32;
    let mut amp = 0.5f32;
    let mut sum = 0.0f32;
    let mut norm = 0.0f32;
    let mut mult = 1i32;
    for o in 0..octaves {
        sum += amp * periodic_noise(u, v, f0 * mult, f0 * mult, wrap_x, wrap_y, seed.wrapping_add(o as u64 * 997));
        norm += amp;
        amp *= 0.5;
        mult *= 2;
    }
    if norm > 0.0 { sum / norm } else { 0.5 }
}

fn sample_palette_pbr(palette: &Palette, uv: Vec2) -> ([f32; 3], f32, f32, f32) {
    let bc = palette.sample_basecolor_nearest(uv);
    let orm = palette.sample_orm_nearest(uv);
    let lin = [
        srgb_to_linear(bc[0]),
        srgb_to_linear(bc[1]),
        srgb_to_linear(bc[2]),
    ];
    (lin, orm[1], orm[2], orm[0])
}

fn generate_stones_tris(
    st: &StonesSpec,
    tile_w: f32,
    tile_h: f32,
    palette: &Palette,
    out: &mut Vec<BakeTri>,
) -> Result<(), String> {
    let mut rng = Rng::new(st.seed);
    let (y0, y1) = st.y_band.unwrap_or((-tile_h * 0.5, tile_h * 0.5));
    let band_h = (y1 - y0).max(0.05);
    let row_h = band_h / st.rows as f32;

    // Resolve palette swatches for stones + optional mortar backing
    let mut mat_pbr = Vec::with_capacity(st.mats.len());
    for m in &st.mats {
        let rm = palette.resolve_mat(m)?;
        mat_pbr.push((rm, sample_palette_pbr(palette, rm.uv)));
    }

    if let Some(ref mortar_name) = st.mortar {
        let rm = palette.resolve_mat(mortar_name)?;
        let (alb, rgh, met, ao) = sample_palette_pbr(palette, rm.uv);
        let m_box = build_box(
            Vec3::new(tile_w * 1.1, band_h * 1.04, 0.02),
            Vec2::new(1.0, 1.0),
            Vec2::ZERO,
            0.0,
            rm,
            &FaceMats::default(),
        );
        let yc = (y0 + y1) * 0.5;
        for t in &m_box.tris {
            out.push(BakeTri {
                a: t.a + Vec3::new(0.0, yc, -0.01),
                b: t.b + Vec3::new(0.0, yc, -0.01),
                c: t.c + Vec3::new(0.0, yc, -0.01),
                na: t.na,
                nb: t.nb,
                nc: t.nc,
                albedo: alb,
                rough: rgh,
                metal: met,
                mat_ao: ao,
            });
        }
    }

    // Generate staggered rows with per-course jittered stone widths that sum strictly to `tile_w`!
    for row in 0..st.rows {
        let row_y = y0 + (row as f32 + 0.5) * row_h;
        let row_shift = ((row as f32) * st.stagger * (tile_w / st.cols as f32)) % tile_w;

        let mut weights = Vec::with_capacity(st.cols);
        let mut w_sum = 0.0f32;
        for _ in 0..st.cols {
            let w = 1.0 + rng.range(-st.jitter_x, st.jitter_x);
            weights.push(w.max(0.45));
            w_sum += w.max(0.45);
        }

        let mut cursor_x = -tile_w * 0.5 + row_shift;
        let mut prev_mat = usize::MAX;

        for col in 0..st.cols {
            let cell_w = tile_w * (weights[col] / w_sum);
            let cx = cursor_x + cell_w * 0.5;
            cursor_x += cell_w;

            // Wrap center back into [-tile_w/2 .. +tile_w/2]
            let mut cx_w = cx;
            while cx_w > tile_w * 0.5 {
                cx_w -= tile_w;
            }
            while cx_w < -tile_w * 0.5 {
                cx_w += tile_w;
            }

            let sx = (cell_w - st.gap).max(0.03);
            let sy = (row_h - st.gap * (1.0 + rng.range(-st.jitter_y, st.jitter_y))).max(0.03);
            let sz = rng.range(st.h_min, st.h_max);
            let bev = st.bevel.min(sx * 0.32).min(sy * 0.32).min(sz * 0.42);

            // Pick a palette material avoiding immediate horizontal repeats when multiple swatches exist
            let mut mi = (rng.next_u64() as usize) % mat_pbr.len();
            if mat_pbr.len() > 1 && mi == prev_mat {
                mi = (mi + 1) % mat_pbr.len();
            }
            prev_mat = mi;
            let (rm, (base_alb, base_rgh, base_met, base_ao)) = mat_pbr[mi];

            // Subtle organic value + warm/cool stone color variation
            let v_shift = 1.0 + rng.range(-st.vary, st.vary);
            let w_shift = rng.range(-st.vary * 0.35, st.vary * 0.35);
            let alb = [
                (base_alb[0] * (v_shift + w_shift)).clamp(0.0, 1.0),
                (base_alb[1] * v_shift).clamp(0.0, 1.0),
                (base_alb[2] * (v_shift - w_shift)).clamp(0.0, 1.0),
            ];
            let rgh = (base_rgh + rng.range(-0.05, 0.05)).clamp(0.08, 0.99);

            // Slight organic trapezoidal taper & hand-hewn bevels
            let taper = Vec2::new(
                rng.range(0.95, 1.02),
                rng.range(0.95, 1.02),
            );
            let mut stone_mesh = build_box(
                Vec3::new(sx, sy, sz),
                taper,
                Vec2::ZERO,
                bev,
                rm,
                &FaceMats::default(),
            );
            stone_mesh.compute_weighted_normals(4.0);

            let rx = rng.range(-st.tilt_deg, st.tilt_deg);
            let ry = rng.range(-st.tilt_deg, st.tilt_deg);
            let rz = rng.range(-st.tilt_deg * 0.3, st.tilt_deg * 0.3);
            let dy = rng.range(-row_h * st.jitter_y * 0.25, row_h * st.jitter_y * 0.25);
            // Center of box at Z = sz*0.5 so back sits cleanly at Z = 0
            let tf = Mat4::from_trs(
                Vec3::new(cx_w, row_y + dy, sz * 0.5),
                Vec3::new(rx, ry, rz),
                Vec3::ONE,
            );

            for t in &stone_mesh.tris {
                let a = tf.transform_point(t.a);
                let b = tf.transform_point(t.b);
                let c = tf.transform_point(t.c);
                let na = tf.transform_normal(t.na).normalized();
                let nb = tf.transform_normal(t.nb).normalized();
                let nc = tf.transform_normal(t.nc).normalized();
                out.push(BakeTri {
                    a,
                    b,
                    c,
                    na,
                    nb,
                    nc,
                    albedo: alb,
                    rough: rgh,
                    metal: base_met,
                    mat_ao: base_ao,
                });
            }
        }
    }

    Ok(())
}

fn append_mdl_mesh_tris(mesh: &Mesh, palette: &Palette, vary: f32, seed: u64, out: &mut Vec<BakeTri>) {
    let flat = mesh.flattened_visual();
    for t in &flat.tris {
        let (base_alb, base_rgh, base_met, base_ao) = sample_palette_pbr(palette, t.uv);
        let alb = if vary > 1e-4 {
            // Subtle deterministic per-part_tag tint variation if tagged
            if let Some(tid) = t.part_tag {
                let h = hash2i(tid as i32, 7, seed);
                let v = 1.0 + (h * 2.0 - 1.0) * vary;
                [
                    (base_alb[0] * v).clamp(0.0, 1.0),
                    (base_alb[1] * v).clamp(0.0, 1.0),
                    (base_alb[2] * v).clamp(0.0, 1.0),
                ]
            } else {
                base_alb
            }
        } else {
            base_alb
        };
        out.push(BakeTri {
            a: t.a,
            b: t.b,
            c: t.c,
            na: t.na,
            nb: t.nb,
            nc: t.nc,
            albedo: alb,
            rough: base_rgh,
            metal: base_met,
            mat_ao: base_ao,
        });
    }
}

/// Toroidally wrap triangles across `±tile_w` / `±tile_h` so edge-crossing geometry tiles 100% seamlessly.
fn wrap_triangles(src: &[BakeTri], tile_w: f32, tile_h: f32, wrap: WrapMode) -> Vec<BakeTri> {
    let hx = tile_w * 0.5;
    let hy = tile_h * 0.5;
    let dx_list: &[f32] = if wrap.wrap_x() { &[-tile_w, 0.0, tile_w] } else { &[0.0] };
    let dy_list: &[f32] = if wrap.wrap_y() { &[-tile_h, 0.0, tile_h] } else { &[0.0] };

    let mut out = Vec::with_capacity(src.len() * 2);
    for t in src {
        let min_x = t.a.x.min(t.b.x).min(t.c.x);
        let max_x = t.a.x.max(t.b.x).max(t.c.x);
        let min_y = t.a.y.min(t.b.y).min(t.c.y);
        let max_y = t.a.y.max(t.b.y).max(t.c.y);
        for &dy in dy_list {
            if max_y + dy < -hy || min_y + dy > hy {
                continue;
            }
            for &dx in dx_list {
                if max_x + dx < -hx || min_x + dx > hx {
                    continue;
                }
                let off = Vec3::new(dx, dy, 0.0);
                out.push(BakeTri {
                    a: t.a + off,
                    b: t.b + off,
                    c: t.c + off,
                    ..*t
                });
            }
        }
    }
    out
}

#[inline]
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

/// Periodic separable box blur (3 passes approximate a clean Gaussian blur with zero tile seams!).
fn periodic_blur(buf: &mut [f32], w: usize, h: usize, radius_px: f32, wrap_x: bool, wrap_y: bool) {
    let r = radius_px.round() as isize;
    if r <= 0 {
        return;
    }
    let mut tmp = vec![0.0f32; w * h];
    let inv = 1.0 / (2 * r + 1) as f32;
    for _pass in 0..3 {
        // Horizontal
        for y in 0..h {
            let row = y * w;
            for x in 0..w {
                let mut acc = 0.0f32;
                for dx in -r..=r {
                    let sx = if wrap_x {
                        (x as isize + dx).rem_euclid(w as isize) as usize
                    } else {
                        (x as isize + dx).clamp(0, (w - 1) as isize) as usize
                    };
                    acc += buf[row + sx];
                }
                tmp[row + x] = acc * inv;
            }
        }
        // Vertical
        for y in 0..h {
            for x in 0..w {
                let mut acc = 0.0f32;
                for dy in -r..=r {
                    let sy = if wrap_y {
                        (y as isize + dy).rem_euclid(h as isize) as usize
                    } else {
                        (y as isize + dy).clamp(0, (h - 1) as isize) as usize
                    };
                    acc += tmp[sy * w + x];
                }
                buf[y * w + x] = acc * inv;
            }
        }
    }
}

#[inline]
fn sample_periodic(buf: &[f32], w: usize, h: usize, x: isize, y: isize, wrap_x: bool, wrap_y: bool) -> f32 {
    let sx = if wrap_x {
        x.rem_euclid(w as isize) as usize
    } else {
        x.clamp(0, (w - 1) as isize) as usize
    };
    let sy = if wrap_y {
        y.rem_euclid(h as isize) as usize
    } else {
        y.clamp(0, (h - 1) as isize) as usize
    };
    buf[sy * w + sx]
}

/// Apply height perturbation `dh` (meters) onto tangent-space normals using periodic central differences.
fn apply_height_bump_to_normals(
    normals: &mut [[f32; 3]],
    dh: &[f32],
    w: usize,
    h: usize,
    tile_w: f32,
    tile_h: f32,
    wrap_x: bool,
    wrap_y: bool,
) {
    let dx = tile_w / w as f32;
    let dy = tile_h / h as f32;
    let inv_2dx = 0.5 / dx;
    let inv_2dy = 0.5 / dy;

    for y in 0..h {
        let yi = y as isize;
        for x in 0..w {
            let xi = x as isize;
            let hl = sample_periodic(dh, w, h, xi - 1, yi, wrap_x, wrap_y);
            let hr = sample_periodic(dh, w, h, xi + 1, yi, wrap_x, wrap_y);
            let hu = sample_periodic(dh, w, h, xi, yi - 1, wrap_x, wrap_y); // +Y up
            let hd = sample_periodic(dh, w, h, xi, yi + 1, wrap_x, wrap_y); // -Y down
            // Tangent-space normal perturbation from dH/dx, dH/dy (OpenGL +Y up)
            let pnx = (hl - hr) * inv_2dx;
            let pny = (hd - hu) * inv_2dy;
            if pnx.abs() < 1e-6 && pny.abs() < 1e-6 {
                continue;
            }
            let i = y * w + x;
            let n = normals[i];
            let nx = n[0] + pnx;
            let ny = n[1] + pny;
            let nz = n[2].max(0.15);
            let inv_l = 1.0 / (nx * nx + ny * ny + nz * nz).sqrt().max(1e-6);
            normals[i] = [nx * inv_l, ny * inv_l, (nz * inv_l).max(0.05)];
        }
    }
}

/// Compute periodic Horizon-Based Ambient Occlusion (HBAO) & multi-scale curvature from the heightfield.
fn compute_hbao_and_curvature(
    height_m: &[f32],
    w: usize,
    h: usize,
    tile_w: f32,
    tile_h: f32,
    depth: f32,
    ao_radius_m: f32,
    ao_strength: f32,
    wrap_x: bool,
    wrap_y: bool,
) -> (Vec<f32>, Vec<f32>) {
    let px_w = tile_w / w as f32;
    let px_h = tile_h / h as f32;
    let max_rad_px = (ao_radius_m / px_w.min(px_h)).clamp(4.0, 72.0);

    const DIRS: usize = 12;
    const STEPS: usize = 6;
    let mut dir_cos = [0.0f32; DIRS];
    let mut dir_sin = [0.0f32; DIRS];
    for d in 0..DIRS {
        let ang = (d as f32) * std::f32::consts::TAU / DIRS as f32 + 0.13;
        dir_cos[d] = ang.cos();
        dir_sin[d] = ang.sin();
    }

    let mut ao = vec![1.0f32; w * h];
    let mut curv = vec![0.0f32; w * h];

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .max(1);
    let band_rows = (h + threads - 1) / threads;

    std::thread::scope(|scope| {
        let mut ao_rest = ao.as_mut_slice();
        let mut cv_rest = curv.as_mut_slice();
        let mut y0 = 0usize;
        while y0 < h {
            let rows = band_rows.min(h - y0);
            let (ao_band, ao_tail) = ao_rest.split_at_mut(rows * w);
            let (cv_band, cv_tail) = cv_rest.split_at_mut(rows * w);
            ao_rest = ao_tail;
            cv_rest = cv_tail;

            scope.spawn(move || {
                let r1 = (w as f32 * 0.0025).round().max(2.0) as isize;
                let r2 = (w as f32 * 0.0065).round().max(5.0) as isize;
                let inv_d = 1.0 / depth.max(0.01);

                for ly in 0..rows {
                    let y = y0 + ly;
                    let yi = y as isize;
                    for x in 0..w {
                        let xi = x as isize;
                        let z0 = height_m[y * w + x];

                        // Horizon-based AO
                        let mut occ_sum = 0.0f32;
                        for d in 0..DIRS {
                            let dx = dir_cos[d];
                            let dy = dir_sin[d];
                            let mut max_sin = 0.0f32;
                            for s in 1..=STEPS {
                                let frac = (s as f32 / STEPS as f32).powf(1.35);
                                let rpx = (frac * max_rad_px).max(1.0);
                                let sx = (x as f32 + dx * rpx).round() as isize;
                                let sy = (y as f32 + dy * rpx).round() as isize;
                                let zs = sample_periodic(height_m, w, h, sx, sy, wrap_x, wrap_y);
                                let dz = zs - z0;
                                if dz > 1e-4 {
                                    let dist_m = ((dx * rpx * px_w).powi(2) + (dy * rpx * px_h).powi(2)).sqrt();
                                    let sin_h = dz / (dz * dz + dist_m * dist_m).sqrt();
                                    let falloff = 1.0 - frac * 0.45;
                                    max_sin = max_sin.max(sin_h * falloff);
                                }
                            }
                            occ_sum += max_sin;
                        }
                        let occ = (occ_sum / DIRS as f32) * ao_strength;
                        ao_band[ly * w + x] = (1.0 - occ).clamp(0.08, 1.0);

                        // Multi-scale Laplacian curvature: + convex bevel edges, - mortar cavities
                        let avg1 = 0.25
                            * (sample_periodic(height_m, w, h, xi - r1, yi, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi + r1, yi, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi, yi - r1, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi, yi + r1, wrap_x, wrap_y));
                        let avg2 = 0.25
                            * (sample_periodic(height_m, w, h, xi - r2, yi, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi + r2, yi, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi, yi - r2, wrap_x, wrap_y)
                                + sample_periodic(height_m, w, h, xi, yi + r2, wrap_x, wrap_y));
                        let c = ((z0 - avg1) * 5.0 + (z0 - avg2) * 3.0) * inv_d;
                        cv_band[ly * w + x] = c.clamp(-1.0, 1.0);
                    }
                }
            });
            y0 += rows;
        }
    });

    (ao, curv)
}

#[inline]
fn eval_mask(
    mask: LayerMask,
    h_norm: f32,
    ao: f32,
    curv: f32,
    normal: [f32; 3],
) -> f32 {
    match mask {
        LayerMask::All => 1.0,
        LayerMask::Cavity => {
            let low_h = (1.0 - h_norm * 2.2).clamp(0.0, 1.0);
            let dark_ao = ((0.78 - ao) * 2.4).clamp(0.0, 1.0);
            let neg_c = (-curv * 2.0).clamp(0.0, 1.0);
            (low_h * 0.5 + dark_ao * 0.5 + neg_c * 0.35).clamp(0.0, 1.0)
        }
        LayerMask::Edge => ((curv - 0.08) * 3.5).clamp(0.0, 1.0),
        LayerMask::Up => ((normal[1] - 0.12) * 1.8).clamp(0.0, 1.0),
        LayerMask::High => ((h_norm - 0.28) * 2.5).clamp(0.0, 1.0),
        LayerMask::Low => ((0.45 - h_norm) * 2.5).clamp(0.0, 1.0),
    }
}

fn measure_seam_scalar(buf: &[f32], w: usize, h: usize, wrap_x: bool, wrap_y: bool) -> f32 {
    let mut sum = 0.0f32;
    let mut cnt = 0usize;
    if wrap_x && w >= 4 {
        for y in 0..h {
            let row = y * w;
            let border = (buf[row] - buf[row + w - 1]).abs();
            let inner = 0.5 * ((buf[row + 1] - buf[row]).abs() + (buf[row + w - 1] - buf[row + w - 2]).abs());
            sum += (border - inner).max(0.0);
            cnt += 1;
        }
    }
    if wrap_y && h >= 4 {
        for x in 0..w {
            let border = (buf[x] - buf[(h - 1) * w + x]).abs();
            let inner = 0.5 * ((buf[w + x] - buf[x]).abs() + (buf[(h - 1) * w + x] - buf[(h - 2) * w + x]).abs());
            sum += (border - inner).max(0.0);
            cnt += 1;
        }
    }
    if cnt > 0 { sum / cnt as f32 } else { 0.0 }
}

fn measure_seam_vec3(buf: &[[f32; 3]], w: usize, h: usize, wrap_x: bool, wrap_y: bool) -> f32 {
    let dist = |a: [f32; 3], b: [f32; 3]| {
        ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
    };
    let mut sum = 0.0f32;
    let mut cnt = 0usize;
    if wrap_x && w >= 4 {
        for y in 0..h {
            let row = y * w;
            let border = dist(buf[row], buf[row + w - 1]);
            let inner = 0.5 * (dist(buf[row + 1], buf[row]) + dist(buf[row + w - 1], buf[row + w - 2]));
            sum += (border - inner).max(0.0);
            cnt += 1;
        }
    }
    if wrap_y && h >= 4 {
        for x in 0..w {
            let border = dist(buf[x], buf[(h - 1) * w + x]);
            let inner = 0.5 * (dist(buf[w + x], buf[x]) + dist(buf[(h - 1) * w + x], buf[(h - 2) * w + x]));
            sum += (border - inner).max(0.0);
            cnt += 1;
        }
    }
    if cnt > 0 { sum / cnt as f32 } else { 0.0 }
}

pub fn bake_texture(spec: &TexSpec, pal_override: Option<&Path>) -> Result<BakedTexture, String> {
    let pal_path = if let Some(p) = pal_override {
        p.to_path_buf()
    } else if let Some(ref rel) = spec.palette_rel {
        spec.base_dir.join(rel)
    } else {
        return Err("tex requires a palette via 'pal <file.pal>' or -p <file.pal>".into());
    };
    let palette = Palette::load_file(&pal_path)?;

    let mut raw_tris: Vec<BakeTri> = Vec::new();

    // 1. Optional external .mdl file
    if let Some(ref mrel) = spec.mesh_rel {
        let mp = spec.base_dir.join(mrel);
        let msrc = std::fs::read_to_string(&mp)
            .map_err(|e| format!("failed to read mesh '{}': {e}", mp.display()))?;
        let mdir = mp.parent().unwrap_or(&spec.base_dir);
        let parsed = compile_model_with_dir(&msrc, &spec.name, &palette, Some(mdir), &HashMap::new())?;
        append_mdl_mesh_tris(&parsed.mesh, &palette, spec.vary, spec.seed, &mut raw_tris);
    }

    // 2. Optional inline `mesh ... end` block
    if let Some(ref inline) = spec.inline_mdl {
        let mut full_src = String::new();
        for vl in &spec.var_lines {
            full_src.push_str(vl);
            full_src.push('\n');
        }
        full_src.push_str(inline);
        let parsed = compile_model_with_dir(&full_src, &spec.name, &palette, Some(&spec.base_dir), &HashMap::new())?;
        append_mdl_mesh_tris(&parsed.mesh, &palette, spec.vary, spec.seed, &mut raw_tris);
    }

    // 3. Optional procedural 3D `stones` generators
    for st in &spec.stones {
        generate_stones_tris(st, spec.tile_w, spec.tile_h, &palette, &mut raw_tris)?;
    }

    if raw_tris.is_empty() {
        return Err("tex produced 0 triangles (add `mesh ... end`, `mesh <file.mdl>`, or `stones ...`)".into());
    }

    let wrapped = wrap_triangles(&raw_tris, spec.tile_w, spec.tile_h, spec.wrap);
    let tri_count = wrapped.len();

    let w = spec.width.max(32) as usize;
    let h = spec.height.max(32) as usize;
    let ss = 2usize;
    let sw = w * ss;
    let sh = h * ss;
    let hx = spec.tile_w * 0.5;
    let hy = spec.tile_h * 0.5;
    let inv_w = sw as f32 / spec.tile_w;
    let inv_h = sh as f32 / spec.tile_h;

    // Pre-project triangles into supersampled orthographic pixel coordinates
    struct ProjTri {
        t_idx: usize,
        s0: (f32, f32),
        s1: (f32, f32),
        s2: (f32, f32),
        min_x: usize,
        max_x: usize,
        min_y: usize,
        max_y: usize,
        inv_area: f32,
    }

    let mut proj: Vec<ProjTri> = Vec::with_capacity(wrapped.len());
    for (ti, t) in wrapped.iter().enumerate() {
        let p0 = ((t.a.x + hx) * inv_w, (hy - t.a.y) * inv_h);
        let p1 = ((t.b.x + hx) * inv_w, (hy - t.b.y) * inv_h);
        let p2 = ((t.c.x + hx) * inv_w, (hy - t.c.y) * inv_h);
        // Outward CCW (+Z facing) triangles in screen-Y-down coords have area > 0
        let area = edge_fn(p0.0, p0.1, p1.0, p1.1, p2.0, p2.1);
        if area <= 1e-4 {
            continue;
        }
        let min_x = p0.0.min(p1.0).min(p2.0).floor().max(0.0) as usize;
        let max_x = p0.0.max(p1.0).max(p2.0).ceil().min((sw - 1) as f32) as usize;
        let min_y = p0.1.min(p1.1).min(p2.1).floor().max(0.0) as usize;
        let max_y = p0.1.max(p1.1).max(p2.1).ceil().min((sh - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y || min_x >= sw || min_y >= sh {
            continue;
        }
        proj.push(ProjTri {
            t_idx: ti,
            s0: p0,
            s1: p1,
            s2: p2,
            min_x,
            max_x,
            min_y,
            max_y,
            inv_area: 1.0 / area,
        });
    }

    // Final downsampled buffers
    let mut albedo = vec![[0.12f32, 0.11, 0.10]; w * h];
    let mut normal = vec![[0.0f32, 0.0, 1.0]; w * h];
    let mut height_m = vec![0.0f32; w * h];
    let mut roughness = vec![0.88f32; w * h];
    let mut metallic = vec![0.0f32; w * h];
    let mut mat_ao = vec![1.0f32; w * h];

    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .max(1);
    let band_rows = (h + threads - 1) / threads;

    std::thread::scope(|scope| {
        let mut alb_r = albedo.as_mut_slice();
        let mut nrm_r = normal.as_mut_slice();
        let mut hgt_r = height_m.as_mut_slice();
        let mut rgh_r = roughness.as_mut_slice();
        let mut met_r = metallic.as_mut_slice();
        let mut mao_r = mat_ao.as_mut_slice();
        let mut y0 = 0usize;

        while y0 < h {
            let rows = band_rows.min(h - y0);
            let (alb_b, alb_t) = alb_r.split_at_mut(rows * w);
            let (nrm_b, nrm_t) = nrm_r.split_at_mut(rows * w);
            let (hgt_b, hgt_t) = hgt_r.split_at_mut(rows * w);
            let (rgh_b, rgh_t) = rgh_r.split_at_mut(rows * w);
            let (met_b, met_t) = met_r.split_at_mut(rows * w);
            let (mao_b, mao_t) = mao_r.split_at_mut(rows * w);
            alb_r = alb_t;
            nrm_r = nrm_t;
            hgt_r = hgt_t;
            rgh_r = rgh_t;
            met_r = met_t;
            mao_r = mao_t;

            let proj_ref = &proj;
            let wrapped_ref = &wrapped;
            scope.spawn(move || {
                let sy0 = y0 * ss;
                let srows = rows * ss;
                let sy1 = sy0 + srows;
                let mut z_ss = vec![-1e9f32; sw * srows];
                let mut a_ss = vec![[0.12f32, 0.11, 0.10]; sw * srows];
                let mut n_ss = vec![[0.0f32, 0.0, 1.0]; sw * srows];
                let mut orm_ss = vec![[1.0f32, 0.88, 0.0]; sw * srows];

                for pt in proj_ref {
                    if pt.max_y < sy0 || pt.min_y >= sy1 {
                        continue;
                    }
                    let t = &wrapped_ref[pt.t_idx];
                    let yb0 = pt.min_y.max(sy0);
                    let yb1 = pt.max_y.min(sy1 - 1);
                    for sy in yb0..=yb1 {
                        let py = sy as f32 + 0.5;
                        let row_off = (sy - sy0) * sw;
                        for sx in pt.min_x..=pt.max_x {
                            let px = sx as f32 + 0.5;
                            let w0 = edge_fn(pt.s1.0, pt.s1.1, pt.s2.0, pt.s2.1, px, py);
                            if w0 < -1e-4 {
                                continue;
                            }
                            let w1 = edge_fn(pt.s2.0, pt.s2.1, pt.s0.0, pt.s0.1, px, py);
                            if w1 < -1e-4 {
                                continue;
                            }
                            let w2 = edge_fn(pt.s0.0, pt.s0.1, pt.s1.0, pt.s1.1, px, py);
                            if w2 < -1e-4 {
                                continue;
                            }
                            let l0 = w0 * pt.inv_area;
                            let l1 = w1 * pt.inv_area;
                            let l2 = w2 * pt.inv_area;
                            let z = t.a.z * l0 + t.b.z * l1 + t.c.z * l2;
                            let idx = row_off + sx;
                            if z > z_ss[idx] {
                                z_ss[idx] = z;
                                a_ss[idx] = t.albedo;
                                let n = (t.na * l0 + t.nb * l1 + t.nc * l2).normalized();
                                n_ss[idx] = [n.x, n.y, n.z.max(0.05)];
                                orm_ss[idx] = [t.mat_ao, t.rough, t.metal];
                            }
                        }
                    }
                }

                // 2x2 SSAA downsample
                for ly in 0..rows {
                    for x in 0..w {
                        let mut alb = [0.0f32; 3];
                        let mut nrm = [0.0f32; 3];
                        let mut zm = 0.0f32;
                        let mut ao_s = 0.0f32;
                        let mut rgh_s = 0.0f32;
                        let mut met_s = 0.0f32;
                        for oy in 0..ss {
                            let r = (ly * ss + oy) * sw;
                            for ox in 0..ss {
                                let i = r + x * ss + ox;
                                let zi = if z_ss[i] > -1e8 { z_ss[i].max(0.0) } else { 0.0 };
                                zm += zi;
                                alb[0] += a_ss[i][0];
                                alb[1] += a_ss[i][1];
                                alb[2] += a_ss[i][2];
                                nrm[0] += n_ss[i][0];
                                nrm[1] += n_ss[i][1];
                                nrm[2] += n_ss[i][2];
                                ao_s += orm_ss[i][0];
                                rgh_s += orm_ss[i][1];
                                met_s += orm_ss[i][2];
                            }
                        }
                        let inv_s = 1.0 / (ss * ss) as f32;
                        let nl = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt().max(1e-6);
                        let oi = ly * w + x;
                        alb_b[oi] = [alb[0] * inv_s, alb[1] * inv_s, alb[2] * inv_s];
                        nrm_b[oi] = [nrm[0] / nl, nrm[1] / nl, (nrm[2] / nl).max(0.05)];
                        hgt_b[oi] = zm * inv_s;
                        mao_b[oi] = ao_s * inv_s;
                        rgh_b[oi] = rgh_s * inv_s;
                        met_b[oi] = met_s * inv_s;
                    }
                }
            });
            y0 += rows;
        }
    });

    // Compute periodic HBAO & multi-scale curvature from the baked 3D relief
    let wrap_x = spec.wrap.wrap_x();
    let wrap_y = spec.wrap.wrap_y();
    let (mut ao, curv) = compute_hbao_and_curvature(
        &height_m,
        w,
        h,
        spec.tile_w,
        spec.tile_h,
        spec.depth,
        spec.ao_radius,
        spec.ao_strength,
        wrap_x,
        wrap_y,
    );
    for i in 0..ao.len() {
        ao[i] = (ao[i] * mat_ao[i]).clamp(0.05, 1.0);
    }

    // Apply post-bake geometry-aware detail passes & seamless SVG overlays in authored order!
    let mut height_norm = vec![0.0f32; w * h];
    let update_hnorm = |hm: &[f32], hn: &mut [f32], depth: f32| {
        let inv = 1.0 / depth.max(1e-4);
        for i in 0..hm.len() {
            hn[i] = (hm[i] * inv).clamp(0.0, 1.0);
        }
    };
    update_hnorm(&height_m, &mut height_norm, spec.depth);

    for layer in &spec.layers {
        match layer {
            TexLayer::Grain { scale, amp, rough, bump, mask, seed } => {
                let mut dh = vec![0.0f32; w * h];
                for y in 0..h {
                    let v = (y as f32 + 0.5) / h as f32;
                    for x in 0..w {
                        let u = (x as f32 + 0.5) / w as f32;
                        let i = y * w + x;
                        let m = eval_mask(*mask, height_norm[i], ao[i], curv[i], normal[i]);
                        if m <= 1e-4 {
                            continue;
                        }
                        let n = periodic_fbm(u, v, *scale, 4, wrap_x, wrap_y, *seed) * 2.0 - 1.0;
                        let f = 1.0 + n * (*amp) * m;
                        albedo[i] = [
                            (albedo[i][0] * f).clamp(0.0, 1.0),
                            (albedo[i][1] * f).clamp(0.0, 1.0),
                            (albedo[i][2] * f).clamp(0.0, 1.0),
                        ];
                        roughness[i] = (roughness[i] + n * (*rough) * m).clamp(0.04, 1.0);
                        if bump.abs() > 1e-6 {
                            let d = n * (*bump) * m;
                            dh[i] = d;
                            height_m[i] = (height_m[i] + d).max(0.0);
                        }
                    }
                }
                if bump.abs() > 1e-6 {
                    apply_height_bump_to_normals(&mut normal, &dh, w, h, spec.tile_w, spec.tile_h, wrap_x, wrap_y);
                    update_hnorm(&height_m, &mut height_norm, spec.depth);
                }
            }
            TexLayer::EdgeWear { color, strength, threshold, chip, rough, bump, scale, seed } => {
                let mut dh = vec![0.0f32; w * h];
                for y in 0..h {
                    let v = (y as f32 + 0.5) / h as f32;
                    for x in 0..w {
                        let u = (x as f32 + 0.5) / w as f32;
                        let i = y * w + x;
                        let c_pos = ((curv[i] - *threshold) / (1.0 - *threshold).max(0.1)).clamp(0.0, 1.0);
                        if c_pos <= 1e-4 {
                            continue;
                        }
                        let n_chip = periodic_fbm(u, v, *scale, 3, wrap_x, wrap_y, *seed);
                        let mod_f = 1.0 - *chip + *chip * ((n_chip - 0.32) * 2.2).clamp(0.0, 1.0);
                        let w_e = (c_pos * mod_f * (*strength)).clamp(0.0, 1.0);
                        if w_e <= 1e-4 {
                            continue;
                        }
                        for ch in 0..3 {
                            albedo[i][ch] = albedo[i][ch] * (1.0 - w_e) + color[ch] * w_e;
                        }
                        roughness[i] = roughness[i] * (1.0 - w_e) + (*rough) * w_e;
                        if bump.abs() > 1e-6 {
                            let d = w_e * (*bump);
                            dh[i] = d;
                            height_m[i] = (height_m[i] + d).max(0.0);
                        }
                    }
                }
                if bump.abs() > 1e-6 {
                    apply_height_bump_to_normals(&mut normal, &dh, w, h, spec.tile_w, spec.tile_h, wrap_x, wrap_y);
                    update_hnorm(&height_m, &mut height_norm, spec.depth);
                }
            }
            TexLayer::Cavity { color, strength, depth, rough } => {
                for i in 0..(w * h) {
                    let low_h = ((depth - height_norm[i]) / depth.max(0.05)).clamp(0.0, 1.0);
                    let dark_ao = ((0.76 - ao[i]) * 2.1).clamp(0.0, 1.0);
                    let m = (low_h * 0.65 + dark_ao * 0.55).clamp(0.0, 1.0) * (*strength);
                    if m <= 1e-4 {
                        continue;
                    }
                    for ch in 0..3 {
                        albedo[i][ch] = albedo[i][ch] * (1.0 - m) + color[ch] * m;
                    }
                    roughness[i] = roughness[i] * (1.0 - m) + (*rough) * m;
                    metallic[i] *= 1.0 - m;
                }
            }
            TexLayer::Moss {
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
            } => {
                let mut dh = vec![0.0f32; w * h];
                for y in 0..h {
                    let v = (y as f32 + 0.5) / h as f32;
                    let wy = (0.5 - v) * spec.tile_h;
                    let band_w = if let Some((by0, by1)) = y_band {
                        let margin = 0.08 * spec.tile_h;
                        let lo = ((wy - (by0 - margin)) / margin.max(1e-4)).clamp(0.0, 1.0);
                        let hi = (((by1 + margin) - wy) / margin.max(1e-4)).clamp(0.0, 1.0);
                        lo.min(hi)
                    } else {
                        1.0
                    };
                    if band_w <= 1e-4 {
                        continue;
                    }
                    for x in 0..w {
                        let u = (x as f32 + 0.5) / w as f32;
                        let i = y * w + x;
                        let n_macro = periodic_fbm(u, v, *scale, 4, wrap_x, wrap_y, *seed);
                        let n_tuft = periodic_fbm(u, v, (*scale) * 3.0, 3, wrap_x, wrap_y, seed.wrapping_add(77));
                        let cav_m = eval_mask(LayerMask::Cavity, height_norm[i], ao[i], curv[i], normal[i]);
                        let up_m = eval_mask(LayerMask::Up, height_norm[i], ao[i], curv[i], normal[i]);
                        let affinity = (cav_m * (*cavity_weight) + up_m * (*up_weight) + 0.15).clamp(0.0, 1.4);
                        let raw = n_macro * 0.72 + n_tuft * 0.28 + affinity * 0.45 - (1.0 - *coverage);
                        let m = (raw * 3.2).clamp(0.0, 1.0) * (*strength) * band_w;
                        if m <= 1e-4 {
                            continue;
                        }
                        let tint = n_tuft;
                        for ch in 0..3 {
                            let mc = color[ch] * (1.0 - tint) + alt_color[ch] * tint;
                            albedo[i][ch] = albedo[i][ch] * (1.0 - m) + mc * m;
                        }
                        roughness[i] = roughness[i] * (1.0 - m) + (*rough) * m;
                        metallic[i] *= 1.0 - m;
                        if bump.abs() > 1e-6 {
                            let d = m * (*bump) * (0.6 + 0.8 * n_tuft);
                            dh[i] = d;
                            height_m[i] = (height_m[i] + d).max(0.0);
                        }
                    }
                }
                if bump.abs() > 1e-6 {
                    apply_height_bump_to_normals(&mut normal, &dh, w, h, spec.tile_w, spec.tile_h, wrap_x, wrap_y);
                    update_hnorm(&height_m, &mut height_norm, spec.depth);
                }
            }
            TexLayer::Svg {
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
            } => {
                let svg_path = spec.base_dir.join(rel_path);
                let doc = load_svg_file(&svg_path)?;
                if !doc.is_ok() {
                    return Err(format!("SVG overlay '{}' invalid: {}", rel_path, doc.errors.join("; ")));
                }
                let swx = wrap.map(|wm| wm.wrap_x()).unwrap_or(wrap_x);
                let swy = wrap.map(|wm| wm.wrap_y()).unwrap_or(wrap_y);
                let rgba = render_svg_rgba_tiled(&doc, w as u32, h as u32, swx, swy);

                let mut dh = vec![0.0f32; w * h];
                let mut eff_a = vec![0.0f32; w * h];
                for i in 0..(w * h) {
                    let sa = rgba[i * 4 + 3] as f32 / 255.0;
                    if sa <= 1e-5 {
                        continue;
                    }
                    let gm = eval_mask(*mask, height_norm[i], ao[i], curv[i], normal[i]);
                    let a = (sa * gm * (*opacity)).clamp(0.0, 1.0);
                    eff_a[i] = a;
                    if a <= 1e-5 {
                        continue;
                    }
                    let sr = srgb_to_linear(rgba[i * 4]);
                    let sg = srgb_to_linear(rgba[i * 4 + 1]);
                    let sb = srgb_to_linear(rgba[i * 4 + 2]);
                    let src_c = [sr, sg, sb];
                    match blend {
                        BlendMode::Over => {
                            for ch in 0..3 {
                                albedo[i][ch] = albedo[i][ch] * (1.0 - a) + src_c[ch] * a;
                            }
                        }
                        BlendMode::Multiply => {
                            for ch in 0..3 {
                                albedo[i][ch] = albedo[i][ch] * (1.0 - a + src_c[ch] * a);
                            }
                        }
                        BlendMode::Add => {
                            for ch in 0..3 {
                                albedo[i][ch] = (albedo[i][ch] + src_c[ch] * a).clamp(0.0, 1.0);
                            }
                        }
                        BlendMode::Screen => {
                            for ch in 0..3 {
                                let sc = 1.0 - (1.0 - albedo[i][ch]) * (1.0 - src_c[ch]);
                                albedo[i][ch] = albedo[i][ch] * (1.0 - a) + sc * a;
                            }
                        }
                        BlendMode::HeightOnly => {}
                    }
                    if let Some(rv) = rough {
                        roughness[i] = roughness[i] * (1.0 - a) + (*rv) * a;
                    }
                    if let Some(mv) = metal {
                        metallic[i] = metallic[i] * (1.0 - a) + (*mv) * a;
                    }
                }

                if height.abs() > 1e-6 {
                    // Use luminance-weighted alpha so grayscale SVG shading smoothly sculpts bevel depths!
                    for i in 0..(w * h) {
                        if eff_a[i] > 1e-5 {
                            let lum = (0.299 * rgba[i * 4] as f32
                                + 0.587 * rgba[i * 4 + 1] as f32
                                + 0.114 * rgba[i * 4 + 2] as f32)
                                / 255.0;
                            let mod_l = if *blend == BlendMode::HeightOnly {
                                lum.max(0.15)
                            } else {
                                1.0
                            };
                            dh[i] = eff_a[i] * mod_l * (*height);
                        }
                    }
                    if *blur_px > 0.2 {
                        periodic_blur(&mut dh, w, h, *blur_px, swx, swy);
                    }
                    for i in 0..(w * h) {
                        if dh[i].abs() > 1e-7 {
                            height_m[i] = (height_m[i] + dh[i]).max(0.0);
                            // Carved negative grooves automatically deepen local cavity AO!
                            if dh[i] < 0.0 && *ao_strength > 0.0 {
                                let carve_frac = (-dh[i] / height.abs().max(1e-4)).clamp(0.0, 1.0);
                                ao[i] = (ao[i] * (1.0 - carve_frac * 0.42 * (*ao_strength))).clamp(0.06, 1.0);
                            }
                        }
                    }
                    apply_height_bump_to_normals(&mut normal, &dh, w, h, spec.tile_w, spec.tile_h, swx, swy);
                    update_hnorm(&height_m, &mut height_norm, spec.depth);
                }
            }
        }
    }

    // Final normalization of height_norm over actual baked depth range `[0..max(depth, max_z)]`
    let max_z = height_m.iter().copied().fold(spec.depth, f32::max).max(1e-4);
    for i in 0..(w * h) {
        height_norm[i] = (height_m[i] / max_z).clamp(0.0, 1.0);
    }

    let seam_albedo = measure_seam_vec3(&albedo, w, h, wrap_x, wrap_y);
    let seam_normal = measure_seam_vec3(&normal, w, h, wrap_x, wrap_y);
    let seam_height = measure_seam_scalar(&height_norm, w, h, wrap_x, wrap_y);
    let ao_min = ao.iter().copied().fold(1.0f32, f32::min);
    let ao_max = ao.iter().copied().fold(0.0f32, f32::max);

    Ok(BakedTexture {
        name: spec.name.clone(),
        width: w,
        height: h,
        tile_w: spec.tile_w,
        tile_h: spec.tile_h,
        depth: max_z,
        wrap: spec.wrap,
        normal_conv: spec.normal_conv,
        albedo,
        normal,
        height_m,
        height_norm,
        ao,
        roughness,
        metallic,
        tri_count,
        seam_albedo,
        seam_normal,
        seam_height,
        ao_min,
        ao_max,
    })
}

impl BakedTexture {
    pub fn encode_basecolor_png(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        for &c in &self.albedo {
            rgb.push(linear_to_srgb_u8(c[0]));
            rgb.push(linear_to_srgb_u8(c[1]));
            rgb.push(linear_to_srgb_u8(c[2]));
        }
        encode_rgb(self.width as u32, self.height as u32, &rgb)
    }

    pub fn encode_normal_png(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        let flip_y = self.normal_conv == NormalConvention::DirectX;
        for &n in &self.normal {
            let ny = if flip_y { -n[1] } else { n[1] };
            let enc = |v: f32| ((v.clamp(-1.0, 1.0) * 0.5 + 0.5) * 255.0).round().clamp(0.0, 255.0) as u8;
            rgb.push(enc(n[0]));
            rgb.push(enc(ny));
            rgb.push(enc(n[2].max(0.0)));
        }
        encode_rgb(self.width as u32, self.height as u32, &rgb)
    }

    pub fn encode_height16_png(&self) -> Vec<u8> {
        let gray: Vec<u16> = self
            .height_norm
            .iter()
            .map(|&v| (v.clamp(0.0, 1.0) * 65535.0).round() as u16)
            .collect();
        encode_gray16(self.width as u32, self.height as u32, &gray)
    }

    pub fn encode_orm_png(&self) -> Vec<u8> {
        let mut rgb = Vec::with_capacity(self.width * self.height * 3);
        for i in 0..(self.width * self.height) {
            rgb.push((self.ao[i].clamp(0.0, 1.0) * 255.0).round() as u8);
            rgb.push((self.roughness[i].clamp(0.0, 1.0) * 255.0).round() as u8);
            rgb.push((self.metallic[i].clamp(0.0, 1.0) * 255.0).round() as u8);
        }
        encode_rgb(self.width as u32, self.height as u32, &rgb)
    }
}
