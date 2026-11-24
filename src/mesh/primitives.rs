use std::collections::HashMap;
use std::f32::consts::TAU;

use crate::common::math::{Rng, Vec2, Vec3};
use crate::mesh::geom::Mesh;
use crate::mesh::palette::ResolvedMat;

#[derive(Debug, Clone, Default)]
pub struct FaceMats {
    pub map: HashMap<String, ResolvedMat>,
}

impl FaceMats {
    pub fn pick(&self, default: ResolvedMat, keys: &[&str]) -> ResolvedMat {
        for &k in keys {
            if let Some(&m) = self.map.get(k) {
                return m;
            }
        }
        default
    }
}

/// Box centered at origin with optional top taper, top shear offset, and 1-segment chamfered bevel.
pub fn build_box(
    size: Vec3,
    taper: Vec2,
    top_offset: Vec2,
    bevel: f32,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    let hx = size.x.abs() * 0.5;
    let hy = size.y.abs() * 0.5;
    let hz = size.z.abs() * 0.5;
    if hx < 1e-6 || hy < 1e-6 || hz < 1e-6 {
        return m;
    }

    let warp = |p: Vec3| -> Vec3 {
        let u = (p.y + hy) / (2.0 * hy);
        let sx = 1.0 + (taper.x - 1.0) * u;
        let sz = 1.0 + (taper.y - 1.0) * u;
        Vec3::new(p.x * sx + top_offset.x * u, p.y, p.z * sz + top_offset.y * u)
    };

    let m_top = fc.pick(mat, &["t", "top"]);
    let m_bot = fc.pick(mat, &["b", "bot", "bottom"]);
    let m_front = fc.pick(mat, &["f", "front", "s", "sides"]);
    let m_back = fc.pick(mat, &["k", "back", "s", "sides"]);
    let m_right = fc.pick(mat, &["r", "right", "s", "sides"]);
    let m_left = fc.pick(mat, &["l", "left", "s", "sides"]);

    let b = bevel.max(0.0).min(hx.min(hy).min(hz) * 0.45);
    if b <= 1e-5 {
        let p = |sx: f32, sy: f32, sz: f32| warp(Vec3::new(sx * hx, sy * hy, sz * hz));
        m.add_quad(p(-1.0, -1.0, 1.0), p(1.0, -1.0, 1.0), p(1.0, 1.0, 1.0), p(-1.0, 1.0, 1.0), m_front);
        m.add_quad(p(1.0, -1.0, -1.0), p(-1.0, -1.0, -1.0), p(-1.0, 1.0, -1.0), p(1.0, 1.0, -1.0), m_back);
        m.add_quad(p(1.0, -1.0, 1.0), p(1.0, -1.0, -1.0), p(1.0, 1.0, -1.0), p(1.0, 1.0, 1.0), m_right);
        m.add_quad(p(-1.0, -1.0, -1.0), p(-1.0, -1.0, 1.0), p(-1.0, 1.0, 1.0), p(-1.0, 1.0, -1.0), m_left);
        m.add_quad(p(-1.0, 1.0, 1.0), p(1.0, 1.0, 1.0), p(1.0, 1.0, -1.0), p(-1.0, 1.0, -1.0), m_top);
        m.add_quad(p(-1.0, -1.0, -1.0), p(1.0, -1.0, -1.0), p(1.0, -1.0, 1.0), p(-1.0, -1.0, 1.0), m_bot);
        return m;
    }

    // Watertight 26-face chamfered box: 6 inset faces + 12 edge bevel quads + 8 corner tris
    let ix = hx - b;
    let iy = hy - b;
    let iz = hz - b;
    let vx = |sx: f32, sy: f32, sz: f32| warp(Vec3::new(sx * hx, sy * iy, sz * iz));
    let vy = |sx: f32, sy: f32, sz: f32| warp(Vec3::new(sx * ix, sy * hy, sz * iz));
    let vz = |sx: f32, sy: f32, sz: f32| warp(Vec3::new(sx * ix, sy * iy, sz * hz));

    // 6 main faces
    m.add_quad(vz(-1.0, -1.0, 1.0), vz(1.0, -1.0, 1.0), vz(1.0, 1.0, 1.0), vz(-1.0, 1.0, 1.0), m_front);
    m.add_quad(vz(1.0, -1.0, -1.0), vz(-1.0, -1.0, -1.0), vz(-1.0, 1.0, -1.0), vz(1.0, 1.0, -1.0), m_back);
    m.add_quad(vx(1.0, -1.0, 1.0), vx(1.0, -1.0, -1.0), vx(1.0, 1.0, -1.0), vx(1.0, 1.0, 1.0), m_right);
    m.add_quad(vx(-1.0, -1.0, -1.0), vx(-1.0, -1.0, 1.0), vx(-1.0, 1.0, 1.0), vx(-1.0, 1.0, -1.0), m_left);
    m.add_quad(vy(-1.0, 1.0, 1.0), vy(1.0, 1.0, 1.0), vy(1.0, 1.0, -1.0), vy(-1.0, 1.0, -1.0), m_top);
    m.add_quad(vy(-1.0, -1.0, -1.0), vy(1.0, -1.0, -1.0), vy(1.0, -1.0, 1.0), vy(-1.0, -1.0, 1.0), m_bot);

    // 4 vertical edges (along Y, bridging X and Z faces)
    m.add_quad(vz(1.0, -1.0, 1.0), vx(1.0, -1.0, 1.0), vx(1.0, 1.0, 1.0), vz(1.0, 1.0, 1.0), m_front);
    m.add_quad(vx(1.0, -1.0, -1.0), vz(1.0, -1.0, -1.0), vz(1.0, 1.0, -1.0), vx(1.0, 1.0, -1.0), m_back);
    m.add_quad(vz(-1.0, -1.0, -1.0), vx(-1.0, -1.0, -1.0), vx(-1.0, 1.0, -1.0), vz(-1.0, 1.0, -1.0), m_back);
    m.add_quad(vx(-1.0, -1.0, 1.0), vz(-1.0, -1.0, 1.0), vz(-1.0, 1.0, 1.0), vx(-1.0, 1.0, 1.0), m_front);

    // 4 top horizontal edges
    m.add_quad(vz(-1.0, 1.0, 1.0), vz(1.0, 1.0, 1.0), vy(1.0, 1.0, 1.0), vy(-1.0, 1.0, 1.0), m_top);
    m.add_quad(vz(1.0, 1.0, -1.0), vz(-1.0, 1.0, -1.0), vy(-1.0, 1.0, -1.0), vy(1.0, 1.0, -1.0), m_top);
    m.add_quad(vx(1.0, 1.0, 1.0), vx(1.0, 1.0, -1.0), vy(1.0, 1.0, -1.0), vy(1.0, 1.0, 1.0), m_top);
    m.add_quad(vx(-1.0, 1.0, -1.0), vx(-1.0, 1.0, 1.0), vy(-1.0, 1.0, 1.0), vy(-1.0, 1.0, -1.0), m_top);

    // 4 bottom horizontal edges
    m.add_quad(vy(-1.0, -1.0, 1.0), vy(1.0, -1.0, 1.0), vz(1.0, -1.0, 1.0), vz(-1.0, -1.0, 1.0), m_bot);
    m.add_quad(vy(1.0, -1.0, -1.0), vy(-1.0, -1.0, -1.0), vz(-1.0, -1.0, -1.0), vz(1.0, -1.0, -1.0), m_bot);
    m.add_quad(vy(1.0, -1.0, 1.0), vy(1.0, -1.0, -1.0), vx(1.0, -1.0, -1.0), vx(1.0, -1.0, 1.0), m_bot);
    m.add_quad(vy(-1.0, -1.0, -1.0), vy(-1.0, -1.0, 1.0), vx(-1.0, -1.0, 1.0), vx(-1.0, -1.0, -1.0), m_bot);

    // 8 corner bevel triangles (oriented strictly outward using corner normal dot test)
    for sx in [-1.0f32, 1.0] {
        for sy in [-1.0f32, 1.0] {
            for sz in [-1.0f32, 1.0] {
                let pa = vx(sx, sy, sz);
                let pb = vy(sx, sy, sz);
                let pc = vz(sx, sy, sz);
                let out_dir = Vec3::new(sx, sy, sz);
                let c_mat = if sy > 0.0 { m_top } else { m_bot };
                if (pb - pa).cross(pc - pa).dot(out_dir) >= 0.0 {
                    m.add_tri(pa, pb, pc, c_mat);
                } else {
                    m.add_tri(pa, pc, pb, c_mat);
                }
            }
        }
    }

    m
}

