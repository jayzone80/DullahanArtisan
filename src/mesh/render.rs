use crate::common::font::Canvas;
use crate::common::math::{linear_to_srgb, srgb_to_linear, Aabb, Vec2, Vec3};
use crate::common::png::encode_rgba;
use crate::mesh::geom::{Mesh, Triangle};
use crate::mesh::palette::Palette;

#[derive(Debug, Clone)]
pub struct RenderOptions {
    pub width: u32,
    pub height: u32,
    pub yaw_deg: f32,
    pub pitch_deg: f32,
    pub fov_deg: f32,
    pub fill: f32,
    pub ortho: bool,
    pub wireframe: bool,
    pub show_colliders: bool,
    pub hud: bool,
    pub bg: [u8; 4],
    pub focus_bounds: Option<Aabb>,
    pub label: Option<String>,
}

impl Default for RenderOptions {
    fn default() -> Self {
        Self {
            width: 640,
            height: 480,
            yaw_deg: 35.0,
            pitch_deg: 24.0,
            fov_deg: 35.0,
            fill: 0.88,
            ortho: false,
            wireframe: false,
            show_colliders: false,
            hud: false,
            bg: [212, 221, 232, 255],
            focus_bounds: None,
            label: None,
        }
    }
}

struct Camera {
    pos: Vec3,
    right: Vec3,
    up: Vec3,
    forward: Vec3, // points from camera toward scene
    ortho: bool,
    tan_half_y: f32,
    tan_half_x: f32,
    ortho_half_x: f32,
    ortho_half_y: f32,
    near: f32,
}

impl Camera {
    #[inline]
    fn project(&self, p: Vec3, w: f32, h: f32) -> Option<(f32, f32, f32)> {
        let rel = p - self.pos;
        let x = rel.dot(self.right);
        let y = rel.dot(self.up);
        let z = rel.dot(self.forward);
        if self.ortho {
            let ndc_x = x / self.ortho_half_x;
            let ndc_y = y / self.ortho_half_y;
            let sx = (ndc_x * 0.5 + 0.5) * w;
            let sy = (0.5 - ndc_y * 0.5) * h;
            Some((sx, sy, z))
        } else {
            if z <= self.near {
                return None;
            }
            let ndc_x = x / (z * self.tan_half_x);
            let ndc_y = y / (z * self.tan_half_y);
            let sx = (ndc_x * 0.5 + 0.5) * w;
            let sy = (0.5 - ndc_y * 0.5) * h;
            Some((sx, sy, z))
        }
    }
}

fn build_camera_basis(yaw_deg: f32, pitch_deg: f32) -> (Vec3, Vec3, Vec3) {
    let yaw = yaw_deg.to_radians();
    let pitch = pitch_deg.clamp(-89.0, 89.0).to_radians();
    // Orbit offset direction FROM target TO camera: yaw=0 -> +Z front
    let (sy, cy) = yaw.sin_cos();
    let (sp, cp) = pitch.sin_cos();
    let to_cam = Vec3::new(cp * sy, sp, cp * cy).normalized();
    let forward = -to_cam;
    let right = forward.cross(Vec3::Y).normalized();
    let up = right.cross(forward).normalized();
    (right, up, forward)
}