/// Cylinder / tapered frustum / cone (rt=0) with optional rim bevels and open ends.
pub fn build_cylinder(
    rb: f32,
    rt: f32,
    height: f32,
    segments: u32,
    capped: bool,
    bevel: f32,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let seg = segments.max(3);
    let hh = height.abs() * 0.5;
    let rb = rb.max(0.0);
    let rt = rt.max(0.0);
    if hh < 1e-6 || (rb < 1e-6 && rt < 1e-6) {
        return Mesh::new();
    }
    let mut profile = Vec::new();
    let max_b = (hh * 0.45).min(if rb > 1e-5 { rb * 0.45 } else { hh }).min(if rt > 1e-5 { rt * 0.45 } else { hh });
    let b = bevel.max(0.0).min(max_b);
    if b > 1e-5 && rb > 1e-5 && rt > 1e-5 {
        profile.push((rb - b, -hh));
        profile.push((rb, -hh + b));
        profile.push((rt, hh - b));
        profile.push((rt - b, hh));
    } else {
        profile.push((rb, -hh));
        profile.push((rt, hh));
    }
    build_lathe(&profile, seg, capped, mat, fc)
}

/// Tapered or uniform rounded capsule centered at origin, exact end-to-end height `height`.
pub fn build_capsule(rb: f32, rt: f32, height: f32, segments: u32, rings: u32, mat: ResolvedMat) -> Mesh {
    let seg = segments.max(4);
    let rings = rings.max(2);
    let rb = rb.max(1e-4);
    let rt = rt.max(1e-4);
    let h = height.max(rb + rt + 1e-4);
    let hh = h * 0.5;
    let yb_center = -hh + rb;
    let yt_center = hh - rt;

    let mut profile = Vec::new();
    // Bottom hemisphere: angle -PI/2 -> 0
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let a = -std::f32::consts::FRAC_PI_2 * (1.0 - t);
        profile.push((rb * a.cos(), yb_center + rb * a.sin()));
    }
    // Top hemisphere: angle 0 -> +PI/2
    for i in 0..=rings {
        let t = i as f32 / rings as f32;
        let a = std::f32::consts::FRAC_PI_2 * t;
        profile.push((rt * a.cos(), yt_center + rt * a.sin()));
    }
    build_lathe(&profile, seg, false, mat, &FaceMats::default())
}

/// Right-triangular wedge centered at origin (vertical back at -Z, slope descending toward +Z).
pub fn build_wedge(size: Vec3, mat: ResolvedMat, fc: &FaceMats) -> Mesh {
    let mut m = Mesh::new();
    let hx = size.x.abs() * 0.5;
    let hy = size.y.abs() * 0.5;
    let hz = size.z.abs() * 0.5;
    if hx < 1e-6 || hy < 1e-6 || hz < 1e-6 {
        return m;
    }

    let b_bl = Vec3::new(-hx, -hy, -hz);
    let b_br = Vec3::new(hx, -hy, -hz);
    let b_tr = Vec3::new(hx, hy, -hz);
    let b_tl = Vec3::new(-hx, hy, -hz);
    let f_bl = Vec3::new(-hx, -hy, hz);
    let f_br = Vec3::new(hx, -hy, hz);

    let m_slope = fc.pick(mat, &["sl", "slope", "t", "top"]);
    let m_back = fc.pick(mat, &["k", "back", "s", "sides"]);
    let m_bot = fc.pick(mat, &["b", "bot", "bottom"]);
    let m_left = fc.pick(mat, &["l", "left", "s", "sides"]);
    let m_right = fc.pick(mat, &["r", "right", "s", "sides"]);

    m.add_quad(b_br, b_bl, b_tl, b_tr, m_back); // -Z back
    m.add_quad(b_bl, b_br, f_br, f_bl, m_bot);  // -Y bottom
    m.add_quad(f_bl, f_br, b_tr, b_tl, m_slope); // +Y/+Z slope
    m.add_tri(b_bl, f_bl, b_tl, m_left);         // -X left
    m.add_tri(f_br, b_br, b_tr, m_right);        // +X right
    m
}

/// Rectangular (sides==4) or N-sided pyramid centered at origin with optional apex offset.
pub fn build_pyramid(
    size: Vec3,
    sides: u32,
    top_offset: Vec2,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    let hx = size.x.abs() * 0.5;
    let hy = size.y.abs() * 0.5;
    let hz = size.z.abs() * 0.5;
    if hx < 1e-6 || hy < 1e-6 || hz < 1e-6 {
        return m;
    }
    let apex = Vec3::new(top_offset.x, hy, top_offset.y);
    let m_bot = fc.pick(mat, &["b", "bot", "bottom"]);
    let m_side = fc.pick(mat, &["s", "sides", "t", "top"]);

    if sides == 4 {
        let p0 = Vec3::new(-hx, -hy, hz);
        let p1 = Vec3::new(hx, -hy, hz);
        let p2 = Vec3::new(hx, -hy, -hz);
        let p3 = Vec3::new(-hx, -hy, -hz);
        m.add_tri(p0, p1, apex, fc.pick(m_side, &["f", "front"]));
        m.add_tri(p1, p2, apex, fc.pick(m_side, &["r", "right"]));
        m.add_tri(p2, p3, apex, fc.pick(m_side, &["k", "back"]));
        m.add_tri(p3, p0, apex, fc.pick(m_side, &["l", "left"]));
        m.add_quad(p3, p2, p1, p0, m_bot);
    } else {
        let n = sides.max(3) as usize;
        let mut ring = Vec::with_capacity(n);
        for i in 0..n {
            let a = TAU * (i as f32) / (n as f32);
            ring.push(Vec3::new(hx * a.cos(), -hy, -hz * a.sin()));
        }
        let center_b = Vec3::new(0.0, -hy, 0.0);
        for i in 0..n {
            let j = (i + 1) % n;
            m.add_tri(ring[i], ring[j], apex, m_side);
            m.add_tri(center_b, ring[j], ring[i], m_bot);
        }
    }
    m
}

/// Subdivided icosphere with watertight indexed radial jitter and optional multi-color palette ramp.
pub fn build_icosphere(
    radius: f32,
    subdivisions: u32,
    jitter: f32,
    seed: u64,
    mat: ResolvedMat,
    variants: &[ResolvedMat],
) -> Mesh {
    let t = (1.0 + 5.0f32.sqrt()) * 0.5;
    let mut verts: Vec<Vec3> = vec![
        Vec3::new(-1.0, t, 0.0), Vec3::new(1.0, t, 0.0), Vec3::new(-1.0, -t, 0.0), Vec3::new(1.0, -t, 0.0),
        Vec3::new(0.0, -1.0, t), Vec3::new(0.0, 1.0, t), Vec3::new(0.0, -1.0, -t), Vec3::new(0.0, 1.0, -t),
        Vec3::new(t, 0.0, -1.0), Vec3::new(t, 0.0, 1.0), Vec3::new(-t, 0.0, -1.0), Vec3::new(-t, 0.0, 1.0),
    ]
    .into_iter()
    .map(|v| v.normalized())
    .collect();

    let mut faces: Vec<[usize; 3]> = vec![
        [0, 11, 5], [0, 5, 1], [0, 1, 7], [0, 7, 10], [0, 10, 11],
        [1, 5, 9], [5, 11, 4], [11, 10, 2], [10, 7, 6], [7, 1, 8],
        [3, 9, 4], [3, 4, 2], [3, 2, 6], [3, 6, 8], [3, 8, 9],
        [4, 9, 5], [2, 4, 11], [6, 2, 10], [8, 6, 7], [9, 8, 1],
    ];

    let sub = subdivisions.min(4);
    for _ in 0..sub {
        let mut cache: HashMap<(usize, usize), usize> = HashMap::new();
        let mut next = Vec::with_capacity(faces.len() * 4);
        let mut mid = |a: usize, b: usize, verts: &mut Vec<Vec3>| -> usize {
            let key = if a < b { (a, b) } else { (b, a) };
            if let Some(&idx) = cache.get(&key) {
                return idx;
            }
            let v = (verts[a] + verts[b]).normalized();
            let idx = verts.len();
            verts.push(v);
            cache.insert(key, idx);
            idx
        };
        for [a, b, c] in faces {
            let ab = mid(a, b, &mut verts);
            let bc = mid(b, c, &mut verts);
            let ca = mid(c, a, &mut verts);
            next.push([a, ab, ca]);
            next.push([b, bc, ab]);
            next.push([c, ca, bc]);
            next.push([ab, bc, ca]);
        }
        faces = next;
    }

    // Apply radial jitter to unique indexed vertices BEFORE triangle emission => 100% watertight!
    let mut rng = Rng::new(seed);
    for v in &mut verts {
        let scale = if jitter.abs() > 1e-6 {
            radius * (1.0 + rng.signed_f32() * jitter)
        } else {
            radius
        };
        *v = *v * scale;
    }

    let mut m = Mesh::new();
    for [a, b, c] in faces {
        let chosen = if variants.is_empty() {
            mat
        } else {
            variants[rng.usize(variants.len())]
        };
        m.add_tri(verts[a], verts[b], verts[c], chosen);
    }
    m
}