fn solve_auto_camera(mesh: &Mesh, opts: &RenderOptions) -> Camera {
    let (right, up, forward) = build_camera_basis(opts.yaw_deg, opts.pitch_deg);
    let aspect = (opts.width as f32 / opts.height.max(1) as f32).max(0.1);
    let fill = opts.fill.clamp(0.15, 0.96);

    // Gather framing points: either the focused part's 8 AABB corners or all mesh vertices
    let mut pts: Vec<Vec3> = Vec::new();
    if let Some(fb) = opts.focus_bounds {
        for x in [fb.min.x, fb.max.x] {
            for y in [fb.min.y, fb.max.y] {
                for z in [fb.min.z, fb.max.z] {
                    pts.push(Vec3::new(x, y, z));
                }
            }
        }
    } else {
        pts.reserve(mesh.tris.len() * 3);
        for t in &mesh.tris {
            pts.push(t.a);
            pts.push(t.b);
            pts.push(t.c);
        }
    }

    let bb = opts.focus_bounds.unwrap_or_else(|| mesh.compute_bounds());
    let center = if bb.is_valid() { bb.center() } else { Vec3::ZERO };
    let radius = if bb.is_valid() {
        (bb.size().len() * 0.5).max(0.05)
    } else {
        1.0
    };

    // Project framing points relative to center into camera basis
    let local: Vec<Vec3> = pts
        .iter()
        .map(|&p| {
            let r = p - center;
            Vec3::new(r.dot(right), r.dot(up), r.dot(forward))
        })
        .collect();

    if opts.ortho {
        let mut min_x = f32::INFINITY;
        let mut max_x = f32::NEG_INFINITY;
        let mut min_y = f32::INFINITY;
        let mut max_y = f32::NEG_INFINITY;
        for v in &local {
            min_x = min_x.min(v.x);
            max_x = max_x.max(v.x);
            min_y = min_y.min(v.y);
            max_y = max_y.max(v.y);
        }
        let cx = 0.5 * (min_x + max_x);
        let cy = 0.5 * (min_y + max_y);
        let span_x = (max_x - min_x).max(0.05);
        let span_y = (max_y - min_y).max(0.05);
        let half_y = (span_y * 0.5 / fill).max(span_x * 0.5 / (fill * aspect));
        let half_x = half_y * aspect;
        let cam_pos = center + right * cx + up * cy - forward * (radius * 4.0);
        return Camera {
            pos: cam_pos,
            right,
            up,
            forward,
            ortho: true,
            tan_half_y: 1.0,
            tan_half_x: aspect,
            ortho_half_x: half_x,
            ortho_half_y: half_y,
            near: -1e6,
        };
    }

    let tan_half_y = (opts.fov_deg.clamp(10.0, 120.0).to_radians() * 0.5).tan();
    let tan_half_x = tan_half_y * aspect;
    let min_z = local.iter().map(|v| v.z).fold(f32::INFINITY, f32::min);
    let mut ox = 0.0f32;
    let mut oy = 0.0f32;
    let mut d = radius * 3.0;

    // Alternate binary search on distance `d` and view-plane offset `(ox, oy)` for pixel-exact framing
    for _ in 0..6 {
        let mut lo = (-min_z).max(0.0) + radius * 0.05 + 0.02;
        let mut hi = lo + radius * 40.0 + 10.0;
        for _ in 0..26 {
            let mid = 0.5 * (lo + hi);
            let mut min_nx = f32::INFINITY;
            let mut max_nx = f32::NEG_INFINITY;
            let mut min_ny = f32::INFINITY;
            let mut max_ny = f32::NEG_INFINITY;
            for v in &local {
                let z = v.z + mid;
                let nx = (v.x - ox) / (z * tan_half_x);
                let ny = (v.y - oy) / (z * tan_half_y);
                min_nx = min_nx.min(nx);
                max_nx = max_nx.max(nx);
                min_ny = min_ny.min(ny);
                max_ny = max_ny.max(ny);
            }
            let span = ((max_nx - min_nx) * 0.5).max((max_ny - min_ny) * 0.5);
            if span > fill {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        d = hi;

        // Recenter view-plane offset
        let mut min_nx = f32::INFINITY;
        let mut max_nx = f32::NEG_INFINITY;
        let mut min_ny = f32::INFINITY;
        let mut max_ny = f32::NEG_INFINITY;
        for v in &local {
            let z = v.z + d;
            let nx = (v.x - ox) / (z * tan_half_x);
            let ny = (v.y - oy) / (z * tan_half_y);
            min_nx = min_nx.min(nx);
            max_nx = max_nx.max(nx);
            min_ny = min_ny.min(ny);
            max_ny = max_ny.max(ny);
        }
        let avg_z = d;
        ox += 0.5 * (min_nx + max_nx) * avg_z * tan_half_x;
        oy += 0.5 * (min_ny + max_ny) * avg_z * tan_half_y;
    }

    let cam_pos = center + right * ox + up * oy - forward * d;
    Camera {
        pos: cam_pos,
        right,
        up,
        forward,
        ortho: false,
        tan_half_y,
        tan_half_x,
        ortho_half_x: 1.0,
        ortho_half_y: 1.0,
        near: 0.01,
    }
}

struct ShadowMap {
    size: usize,
    depth: Vec<f32>,
    origin: Vec3,
    right: Vec3,
    up: Vec3,
    dir: Vec3, // light direction FROM light TOWARD scene
    half_extent: f32,
}

impl ShadowMap {
    fn build(mesh: &Mesh, light_from_dir: Vec3, size: usize) -> Self {
        let dir = (-light_from_dir).normalized();
        let right = dir.cross(Vec3::Y).normalized();
        let up = right.cross(dir).normalized();
        let bb = mesh.compute_bounds();
        let center = if bb.is_valid() { bb.center() } else { Vec3::ZERO };
        let rad = if bb.is_valid() {
            (bb.size().len() * 0.55).max(0.25)
        } else {
            1.0
        };
        let origin = center - dir * (rad * 2.5);
        let mut sm = Self {
            size,
            depth: vec![f32::INFINITY; size * size],
            origin,
            right,
            up,
            dir,
            half_extent: rad * 1.25,
        };
        let s_f = size as f32;
        for t in &mesh.tris {
            if t.is_translucent {
                continue; // Light passes through translucent glass/water into interiors
            }
            let p0 = sm.project(t.a, s_f);
            let p1 = sm.project(t.b, s_f);
            let p2 = sm.project(t.c, s_f);
            rasterize_depth(&mut sm.depth, size, p0, p1, p2);
        }
        sm
    }

    #[inline]
    fn project(&self, p: Vec3, s_f: f32) -> (f32, f32, f32) {
        let rel = p - self.origin;
        let u = (rel.dot(self.right) / self.half_extent * 0.5 + 0.5) * s_f;
        let v = (0.5 - rel.dot(self.up) / self.half_extent * 0.5) * s_f;
        let z = rel.dot(self.dir);
        (u, v, z)
    }

    /// 3x3 PCF soft shadow factor in [0.0 (shadowed), 1.0 (lit)].
    #[inline]
    fn sample_pcf(&self, p: Vec3, normal: Vec3) -> f32 {
        let bias = self.half_extent * 0.008;
        let (u, v, z) = self.project(p + normal * bias, self.size as f32);
        let ix = u.floor() as i32;
        let iy = v.floor() as i32;
        let s = self.size as i32;
        let mut lit = 0.0f32;
        for dy in -1..=1 {
            let py = (iy + dy).clamp(0, s - 1) as usize;
            for dx in -1..=1 {
                let px = (ix + dx).clamp(0, s - 1) as usize;
                if z <= self.depth[py * self.size + px] + bias * 1.4 {
                    lit += 1.0;
                }
            }
        }
        lit / 9.0
    }
}

fn rasterize_depth(depth: &mut [f32], size: usize, p0: (f32, f32, f32), p1: (f32, f32, f32), p2: (f32, f32, f32)) {
    let min_x = p0.0.min(p1.0).min(p2.0).floor().max(0.0) as usize;
    let max_x = p0.0.max(p1.0).max(p2.0).ceil().min((size - 1) as f32) as usize;
    let min_y = p0.1.min(p1.1).min(p2.1).floor().max(0.0) as usize;
    let max_y = p0.1.max(p1.1).max(p2.1).ceil().min((size - 1) as f32) as usize;
    let area = edge_fn(p0.0, p0.1, p1.0, p1.1, p2.0, p2.1);
    if area.abs() < 1e-5 {
        return;
    }
    let inv_area = 1.0 / area;
    for y in min_y..=max_y {
        let py = y as f32 + 0.5;
        for x in min_x..=max_x {
            let px = x as f32 + 0.5;
            let w0 = edge_fn(p1.0, p1.1, p2.0, p2.1, px, py) * inv_area;
            let w1 = edge_fn(p2.0, p2.1, p0.0, p0.1, px, py) * inv_area;
            let w2 = 1.0 - w0 - w1;
            if w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0 {
                let z = w0 * p0.2 + w1 * p1.2 + w2 * p2.2;
                let idx = y * size + x;
                if z < depth[idx] {
                    depth[idx] = z;
                }
            }
        }
    }
}

#[inline]
fn edge_fn(ax: f32, ay: f32, bx: f32, by: f32, px: f32, py: f32) -> f32 {
    (px - ax) * (by - ay) - (py - ay) * (bx - ax)
}

#[inline]
fn shade_pbr(
    world_pos: Vec3,
    normal: Vec3,
    cam_pos: Vec3,
    uv: Vec2,
    palette: &Palette,
    shadow: &ShadowMap,
    key_dir: Vec3,
    fill_dir: Vec3,
) -> ([f32; 3], f32) {
    let bc = palette.sample_basecolor_nearest(uv);
    let orm = palette.sample_orm_nearest(uv);
    let em = palette.sample_emissive_nearest(uv);

    let albedo = Vec3::new(
        srgb_to_linear(bc[0] as f32 / 255.0),
        srgb_to_linear(bc[1] as f32 / 255.0),
        srgb_to_linear(bc[2] as f32 / 255.0),
    );
    let alpha = bc[3] as f32 / 255.0;
    let ao = orm[0];
    let rough = orm[1].clamp(0.06, 1.0);
    let metal = orm[2].clamp(0.0, 1.0);
    let emissive = Vec3::new(
        srgb_to_linear(em[0]),
        srgb_to_linear(em[1]),
        srgb_to_linear(em[2]),
    );

    let n = normal.normalized();
    let v = (cam_pos - world_pos).normalized();

    // Bright sunlit studio hemisphere ambient modulated by AO
    let sky = Vec3::new(0.46, 0.51, 0.60);
    let ground = Vec3::new(0.22, 0.21, 0.20);
    let hemi = ground.lerp(sky, n.y * 0.5 + 0.5) * ao;

    // Warm sunlit key + cool sky fill
    let n_dot_l = n.dot(key_dir).max(0.0);
    let sh = if n_dot_l > 0.0 {
        0.52 + 0.48 * shadow.sample_pcf(world_pos, n)
    } else {
        0.52
    };
    let key_col = Vec3::new(1.06, 1.01, 0.94) * (n_dot_l * sh);
    let fill_col = Vec3::new(0.28, 0.34, 0.44) * n.dot(fill_dir).max(0.0);

    let diffuse_color = albedo * (1.0 - metal * 0.82);
    let mut color = diffuse_color.mul_elem(hemi + key_col + fill_col);

    // Per-pixel Blinn-Phong specular (eliminates diagonal seams across coplanar quads)
    let h_key = (key_dir + v).normalized();
    let shininess = (2.0 / (rough * rough).max(0.004) - 2.0).clamp(6.0, 256.0);
    let spec_n = n.dot(h_key).max(0.0).powf(shininess);
    let spec_strength = (1.0 - rough * 0.78) * (0.34 + 0.66 * metal) * (0.28 + 0.72 * sh);
    let spec_tint = Vec3::ONE.lerp(albedo, metal);
    color = color + spec_tint * (spec_n * spec_strength);

    // Subtle crisp Fresnel rim for silhouette separation
    let rim = (1.0 - n.dot(v).abs()).powf(3.2) * (0.08 + 0.16 * (1.0 - rough));
    color = color + Vec3::new(0.55, 0.62, 0.74) * rim;

    // Metallic studio environment reflection
    if metal > 0.05 {
        let refl_y = (n * (2.0 * n.dot(v)) - v).y * 0.5 + 0.5;
        let env = ground.lerp(Vec3::new(0.76, 0.82, 0.92), refl_y.clamp(0.0, 1.0));
        color = color + albedo.mul_elem(env) * (metal * (1.0 - rough * 0.55) * 0.52 * ao);
    }

    // Self-lit emissive
    color = color + emissive * 1.2;

    // Gentle highlight compression preserving true vibrant palette colors
    let tone = |x: f32| linear_to_srgb(x / (1.0 + x * 0.14));
    ([tone(color.x), tone(color.y), tone(color.z)], alpha)
}

/// Render a single view of `mesh` into a raw RGBA `Canvas`.
pub fn render_view_canvas(mesh: &Mesh, palette: &Palette, opts: &RenderOptions) -> Canvas {
    let w = opts.width.max(32) as usize;
    let h = opts.height.max(32) as usize;
    let ss = 2usize; // 2x2 SSAA
    let sw = w * ss;
    let sh = h * ss;

    let cam = solve_auto_camera(mesh, opts);
    // Key light positioned front-upper-right relative to camera orbit so every turntable face is well-lit
    let key_dir = (cam.right * 0.55 + cam.up * 0.72 - cam.forward * 0.42).normalized();
    let fill_dir = (-cam.right * 0.7 + cam.up * 0.25 - cam.forward * 0.4).normalized();
    let shadow = ShadowMap::build(mesh, key_dir, 1024);

    // Clean studio gradient background
    let mut color_buf = vec![0.0f32; sw * sh * 3];
    let mut depth_buf = vec![f32::INFINITY; sw * sh];
    let bg_top = [
        (opts.bg[0] as f32 / 255.0 * 1.04).min(1.0),
        (opts.bg[1] as f32 / 255.0 * 1.04).min(1.0),
        (opts.bg[2] as f32 / 255.0 * 1.04).min(1.0),
    ];
    let bg_bot = [
        opts.bg[0] as f32 / 255.0 * 0.92,
        opts.bg[1] as f32 / 255.0 * 0.92,
        opts.bg[2] as f32 / 255.0 * 0.92,
    ];
    for y in 0..sh {
        let t = y as f32 / (sh - 1).max(1) as f32;
        let r = bg_top[0] + (bg_bot[0] - bg_top[0]) * t;
        let g = bg_top[1] + (bg_bot[1] - bg_top[1]) * t;
        let b = bg_top[2] + (bg_bot[2] - bg_top[2]) * t;
        for x in 0..sw {
            let p = (y * sw + x) * 3;
            color_buf[p] = r;
            color_buf[p + 1] = g;
            color_buf[p + 2] = b;
        }
    }

    // Optional subtle ground grid on Y = 0 when HUD is enabled
    if opts.hud {
        draw_ground_grid(&mut color_buf, &mut depth_buf, sw, sh, &cam, mesh);
    }

    // Pre-project opaque triangles
    struct ProjTri<'a> {
        tri: &'a Triangle,
        s0: (f32, f32, f32),
        s1: (f32, f32, f32),
        s2: (f32, f32, f32),
        min_x: usize,
        max_x: usize,
        min_y: usize,
        max_y: usize,
        inv_area: f32,
        avg_z: f32,
    }

    let sw_f = sw as f32;
    let sh_f = sh as f32;
    let mut opaque_tris: Vec<ProjTri<'_>> = Vec::new();
    let mut blend_tris: Vec<ProjTri<'_>> = Vec::new();

    for tri in &mesh.tris {
        let (Some(s0), Some(s1), Some(s2)) = (
            cam.project(tri.a, sw_f, sh_f),
            cam.project(tri.b, sw_f, sh_f),
            cam.project(tri.c, sw_f, sh_f),
        ) else {
            continue;
        };
        // edge_fn computes (p-a)x(b-a); in screen-Y-down coords, outward CCW triangles have area > 0.
        let area = edge_fn(s0.0, s0.1, s1.0, s1.1, s2.0, s2.1);
        if area <= 1e-4 {
            continue; // Strict backface culling exposes winding errors immediately!
        }
        let min_x = s0.0.min(s1.0).min(s2.0).floor().max(0.0) as usize;
        let max_x = s0.0.max(s1.0).max(s2.0).ceil().min((sw - 1) as f32) as usize;
        let min_y = s0.1.min(s1.1).min(s2.1).floor().max(0.0) as usize;
        let max_y = s0.1.max(s1.1).max(s2.1).ceil().min((sh - 1) as f32) as usize;
        if min_x > max_x || min_y > max_y {
            continue;
        }
        let pt = ProjTri {
            tri,
            s0,
            s1,
            s2,
            min_x,
            max_x,
            min_y,
            max_y,
            inv_area: 1.0 / area,
            avg_z: (s0.2 + s1.2 + s2.2) * (1.0 / 3.0),
        };
        if tri.is_translucent {
            blend_tris.push(pt);
        } else {
            opaque_tris.push(pt);
        }
    }

    // Multi-threaded scanline band rasterization for opaque triangles
    let threads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(8)
        .max(1);
    let band_rows = (sh + threads - 1) / threads;

    std::thread::scope(|scope| {
        let mut color_rest = color_buf.as_mut_slice();
        let mut depth_rest = depth_buf.as_mut_slice();
        let mut y_start = 0usize;

        while y_start < sh {
            let rows = band_rows.min(sh - y_start);
            let y_end = y_start + rows;
            let (c_band, c_tail) = color_rest.split_at_mut(rows * sw * 3);
            let (d_band, d_tail) = depth_rest.split_at_mut(rows * sw);
            color_rest = c_tail;
            depth_rest = d_tail;

            let opaque_ref = &opaque_tris;
            let cam_ref = &cam;
            let shadow_ref = &shadow;

            scope.spawn(move || {
                for pt in opaque_ref {
                    if pt.max_y < y_start || pt.min_y >= y_end {
                        continue;
                    }
                    let y0 = pt.min_y.max(y_start);
                    let y1 = pt.max_y.min(y_end - 1);
                    let inv_z0 = if cam_ref.ortho { 1.0 } else { 1.0 / pt.s0.2 };
                    let inv_z1 = if cam_ref.ortho { 1.0 } else { 1.0 / pt.s1.2 };
                    let inv_z2 = if cam_ref.ortho { 1.0 } else { 1.0 / pt.s2.2 };

                    for y in y0..=y1 {
                        let py = y as f32 + 0.5;
                        let local_y = y - y_start;
                        for x in pt.min_x..=pt.max_x {
                            let px = x as f32 + 0.5;
                            let w0 = edge_fn(pt.s1.0, pt.s1.1, pt.s2.0, pt.s2.1, px, py) * pt.inv_area;
                            if w0 < 0.0 {
                                continue;
                            }
                            let w1 = edge_fn(pt.s2.0, pt.s2.1, pt.s0.0, pt.s0.1, px, py) * pt.inv_area;
                            if w1 < 0.0 {
                                continue;
                            }
                            let w2 = 1.0 - w0 - w1;
                            if w2 < 0.0 {
                                continue;
                            }
                            let (z, b0, b1, b2) = if cam_ref.ortho {
                                (w0 * pt.s0.2 + w1 * pt.s1.2 + w2 * pt.s2.2, w0, w1, w2)
                            } else {
                                let iz = w0 * inv_z0 + w1 * inv_z1 + w2 * inv_z2;
                                let z = 1.0 / iz;
                                (z, w0 * inv_z0 * z, w1 * inv_z1 * z, w2 * inv_z2 * z)
                            };
                            let idx = local_y * sw + x;
                            if z < d_band[idx] {
                                d_band[idx] = z;
                                let wp = pt.tri.a * b0 + pt.tri.b * b1 + pt.tri.c * b2;
                                let wn = pt.tri.na * b0 + pt.tri.nb * b1 + pt.tri.nc * b2;
                                let (rgb, _) = shade_pbr(
                                    wp, wn, cam_ref.pos, pt.tri.uv, palette, shadow_ref, key_dir, fill_dir,
                                );
                                let p = idx * 3;
                                c_band[p] = rgb[0];
                                c_band[p + 1] = rgb[1];
                                c_band[p + 2] = rgb[2];
                            }
                        }
                    }
                }
            });

            y_start = y_end;
        }
    });

    // Translucent pass: sort back-to-front by view depth, depth-test without depth-write
    blend_tris.sort_by(|a, b| b.avg_z.total_cmp(&a.avg_z));
    for pt in &blend_tris {
        let inv_z0 = if cam.ortho { 1.0 } else { 1.0 / pt.s0.2 };
        let inv_z1 = if cam.ortho { 1.0 } else { 1.0 / pt.s1.2 };
        let inv_z2 = if cam.ortho { 1.0 } else { 1.0 / pt.s2.2 };
        for y in pt.min_y..=pt.max_y {
            let py = y as f32 + 0.5;
            for x in pt.min_x..=pt.max_x {
                let px = x as f32 + 0.5;
                let w0 = edge_fn(pt.s1.0, pt.s1.1, pt.s2.0, pt.s2.1, px, py) * pt.inv_area;
                let w1 = edge_fn(pt.s2.0, pt.s2.1, pt.s0.0, pt.s0.1, px, py) * pt.inv_area;
                let w2 = 1.0 - w0 - w1;
                if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                    continue;
                }
                let (z, b0, b1, b2) = if cam.ortho {
                    (w0 * pt.s0.2 + w1 * pt.s1.2 + w2 * pt.s2.2, w0, w1, w2)
                } else {
                    let iz = w0 * inv_z0 + w1 * inv_z1 + w2 * inv_z2;
                    let z = 1.0 / iz;
                    (z, w0 * inv_z0 * z, w1 * inv_z1 * z, w2 * inv_z2 * z)
                };
                let idx = y * sw + x;
                if z < depth_buf[idx] {
                    let wp = pt.tri.a * b0 + pt.tri.b * b1 + pt.tri.c * b2;
                    let wn = pt.tri.na * b0 + pt.tri.nb * b1 + pt.tri.nc * b2;
                    let (rgb, alpha) = shade_pbr(wp, wn, cam.pos, pt.tri.uv, palette, &shadow, key_dir, fill_dir);
                    let p = idx * 3;
                    color_buf[p] = rgb[0] * alpha + color_buf[p] * (1.0 - alpha);
                    color_buf[p + 1] = rgb[1] * alpha + color_buf[p + 1] * (1.0 - alpha);
                    color_buf[p + 2] = rgb[2] * alpha + color_buf[p + 2] * (1.0 - alpha);
                }
            }
        }
    }

    // 2x2 Box downsample to output RGBA Canvas
    let mut canvas = Canvas::new(w, h, [0, 0, 0, 255]);
    for y in 0..h {
        for x in 0..w {
            let mut r = 0.0f32;
            let mut g = 0.0f32;
            let mut b = 0.0f32;
            for sy in 0..ss {
                for sx in 0..ss {
                    let p = ((y * ss + sy) * sw + (x * ss + sx)) * 3;
                    r += color_buf[p];
                    g += color_buf[p + 1];
                    b += color_buf[p + 2];
                }
            }
            let inv = 255.0 / (ss * ss) as f32;
            let o = (y * w + x) * 4;
            canvas.rgba[o] = (r * inv).clamp(0.0, 255.0).round() as u8;
            canvas.rgba[o + 1] = (g * inv).clamp(0.0, 255.0).round() as u8;
            canvas.rgba[o + 2] = (b * inv).clamp(0.0, 255.0).round() as u8;
            canvas.rgba[o + 3] = 255;
        }
    }

    // Optional wireframe edge overlay
    if opts.wireframe {
        let wf = [220, 240, 255, 110];
        let wf_w = w as f32;
        let wf_h = h as f32;
        for tri in &mesh.tris {
            if let (Some(a), Some(b), Some(c)) = (
                cam.project(tri.a, wf_w, wf_h),
                cam.project(tri.b, wf_w, wf_h),
                cam.project(tri.c, wf_w, wf_h),
            ) {
                if edge_fn(a.0, a.1, b.0, b.1, c.0, c.1) > 0.0 {
                    canvas.draw_line(a.0 as i32, a.1 as i32, b.0 as i32, b.1 as i32, wf);
                    canvas.draw_line(b.0 as i32, b.1 as i32, c.0 as i32, c.1 as i32, wf);
                    canvas.draw_line(c.0 as i32, c.1 as i32, a.0 as i32, a.1 as i32, wf);
                }
            }
        }
    }

    // Optional collider wireframe overlay (bright green)
    if (opts.show_colliders || opts.wireframe) && !mesh.colliders.is_empty() {
        let col_wf = [72, 245, 118, 195];
        let wf_w = w as f32;
        let wf_h = h as f32;
        for col in &mesh.colliders {
            for tri in &col.tris {
                if let (Some(a), Some(b), Some(c)) = (
                    cam.project(tri.a, wf_w, wf_h),
                    cam.project(tri.b, wf_w, wf_h),
                    cam.project(tri.c, wf_w, wf_h),
                ) {
                    canvas.draw_line(a.0 as i32, a.1 as i32, b.0 as i32, b.1 as i32, col_wf);
                    canvas.draw_line(b.0 as i32, b.1 as i32, c.0 as i32, c.1 as i32, col_wf);
                    canvas.draw_line(c.0 as i32, c.1 as i32, a.0 as i32, a.1 as i32, col_wf);
                }
            }
        }
    }

    // Optional HUD: RGB orientation triad + telemetry label strip
    if opts.hud {
        draw_axis_triad(&mut canvas, &cam);
        if let Some(ref lbl) = opts.label {
            canvas.fill_rect(6, 6, (lbl.len() as i32) * 6 + 10, 15, [12, 14, 18, 180]);
            canvas.draw_text(11, 10, lbl, [235, 240, 248, 255], 1);
        }
    }

    canvas
}