/// Torus centered at origin in XZ plane (axis = Y).
pub fn build_torus(major_r: f32, tube_r: f32, segments: u32, tube_segments: u32, mat: ResolvedMat) -> Mesh {
    let mut m = Mesh::new();
    let seg = segments.max(3) as usize;
    let tseg = tube_segments.max(3) as usize;
    let mut grid = vec![Vec3::ZERO; seg * tseg];

    for i in 0..seg {
        let u = TAU * i as f32 / seg as f32;
        let (su, cu) = u.sin_cos();
        let rad_dir = Vec3::new(cu, 0.0, -su);
        for j in 0..tseg {
            let v = TAU * j as f32 / tseg as f32;
            let (sv, cv) = v.sin_cos();
            let r = major_r + tube_r * cv;
            grid[i * tseg + j] = rad_dir * r + Vec3::new(0.0, tube_r * sv, 0.0);
        }
    }

    for i in 0..seg {
        let inx = (i + 1) % seg;
        for j in 0..tseg {
            let jnx = (j + 1) % tseg;
            let a = grid[i * tseg + j];
            let b = grid[inx * tseg + j];
            let c = grid[inx * tseg + jnx];
            let d = grid[i * tseg + jnx];
            m.add_quad(a, b, c, d, mat);
        }
    }
    m
}

/// Extrude any simple 2D polygon along Z (from -depth/2 to +depth/2) with auto-CCW,
/// ear-clipping triangulation, and optional 1-segment chamfered bevel.
pub fn build_prism(
    points_in: &[Vec2],
    depth: f32,
    bevel: f32,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    if points_in.len() < 3 || depth.abs() < 1e-6 {
        return m;
    }
    let mut pts = points_in.to_vec();
    if signed_area_2d(&pts) < 0.0 {
        pts.reverse();
    }
    let hd = depth.abs() * 0.5;
    let m_front = fc.pick(mat, &["f", "front", "t", "top"]);
    let m_back = fc.pick(mat, &["k", "back", "b", "bot", "bottom"]);
    let m_side = fc.pick(mat, &["s", "sides"]);

    let b = bevel.max(0.0).min(hd * 0.45);
    let n = pts.len();

    if b <= 1e-5 {
        let tris = ear_clip_2d(&pts);
        for [i0, i1, i2] in &tris {
            // +Z front face (CCW in XY viewed from +Z)
            m.add_tri(
                Vec3::new(pts[*i0].x, pts[*i0].y, hd),
                Vec3::new(pts[*i1].x, pts[*i1].y, hd),
                Vec3::new(pts[*i2].x, pts[*i2].y, hd),
                m_front,
            );
            // -Z back face (reversed winding)
            m.add_tri(
                Vec3::new(pts[*i0].x, pts[*i0].y, -hd),
                Vec3::new(pts[*i2].x, pts[*i2].y, -hd),
                Vec3::new(pts[*i1].x, pts[*i1].y, -hd),
                m_back,
            );
        }
        for i in 0..n {
            let j = (i + 1) % n;
            let a = Vec3::new(pts[i].x, pts[i].y, -hd);
            let b0 = Vec3::new(pts[j].x, pts[j].y, -hd);
            let c = Vec3::new(pts[j].x, pts[j].y, hd);
            let d = Vec3::new(pts[i].x, pts[i].y, hd);
            m.add_quad(a, b0, c, d, m_side);
        }
        return m;
    }

    // Beveled prism: inset polygon for caps at ±hd, full polygon at ±(hd - b)
    let inset = inset_polygon_2d(&pts, b);
    let z_inner = hd - b;
    let tris = ear_clip_2d(&inset);
    for [i0, i1, i2] in &tris {
        m.add_tri(
            Vec3::new(inset[*i0].x, inset[*i0].y, hd),
            Vec3::new(inset[*i1].x, inset[*i1].y, hd),
            Vec3::new(inset[*i2].x, inset[*i2].y, hd),
            m_front,
        );
        m.add_tri(
            Vec3::new(inset[*i0].x, inset[*i0].y, -hd),
            Vec3::new(inset[*i2].x, inset[*i2].y, -hd),
            Vec3::new(inset[*i1].x, inset[*i1].y, -hd),
            m_back,
        );
    }
    for i in 0..n {
        let j = (i + 1) % n;
        let o0_b = Vec3::new(pts[i].x, pts[i].y, -z_inner);
        let o1_b = Vec3::new(pts[j].x, pts[j].y, -z_inner);
        let o1_f = Vec3::new(pts[j].x, pts[j].y, z_inner);
        let o0_f = Vec3::new(pts[i].x, pts[i].y, z_inner);
        let i0_b = Vec3::new(inset[i].x, inset[i].y, -hd);
        let i1_b = Vec3::new(inset[j].x, inset[j].y, -hd);
        let i1_f = Vec3::new(inset[j].x, inset[j].y, hd);
        let i0_f = Vec3::new(inset[i].x, inset[i].y, hd);

        m.add_quad(o0_b, o1_b, o1_f, o0_f, m_side);  // main side band
        m.add_quad(o0_f, o1_f, i1_f, i0_f, m_front); // front chamfer
        m.add_quad(i0_b, i1_b, o1_b, o0_b, m_back);  // back chamfer
    }
    m
}

fn signed_area_2d(pts: &[Vec2]) -> f32 {
    let mut a = 0.0f32;
    for i in 0..pts.len() {
        let j = (i + 1) % pts.len();
        a += pts[i].perp_dot(pts[j]);
    }
    0.5 * a
}

fn inset_polygon_2d(pts: &[Vec2], amount: f32) -> Vec<Vec2> {
    let n = pts.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let prev = pts[(i + n - 1) % n];
        let curr = pts[i];
        let next = pts[(i + 1) % n];
        let e0 = (curr - prev).normalized();
        let e1 = (next - curr).normalized();
        // Inward normal for CCW polygon is (-ey, ex)
        let n0 = Vec2::new(-e0.y, e0.x);
        let n1 = Vec2::new(-e1.y, e1.x);
        let bis = n0 + n1;
        let bl = bis.len();
        if bl < 1e-5 {
            out.push(curr + n0 * amount);
        } else {
            let bn = bis * (1.0 / bl);
            let denom = bn.dot(n0).max(0.35); // clamp sharp miter
            out.push(curr + bn * (amount / denom));
        }
    }
    out
}

fn point_in_tri_2d(p: Vec2, a: Vec2, b: Vec2, c: Vec2) -> bool {
    let c0 = (b - a).perp_dot(p - a);
    let c1 = (c - b).perp_dot(p - b);
    let c2 = (a - c).perp_dot(p - c);
    c0 >= -1e-6 && c1 >= -1e-6 && c2 >= -1e-6
}

fn ear_clip_2d(pts: &[Vec2]) -> Vec<[usize; 3]> {
    let mut idx: Vec<usize> = (0..pts.len()).collect();
    let mut tris = Vec::new();
    let mut guard = 0usize;
    while idx.len() > 3 && guard < pts.len() * pts.len() + 16 {
        guard += 1;
        let mut clipped = false;
        let m = idx.len();
        for k in 0..m {
            let i0 = idx[(k + m - 1) % m];
            let i1 = idx[k];
            let i2 = idx[(k + 1) % m];
            let (a, b, c) = (pts[i0], pts[i1], pts[i2]);
            if (b - a).perp_dot(c - a) <= 1e-7 {
                continue;
            }
            let mut any_inside = false;
            for &other in &idx {
                if other == i0 || other == i1 || other == i2 {
                    continue;
                }
                if point_in_tri_2d(pts[other], a, b, c) {
                    any_inside = true;
                    break;
                }
            }
            if !any_inside {
                tris.push([i0, i1, i2]);
                idx.remove(k);
                clipped = true;
                break;
            }
        }
        if !clipped {
            break;
        }
    }
    if idx.len() == 3 {
        tris.push([idx[0], idx[1], idx[2]]);
    }
    tris
}

/// Revolve a 2D `(radius, y)` profile around Y. Auto-orders bottom-to-top for outward CCW winding.
pub fn build_lathe(
    profile_in: &[(f32, f32)],
    segments: u32,
    capped: bool,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    if profile_in.len() < 2 {
        return m;
    }
    let mut prof = profile_in.to_vec();
    if prof.first().unwrap().1 > prof.last().unwrap().1 {
        prof.reverse();
    }
    let seg = segments.max(3) as usize;
    let m_top = fc.pick(mat, &["t", "top"]);
    let m_bot = fc.pick(mat, &["b", "bot", "bottom"]);
    let m_side = fc.pick(mat, &["s", "sides"]);

    let pt = |r: f32, y: f32, i: usize| -> Vec3 {
        let a = TAU * (i as f32) / (seg as f32);
        Vec3::new(r * a.cos(), y, -r * a.sin())
    };

    for w in prof.windows(2) {
        let (r0, y0) = (w[0].0.max(0.0), w[0].1);
        let (r1, y1) = (w[1].0.max(0.0), w[1].1);
        if (r0 - r1).abs() < 1e-6 && (y0 - y1).abs() < 1e-6 {
            continue;
        }
        for i in 0..seg {
            let j = (i + 1) % seg;
            if r0 < 1e-5 && r1 >= 1e-5 {
                let p0 = Vec3::new(0.0, y0, 0.0);
                m.add_tri(p0, pt(r1, y1, j), pt(r1, y1, i), m_side);
            } else if r1 < 1e-5 && r0 >= 1e-5 {
                let p1 = Vec3::new(0.0, y1, 0.0);
                m.add_tri(pt(r0, y0, i), pt(r0, y0, j), p1, m_side);
            } else if r0 >= 1e-5 && r1 >= 1e-5 {
                m.add_quad(
                    pt(r0, y0, i),
                    pt(r0, y0, j),
                    pt(r1, y1, j),
                    pt(r1, y1, i),
                    m_side,
                );
            }
        }
    }

    if capped {
        let (rb, yb) = prof[0];
        if rb > 1e-5 {
            let c = Vec3::new(0.0, yb, 0.0);
            for i in 0..seg {
                let j = (i + 1) % seg;
                m.add_tri(c, pt(rb, yb, j), pt(rb, yb, i), m_bot);
            }
        }
        let (rt, yt) = *prof.last().unwrap();
        if rt > 1e-5 {
            let c = Vec3::new(0.0, yt, 0.0);
            for i in 0..seg {
                let j = (i + 1) % seg;
                m.add_tri(c, pt(rt, yt, i), pt(rt, yt, j), m_top);
            }
        }
    }
    m
}

/// Sweep circular cross-sections along a 3D polyline using parallel-transport (Bishop) frames.
pub fn build_tube(
    points: &[Vec3],
    radii: &[f32],
    segments: u32,
    capped: bool,
    mat: ResolvedMat,
) -> Mesh {
    if points.len() < 2 {
        return Mesh::new();
    }
    let sections: Vec<LoftSection> = points
        .iter()
        .enumerate()
        .map(|(i, &p)| {
            let r = radii.get(i).copied().unwrap_or_else(|| *radii.last().unwrap_or(&0.1));
            LoftSection {
                pos: p,
                rx: r,
                ry: r,
                twist_deg: 0.0,
                mat: Some(mat),
            }
        })
        .collect();
    build_loft(&sections, segments, 0.0, capped, mat)
}

#[derive(Debug, Clone, Copy)]
pub struct LoftSection {
    pub pos: Vec3,
    pub rx: f32,
    pub ry: f32,
    pub twist_deg: f32,
    pub mat: Option<ResolvedMat>,
}