fn draw_ground_grid(
    color_buf: &mut [f32],
    depth_buf: &mut [f32],
    sw: usize,
    sh: usize,
    cam: &Camera,
    mesh: &Mesh,
) {
    let bb = mesh.compute_bounds();
    let max_xz = if bb.is_valid() {
        bb.min.x.abs().max(bb.max.x.abs()).max(bb.min.z.abs()).max(bb.max.z.abs())
    } else {
        1.0
    };
    let extent = (max_xz.ceil() as i32 + 1).clamp(2, 10);
    let sw_f = sw as f32;
    let sh_f = sh as f32;

    let mut draw_seg = |a: Vec3, b: Vec3, col: [f32; 3]| {
        let steps = 24;
        for i in 0..steps {
            let t0 = i as f32 / steps as f32;
            let t1 = (i + 1) as f32 / steps as f32;
            let (Some(p0), Some(p1)) = (
                cam.project(a.lerp(b, t0), sw_f, sh_f),
                cam.project(a.lerp(b, t1), sw_f, sh_f),
            ) else {
                continue;
            };
            let dx = p1.0 - p0.0;
            let dy = p1.1 - p0.1;
            let n = dx.abs().max(dy.abs()).ceil().max(1.0) as i32;
            for k in 0..=n {
                let u = k as f32 / n as f32;
                let x = (p0.0 + dx * u) as i32;
                let y = (p0.1 + dy * u) as i32;
                let z = p0.2 + (p1.2 - p0.2) * u;
                if x >= 0 && y >= 0 && x < sw as i32 && y < sh as i32 {
                    let idx = y as usize * sw + x as usize;
                    if z < depth_buf[idx] {
                        let p = idx * 3;
                        color_buf[p] = color_buf[p] * 0.6 + col[0] * 0.4;
                        color_buf[p + 1] = color_buf[p + 1] * 0.6 + col[1] * 0.4;
                        color_buf[p + 2] = color_buf[p + 2] * 0.6 + col[2] * 0.4;
                    }
                }
            }
        }
    };

    let e = extent as f32;
    for i in -extent..=extent {
        let fi = i as f32;
        let col_x = if i == 0 { [0.78, 0.30, 0.30] } else { [0.58, 0.63, 0.70] };
        let col_z = if i == 0 { [0.28, 0.45, 0.80] } else { [0.58, 0.63, 0.70] };
        draw_seg(Vec3::new(-e, 0.0, fi), Vec3::new(e, 0.0, fi), col_x);
        draw_seg(Vec3::new(fi, 0.0, -e), Vec3::new(fi, 0.0, e), col_z);
    }
}