/// Sweep elliptical cross-sections along a 3D spine with world-aligned initial frame,
/// parallel transport, optional twist, zero-radius tips, and sticky per-section colors.
pub fn build_loft(
    sections: &[LoftSection],
    segments: u32,
    phase_deg: f32,
    capped: bool,
    default_mat: ResolvedMat,
) -> Mesh {
    let mut m = Mesh::new();
    if sections.len() < 2 {
        return m;
    }
    let seg = segments.max(3) as usize;
    let n = sections.len();

    // Compute smooth tangents along spine
    let mut tangents = Vec::with_capacity(n);
    for i in 0..n {
        let t = if i == 0 {
            sections[1].pos - sections[0].pos
        } else if i == n - 1 {
            sections[n - 1].pos - sections[n - 2].pos
        } else {
            sections[i + 1].pos - sections[i - 1].pos
        };
        let tn = t.normalized();
        tangents.push(if tn.len_sq() > 0.5 { tn } else { Vec3::Y });
    }

    // World-aligned initial frame: try X as right axis so width aligns with +X
    let t0 = tangents[0];
    let mut right = if t0.dot(Vec3::X).abs() < 0.92 {
        let up_tmp = t0.cross(Vec3::X).normalized();
        up_tmp.cross(t0).normalized()
    } else {
        let up_tmp = t0.cross(Vec3::Z).normalized();
        up_tmp.cross(t0).normalized()
    };
    let mut up = t0.cross(right).normalized();

    // Ensure (right, up, tangent) is right-handed: right × up == tangent
    if right.cross(up).dot(t0) < 0.0 {
        up = -up;
    }

    let mut rings: Vec<Vec<Vec3>> = Vec::with_capacity(n);
    let phase = phase_deg.to_radians();

    for i in 0..n {
        let ti = tangents[i];
        if i > 0 {
            // Parallel transport (Bishop frame) from tangents[i-1] to ti
            let t_prev = tangents[i - 1];
            let axis = t_prev.cross(ti);
            let sin_a = axis.len();
            if sin_a > 1e-6 {
                let k = axis / sin_a;
                let cos_a = t_prev.dot(ti).clamp(-1.0, 1.0);
                let ang = sin_a.atan2(cos_a);
                right = rodrigues(right, k, ang).normalized();
            }
            // Re-orthogonalize
            right = (right - ti * right.dot(ti)).normalized();
            up = ti.cross(right).normalized();
            if right.cross(up).dot(ti) < 0.0 {
                up = -up;
            }
        }

        let sec = sections[i];
        let tw = sec.twist_deg.to_radians();
        let (st, ct) = tw.sin_cos();
        let r_axis = right * ct + up * st;
        let u_axis = -right * st + up * ct;

        let mut ring = Vec::with_capacity(seg);
        for s in 0..seg {
            let a = phase + TAU * (s as f32) / (seg as f32);
            let p = sec.pos + r_axis * (sec.rx * a.cos()) + u_axis * (sec.ry * a.sin());
            ring.push(p);
        }
        rings.push(ring);
    }

    let mut cur_mat = sections[0].mat.unwrap_or(default_mat);
    for i in 0..(n - 1) {
        if let Some(sm) = sections[i].mat {
            cur_mat = sm;
        }
        let r0_zero = sections[i].rx.abs() < 1e-5 && sections[i].ry.abs() < 1e-5;
        let r1_zero = sections[i + 1].rx.abs() < 1e-5 && sections[i + 1].ry.abs() < 1e-5;
        for s in 0..seg {
            let sn = (s + 1) % seg;
            // Ring goes CCW around +tangent: (r_i[s], r_i[sn], r_{i+1}[sn], r_{i+1}[s])?
            // Check: along ring s->sn is +up at a=0 (r_axis = +right). Along spine i->i+1 is +tangent.
            // Outward normal at a=0 is +right. We need (b-a) × (d-a) = +right.
            // tangent × up = -right! up × tangent = +right!
            // So (r_i[s], r_i[sn], r_{i+1}[sn], r_{i+1}[s]) has (b-a)=+up, (d-a)=+tangent => (+up) × (+tangent) = +right! OUTWARD!
            if r0_zero && !r1_zero {
                m.add_tri(sections[i].pos, rings[i + 1][sn], rings[i + 1][s], cur_mat);
            } else if r1_zero && !r0_zero {
                m.add_tri(rings[i][s], rings[i][sn], sections[i + 1].pos, cur_mat);
            } else if !r0_zero && !r1_zero {
                m.add_quad(
                    rings[i][s],
                    rings[i][sn],
                    rings[i + 1][sn],
                    rings[i + 1][s],
                    cur_mat,
                );
            }
        }
    }

    if capped {
        let start_mat = sections[0].mat.unwrap_or(default_mat);
        if sections[0].rx.abs() > 1e-5 || sections[0].ry.abs() > 1e-5 {
            for s in 0..seg {
                let sn = (s + 1) % seg;
                // Cap normal should point along -tangent
                m.add_tri(sections[0].pos, rings[0][sn], rings[0][s], start_mat);
            }
        }
        let end_mat = sections[n - 1].mat.unwrap_or(cur_mat);
        if sections[n - 1].rx.abs() > 1e-5 || sections[n - 1].ry.abs() > 1e-5 {
            for s in 0..seg {
                let sn = (s + 1) % seg;
                // Cap normal should point along +tangent
                m.add_tri(sections[n - 1].pos, rings[n - 1][s], rings[n - 1][sn], end_mat);
            }
        }
    }
    m
}

fn rodrigues(v: Vec3, k: Vec3, theta: f32) -> Vec3 {
    let (s, c) = theta.sin_cos();
    v * c + k.cross(v) * s + k * (k.dot(v) * (1.0 - c))
}

/// Subdivided XZ terrain/cargo plane with seeded height jitter, optional smooth dome mound,
/// edge pinning, randomized quad diagonals, multi-color ramp, and optional watertight skirt thickness.
#[allow(clippy::too_many_arguments)]
pub fn build_noisy_plane(
    size_xz: Vec2,
    sub_x: u32,
    sub_z: u32,
    jitter: f32,
    mound: f32,
    edge_pin: bool,
    thickness: f32,
    seed: u64,
    mat: ResolvedMat,
    variants: &[ResolvedMat],
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    let nx = sub_x.max(1) as usize;
    let nz = sub_z.max(1) as usize;
    let hx = size_xz.x.abs() * 0.5;
    let hz = size_xz.y.abs() * 0.5;
    if hx < 1e-6 || hz < 1e-6 {
        return m;
    }

    let mut rng = Rng::new(seed);
    let mut grid = vec![Vec3::ZERO; (nx + 1) * (nz + 1)];

    for iz in 0..=nz {
        let vz = iz as f32 / nz as f32;
        let z = -hz + vz * (2.0 * hz);
        for ix in 0..=nx {
            let ux = ix as f32 / nx as f32;
            let x = -hx + ux * (2.0 * hx);
            let on_edge = ix == 0 || ix == nx || iz == 0 || iz == nz;
            let falloff = (std::f32::consts::PI * ux).sin() * (std::f32::consts::PI * vz).sin();
            let mut y = mound * falloff;
            if !edge_pin || !on_edge {
                let w = if edge_pin { falloff.sqrt() } else { 1.0 };
                y += rng.signed_f32() * jitter * w;
            }
            grid[iz * (nx + 1) + ix] = Vec3::new(x, y, z);
        }
    }

    let pick_top = |rng: &mut Rng| -> ResolvedMat {
        if !variants.is_empty() {
            variants[rng.usize(variants.len())]
        } else {
            fc.pick(mat, &["t", "top"])
        }
    };

    for iz in 0..nz {
        for ix in 0..nx {
            // Grid indices: p00=(-x,-z), p10=(+x,-z), p11=(+x,+z), p01=(-x,+z)
            // For +Y outward normal: (p01 - p00) = +Z, (p10 - p00) = +X; (+Z) × (+X) = +Y!
            let p00 = grid[iz * (nx + 1) + ix];
            let p10 = grid[iz * (nx + 1) + (ix + 1)];
            let p11 = grid[(iz + 1) * (nx + 1) + (ix + 1)];
            let p01 = grid[(iz + 1) * (nx + 1) + ix];
            let m0 = pick_top(&mut rng);
            let m1 = pick_top(&mut rng);
            if (rng.next_u64() & 1) == 0 {
                m.add_tri(p00, p01, p11, m0);
                m.add_tri(p00, p11, p10, m1);
            } else {
                m.add_tri(p00, p01, p10, m0);
                m.add_tri(p10, p01, p11, m1);
            }
        }
    }

    if thickness > 1e-5 {
        let y_bot = -thickness;
        let m_side = fc.pick(mat, &["s", "sides"]);
        let m_bot = fc.pick(mat, &["b", "bot", "bottom"]);
        // Front (+Z) edge
        for ix in 0..nx {
            let t0 = grid[nz * (nx + 1) + ix];
            let t1 = grid[nz * (nx + 1) + ix + 1];
            let b0 = Vec3::new(t0.x, y_bot, t0.z);
            let b1 = Vec3::new(t1.x, y_bot, t1.z);
            m.add_quad(b0, b1, t1, t0, m_side);
        }
        // Back (-Z) edge
        for ix in 0..nx {
            let t0 = grid[ix];
            let t1 = grid[ix + 1];
            let b0 = Vec3::new(t0.x, y_bot, t0.z);
            let b1 = Vec3::new(t1.x, y_bot, t1.z);
            m.add_quad(b1, b0, t0, t1, m_side);
        }
        // Right (+X) edge
        for iz in 0..nz {
            let t0 = grid[(iz + 1) * (nx + 1) + nx];
            let t1 = grid[iz * (nx + 1) + nx];
            let b0 = Vec3::new(t0.x, y_bot, t0.z);
            let b1 = Vec3::new(t1.x, y_bot, t1.z);
            m.add_quad(b0, b1, t1, t0, m_side);
        }
        // Left (-X) edge
        for iz in 0..nz {
            let t0 = grid[iz * (nx + 1)];
            let t1 = grid[(iz + 1) * (nx + 1)];
            let b0 = Vec3::new(t0.x, y_bot, t0.z);
            let b1 = Vec3::new(t1.x, y_bot, t1.z);
            m.add_quad(b0, b1, t1, t0, m_side);
        }
        // Flat bottom closing slab (-Y outward)
        let c00 = Vec3::new(-hx, y_bot, -hz);
        let c10 = Vec3::new(hx, y_bot, -hz);
        let c11 = Vec3::new(hx, y_bot, hz);
        let c01 = Vec3::new(-hx, y_bot, hz);
        m.add_quad(c00, c10, c11, c01, m_bot);
    }

    m
}