fn draw_axis_triad(canvas: &mut Canvas, cam: &Camera) {
    let cx = canvas.width as i32 - 34;
    let cy = canvas.height as i32 - 30;
    let len = 20.0f32;
    let axes = [
        (Vec3::X, [235, 80, 80, 255], "X"),
        (Vec3::Y, [90, 220, 110, 255], "Y"),
        (Vec3::Z, [90, 155, 245, 255], "Z"),
    ];
    for (dir, col, lbl) in axes {
        let sx = (dir.dot(cam.right) * len).round() as i32;
        let sy = (-dir.dot(cam.up) * len).round() as i32;
        canvas.draw_line(cx, cy, cx + sx, cy + sy, col);
        canvas.draw_text(cx + sx + if sx >= 0 { 2 } else { -7 }, cy + sy - 3, lbl, col, 1);
    }
}

pub fn render_to_png(mesh: &Mesh, palette: &Palette, opts: &RenderOptions) -> Vec<u8> {
    let mut o = opts.clone();
    if o.hud && o.label.is_none() {
        let bb = mesh.compute_bounds();
        let sz = if bb.is_valid() { bb.size() } else { Vec3::ZERO };
        o.label = Some(format!(
            "YAW:{:.0} PITCH:{:.0} | {:.2}x{:.2}x{:.2} | {} TRIS",
            o.yaw_deg, o.pitch_deg, sz.x, sz.y, sz.z, mesh.tris.len()
        ));
    }
    let c = render_view_canvas(mesh, palette, &o);
    encode_rgba(c.width as u32, c.height as u32, &c.rgba)
}