/// Tapered curved ribbon / foliage blade rising along +Y from `y=0..h`, curving toward +Z by `curve`.
/// When `double_sided` is true, emits front and reversed back faces separated by `±offset` along the
/// local blade normal to satisfy backface culling with zero Z-fighting.
pub fn build_ribbon(
    w0: f32,
    w1: f32,
    height: f32,
    curve: f32,
    segments: u32,
    double_sided: bool,
    offset: f32,
    mat: ResolvedMat,
    fc: &FaceMats,
) -> Mesh {
    let mut m = Mesh::new();
    let seg = segments.max(1) as usize;
    let h = height.abs();
    let hw0 = w0.abs() * 0.5;
    let hw1 = w1.abs() * 0.5;
    if h < 1e-6 || (hw0 < 1e-6 && hw1 < 1e-6) {
        return m;
    }
    let m_front = fc.pick(mat, &["f", "front", "t", "top"]);
    let m_back = fc.pick(mat, &["k", "back", "b", "bot", "bottom"]);
    let off = if double_sided { offset.abs().max(1e-4) } else { 0.0 };

    // Spine along Y in [0, h] with quadratic forward droop z(u) = curve * u^2
    let mut left_f = Vec::with_capacity(seg + 1);
    let mut right_f = Vec::with_capacity(seg + 1);
    let mut left_b = Vec::with_capacity(seg + 1);
    let mut right_b = Vec::with_capacity(seg + 1);
    let mut half_w = Vec::with_capacity(seg + 1);

    for i in 0..=seg {
        let u = i as f32 / seg as f32;
        let y = h * u;
        let z = curve * u * u;
        let hw = hw0 + (hw1 - hw0) * u;
        // Tangent along spine is (0, h, 2*curve*u); front normal (right=+X, up=tangent) = tangent × X = (0, 2*curve*u, -h)?
        // Wait: at u=0 (flat vertical ribbon), front face faces +Z: (right - left) = +X, (top - bot) = +Y => X × Y = +Z!
        // So normal = X × tangent = (0, -dz/du, dy/du) = (0, -2*curve*u, h) normalized!
        let n = Vec3::new(0.0, -2.0 * curve * u, h).normalized();
        let shift_f = n * off;
        let shift_b = -shift_f;
        left_f.push(Vec3::new(-hw, y, z) + shift_f);
        right_f.push(Vec3::new(hw, y, z) + shift_f);
        left_b.push(Vec3::new(-hw, y, z) + shift_b);
        right_b.push(Vec3::new(hw, y, z) + shift_b);
        half_w.push(hw);
    }

    for i in 0..seg {
        let w_bot = half_w[i];
        let w_top = half_w[i + 1];
        if w_top < 1e-5 && w_bot >= 1e-5 {
            // Pointed tip triangle
            let tip_f = (left_f[i + 1] + right_f[i + 1]) * 0.5;
            m.add_tri(left_f[i], right_f[i], tip_f, m_front);
            if double_sided {
                let tip_b = (left_b[i + 1] + right_b[i + 1]) * 0.5;
                m.add_tri(right_b[i], left_b[i], tip_b, m_back);
            }
        } else if w_bot < 1e-5 && w_top >= 1e-5 {
            let tip_f = (left_f[i] + right_f[i]) * 0.5;
            m.add_tri(tip_f, right_f[i + 1], left_f[i + 1], m_front);
            if double_sided {
                let tip_b = (left_b[i] + right_b[i]) * 0.5;
                m.add_tri(tip_b, left_b[i + 1], right_b[i + 1], m_back);
            }
        } else if w_bot >= 1e-5 && w_top >= 1e-5 {
            m.add_quad(left_f[i], right_f[i], right_f[i + 1], left_f[i + 1], m_front);
            if double_sided {
                m.add_quad(right_b[i], left_b[i], left_b[i + 1], right_b[i + 1], m_back);
            }
        }
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_mat() -> ResolvedMat {
        ResolvedMat {
            uv: Vec2::new(0.5, 0.5),
            is_translucent: false,
        }
    }

    /// Every triangle of a closed convex solid must satisfy dot(tri_centroid - interior, normal) > 0.
    fn assert_outward_convex(mesh: &Mesh, name: &str) {
        assert!(!mesh.tris.is_empty(), "{name} has no triangles");
        let mut interior = Vec3::ZERO;
        for t in &mesh.tris {
            interior = interior + (t.a + t.b + t.c) * (1.0 / 3.0);
        }
        interior = interior / (mesh.tris.len() as f32);
        for (i, t) in mesh.tris.iter().enumerate() {
            let c = (t.a + t.b + t.c) * (1.0 / 3.0);
            assert!(
                (c - interior).dot(t.face_normal) > 0.0,
                "{name} tri {i} inverted! centroid={c:?} interior={interior:?} normal={:?}",
                t.face_normal
            );
        }
    }

    #[test]
    fn primitive_winding_checks() {
        let mat = dummy_mat();
        let fc = FaceMats::default();
        assert_outward_convex(&build_box(Vec3::ONE, Vec2::new(1.0, 1.0), Vec2::ZERO, 0.0, mat, &fc), "box");
        assert_outward_convex(&build_box(Vec3::ONE, Vec2::new(1.0, 1.0), Vec2::ZERO, 0.12, mat, &fc), "bevel_box");
        assert_outward_convex(&build_cylinder(0.5, 0.4, 1.0, 12, true, 0.05, mat, &fc), "bevel_cyl");
        assert_outward_convex(&build_cylinder(0.5, 0.0, 1.0, 12, true, 0.0, mat, &fc), "cone");
        assert_outward_convex(&build_capsule(0.25, 0.2, 1.0, 10, 4, mat), "capsule");
        assert_outward_convex(&build_wedge(Vec3::ONE, mat, &fc), "wedge");
        assert_outward_convex(&build_pyramid(Vec3::ONE, 4, Vec2::ZERO, mat, &fc), "pyramid4");
        assert_outward_convex(&build_pyramid(Vec3::ONE, 6, Vec2::ZERO, mat, &fc), "pyramid6");
        assert_outward_convex(&build_icosphere(1.0, 2, 0.15, 42, mat, &[]), "icosphere");
        let sq = vec![Vec2::new(-1.0, -1.0), Vec2::new(1.0, -1.0), Vec2::new(1.0, 1.0), Vec2::new(-1.0, 1.0)];
        assert_outward_convex(&build_prism(&sq, 1.0, 0.1, mat, &fc), "bevel_prism");
        let loft = vec![
            LoftSection { pos: Vec3::new(0.0, -1.0, 0.0), rx: 0.4, ry: 0.3, twist_deg: 0.0, mat: None },
            LoftSection { pos: Vec3::new(0.0, 0.0, 0.0), rx: 0.6, ry: 0.5, twist_deg: 10.0, mat: None },
            LoftSection { pos: Vec3::new(0.0, 1.0, 0.0), rx: 0.3, ry: 0.25, twist_deg: 20.0, mat: None },
        ];
        assert_outward_convex(&build_loft(&loft, 10, 0.0, true, mat), "loft");
    }
}