/// Render a 2x2 turntable contact sheet (`yaw`, `yaw+90`, `yaw+180`, `yaw+270`) with clean HUD footer.
pub fn render_turntable_png(mesh: &Mesh, palette: &Palette, opts: &RenderOptions) -> Vec<u8> {
    let cell_w = opts.width.max(160);
    let cell_h = opts.height.max(120);
    let total_w = cell_w as usize * 2;
    let footer_h = if opts.hud { 22usize } else { 0 };
    let total_h = cell_h as usize * 2 + footer_h;

    let angles = [
        (opts.yaw_deg, "VIEW 0 (+0)"),
        (opts.yaw_deg + 90.0, "VIEW 1 (+90)"),
        (opts.yaw_deg + 180.0, "VIEW 2 (+180)"),
        (opts.yaw_deg + 270.0, "VIEW 3 (+270)"),
    ];
    let mut out = Canvas::new(total_w, total_h, [18, 20, 24, 255]);

    for (idx, (yaw, name)) in angles.iter().enumerate() {
        let mut sub = opts.clone();
        sub.width = cell_w;
        sub.height = cell_h;
        sub.yaw_deg = *yaw;
        sub.label = Some(format!("{name} Y:{:.0} P:{:.0}", yaw % 360.0, sub.pitch_deg));
        let cell = render_view_canvas(mesh, palette, &sub);
        let ox = (idx % 2) * cell_w as usize;
        let oy = (idx / 2) * cell_h as usize;
        for y in 0..cell.height {
            let src_start = y * cell.width * 4;
            let dst_start = ((oy + y) * total_w + ox) * 4;
            out.rgba[dst_start..dst_start + cell.width * 4]
                .copy_from_slice(&cell.rgba[src_start..src_start + cell.width * 4]);
        }
    }

    // Crisp divider lines between quadrants
    out.fill_rect(cell_w as i32 - 1, 0, 2, (cell_h * 2) as i32, [14, 16, 20, 255]);
    out.fill_rect(0, cell_h as i32 - 1, total_w as i32, 2, [14, 16, 20, 255]);

    if opts.hud {
        let bb = mesh.compute_bounds();
        let sz = if bb.is_valid() { bb.size() } else { Vec3::ZERO };
        let has_blend = mesh.tris.iter().any(|t| t.is_translucent);
        let info = format!(
            "BBOX: {:.2}W x {:.2}H x {:.2}D  Y:[{:.2}..{:.2}]  TRIS: {}  DRAWS: {}",
            sz.x,
            sz.y,
            sz.z,
            if bb.is_valid() { bb.min.y } else { 0.0 },
            if bb.is_valid() { bb.max.y } else { 0.0 },
            mesh.tris.len(),
            if has_blend { 2 } else { 1 }
        );
        out.fill_rect(0, (cell_h * 2) as i32, total_w as i32, footer_h as i32, [16, 18, 22, 255]);
        out.draw_text(10, (cell_h * 2) as i32 + 7, &info, [220, 228, 240, 255], 1);
    }

    encode_rgba(out.width as u32, out.height as u32, &out.rgba)
}
