use std::collections::HashMap;

use crate::common::math::{Aabb, Mat4, Vec2, Vec3};
use crate::mesh::palette::ResolvedMat;

#[derive(Debug, Clone, Copy)]
pub struct Triangle {
    pub a: Vec3,
    pub b: Vec3,
    pub c: Vec3,
    pub na: Vec3,
    pub nb: Vec3,
    pub nc: Vec3,
    pub face_normal: Vec3,
    pub uv: Vec2,
    pub is_translucent: bool,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub tris: Vec<Triangle>,
    /// Optional bounding boxes keyed by `@id` tags for `--focus @id` close-up renders.
    pub part_bounds: HashMap<String, Aabb>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_tri(&mut self, a: Vec3, b: Vec3, c: Vec3, mat: ResolvedMat) {
        let cross = (b - a).cross(c - a);
        let area2 = cross.len();
        if area2 < 1e-10 {
            return;
        }
        let n = cross / area2;
        self.tris.push(Triangle {
            a,
            b,
            c,
            na: n,
            nb: n,
            nc: n,
            face_normal: n,
            uv: mat.uv,
            is_translucent: mat.is_translucent,
        });
    }

    /// Outward-CCW quad split into (a, b, c) and (a, c, d).
    pub fn add_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, mat: ResolvedMat) {
        self.add_tri(a, b, c, mat);
        self.add_tri(a, c, d, mat);
    }

    pub fn compute_bounds(&self) -> Aabb {
        let mut bb = Aabb::empty();
        for t in &self.tris {
            bb.expand(t.a);
            bb.expand(t.b);
            bb.expand(t.c);
        }
        bb
    }

    pub fn translate(&mut self, delta: Vec3) {
        for t in &mut self.tris {
            t.a = t.a + delta;
            t.b = t.b + delta;
            t.c = t.c + delta;
        }
    }

    /// Apply area-weighted smooth vertex normals across shared positions within this local shape
    /// (preserving sharp seams across different palette cells / UVs).
    pub fn compute_smooth_normals(&mut self) {
        let quant = |p: Vec3, uv: Vec2| -> (i32, i32, i32, i32, i32) {
            (
                (p.x * 10000.0).round() as i32,
                (p.y * 10000.0).round() as i32,
                (p.z * 10000.0).round() as i32,
                (uv.x * 10000.0).round() as i32,
                (uv.y * 10000.0).round() as i32,
            )
        };
        let mut accum: HashMap<(i32, i32, i32, i32, i32), Vec3> = HashMap::new();
        for t in &self.tris {
            let weighted = (t.b - t.a).cross(t.c - t.a); // magnitude = 2 * area
            for p in [t.a, t.b, t.c] {
                let k = quant(p, t.uv);
                let e = accum.entry(k).or_insert(Vec3::ZERO);
                *e = *e + weighted;
            }
        }
        for t in &mut self.tris {
            let ka = quant(t.a, t.uv);
            let kb = quant(t.b, t.uv);
            let kc = quant(t.c, t.uv);
            let na = accum.get(&ka).copied().unwrap_or(t.face_normal).normalized();
            let nb = accum.get(&kb).copied().unwrap_or(t.face_normal).normalized();
            let nc = accum.get(&kc).copied().unwrap_or(t.face_normal).normalized();
            t.na = if na.len_sq() > 0.5 { na } else { t.face_normal };
            t.nb = if nb.len_sq() > 0.5 { nb } else { t.face_normal };
            t.nc = if nc.len_sq() > 0.5 { nc } else { t.face_normal };
        }
    }

    /// Recompute flat normals from current vertex positions (call after vertex deformers if flat).
    pub fn recompute_flat_normals(&mut self) {
        for t in &mut self.tris {
            let n = (t.b - t.a).cross(t.c - t.a).normalized();
            if n.len_sq() > 0.5 {
                t.face_normal = n;
                t.na = n;
                t.nb = n;
                t.nc = n;
            }
        }
    }

    /// Conforming plane-slice subdivision along `axis_idx` so adjacent faces share exact cut vertices
    /// with zero T-junction cracks when bent or twisted (even on chamfered/beveled meshes).
    fn subdivide_along_axis(&mut self, axis_idx: usize, min_a: f32, max_a: f32, segments: usize) {
        if segments <= 1 || (max_a - min_a).abs() < 1e-6 {
            return;
        }
        let step = (max_a - min_a) / segments as f32;
        for k in 1..segments {
            let cut = min_a + step * k as f32;
            let mut next = Vec::with_capacity(self.tris.len() + 32);
            for t in &self.tris {
                let verts = [(t.a, t.na), (t.b, t.nb), (t.c, t.nc)];
                let d = [
                    get_comp(t.a, axis_idx) - cut,
                    get_comp(t.b, axis_idx) - cut,
                    get_comp(t.c, axis_idx) - cut,
                ];
                let neg = d.iter().filter(|&&v| v < -1e-6).count();
                let pos = d.iter().filter(|&&v| v > 1e-6).count();
                if neg == 0 || pos == 0 {
                    next.push(*t);
                    continue;
                }
                // Split polygon (triangle) into below-plane and above-plane convex pieces (preserving CCW winding)
                let mut below: Vec<(Vec3, Vec3)> = Vec::with_capacity(4);
                let mut above: Vec<(Vec3, Vec3)> = Vec::with_capacity(4);
                for i in 0..3 {
                    let j = (i + 1) % 3;
                    let (p0, n0) = verts[i];
                    let (p1, n1) = verts[j];
                    let d0 = d[i];
                    let d1 = d[j];
                    if d0 <= 1e-6 {
                        below.push((p0, n0));
                    }
                    if d0 >= -1e-6 {
                        above.push((p0, n0));
                    }
                    if (d0 < -1e-6 && d1 > 1e-6) || (d0 > 1e-6 && d1 < -1e-6) {
                        let u = d0 / (d0 - d1);
                        let pm = p0.lerp(p1, u);
                        let nm = n0.lerp(n1, u).normalized();
                        below.push((pm, nm));
                        above.push((pm, nm));
                    }
                }
                for poly in [&below, &above] {
                    if poly.len() >= 3 {
                        for m in 1..(poly.len() - 1) {
                            let (pa, na) = poly[0];
                            let (pb, nb) = poly[m];
                            let (pc, nc) = poly[m + 1];
                            let fn_v = (pb - pa).cross(pc - pa).normalized();
                            if fn_v.len_sq() > 0.5 {
                                next.push(Triangle {
                                    a: pa,
                                    b: pb,
                                    c: pc,
                                    na,
                                    nb,
                                    nc,
                                    face_normal: fn_v,
                                    uv: t.uv,
                                    is_translucent: t.is_translucent,
                                });
                            }
                        }
                    }
                }
            }
            self.tris = next;
        }
    }

    /// Bend vertices along a local primary axis (`x`, `y`, `z`) into an arc of `angle_deg`
    /// toward a secondary direction (`x`, `y`, `z`).
    pub fn apply_bend(&mut self, angle_deg: f32, along: char, toward: char, smooth: bool) {
        if angle_deg.abs() < 1e-4 || self.tris.is_empty() {
            return;
        }
        let bb = self.compute_bounds();
        let axis_idx = axis_to_idx(along);
        let dir_idx = if toward == along {
            (axis_idx + 1) % 3
        } else {
            axis_to_idx(toward)
        };
        let min_a = get_comp(bb.min, axis_idx);
        let max_a = get_comp(bb.max, axis_idx);
        let span = max_a - min_a;
        if span.abs() < 1e-6 {
            return;
        }
        let target_segs = (angle_deg.abs() / 10.0).ceil().clamp(6.0, 24.0) as usize;
        self.subdivide_along_axis(axis_idx, min_a, max_a, target_segs);
        let center_a = 0.5 * (min_a + max_a);
        let total_rad = angle_deg.to_radians();
        let radius = span / total_rad; // signed curvature radius

        let deform = |p: Vec3| -> Vec3 {
            let u = get_comp(p, axis_idx) - center_a;
            let v = get_comp(p, dir_idx);
            let theta = u / radius;
            let (s, c) = theta.sin_cos();
            let r_eff = radius - v;
            let new_a = center_a + r_eff * s;
            let new_d = radius - r_eff * c;
            let mut out = p;
            set_comp(&mut out, axis_idx, new_a);
            set_comp(&mut out, dir_idx, new_d);
            out
        };

        for t in &mut self.tris {
            t.a = deform(t.a);
            t.b = deform(t.b);
            t.c = deform(t.c);
        }
        self.recompute_flat_normals();
        if smooth {
            self.compute_smooth_normals();
        }
    }

    /// Twist vertices helically around a local axis (`x`, `y`, `z`) by `angle_deg` over the shape's span.
    pub fn apply_twist(&mut self, angle_deg: f32, axis: char, smooth: bool) {
        if angle_deg.abs() < 1e-4 || self.tris.is_empty() {
            return;
        }
        let bb = self.compute_bounds();
        let idx = axis_to_idx(axis);
        let min_a = get_comp(bb.min, idx);
        let max_a = get_comp(bb.max, idx);
        let span = max_a - min_a;
        if span.abs() < 1e-6 {
            return;
        }
        let target_segs = (angle_deg.abs() / 12.0).ceil().clamp(6.0, 24.0) as usize;
        self.subdivide_along_axis(idx, min_a, max_a, target_segs);
        let total_rad = angle_deg.to_radians();

        let deform = |p: Vec3| -> Vec3 {
            let t = ((get_comp(p, idx) - min_a) / span) - 0.5;
            let (s, c) = (t * total_rad).sin_cos();
            match idx {
                0 => Vec3::new(p.x, p.y * c - p.z * s, p.y * s + p.z * c),
                1 => Vec3::new(p.x * c + p.z * s, p.y, -p.x * s + p.z * c),
                _ => Vec3::new(p.x * c - p.y * s, p.x * s + p.y * c, p.z),
            }
        };

        for t in &mut self.tris {
            t.a = deform(t.a);
            t.b = deform(t.b);
            t.c = deform(t.c);
        }
        self.recompute_flat_normals();
        if smooth {
            self.compute_smooth_normals();
        }
    }

    /// Append a transformed copy of `src`, automatically flipping winding on negative-determinant mirrors.
    pub fn append_transformed(&mut self, src: &Mesh, mat: Mat4, tag: Option<&str>) {
        let flip = mat.det3() < 0.0;
        let mut tag_bb = Aabb::empty();

        for t in &src.tris {
            let pa = mat.transform_point(t.a);
            let pb = mat.transform_point(t.b);
            let pc = mat.transform_point(t.c);

            let (a, b, c, na, nb, nc) = if flip {
                (
                    pa,
                    pc,
                    pb,
                    mat.transform_normal(t.na),
                    mat.transform_normal(t.nc),
                    mat.transform_normal(t.nb),
                )
            } else {
                (
                    pa,
                    pb,
                    pc,
                    mat.transform_normal(t.na),
                    mat.transform_normal(t.nb),
                    mat.transform_normal(t.nc),
                )
            };

            let face_n = (b - a).cross(c - a).normalized();
            if face_n.len_sq() < 0.5 {
                continue;
            }
            if tag.is_some() {
                tag_bb.expand(a);
                tag_bb.expand(b);
                tag_bb.expand(c);
            }
            self.tris.push(Triangle {
                a,
                b,
                c,
                na: if na.len_sq() > 0.5 { na } else { face_n },
                nb: if nb.len_sq() > 0.5 { nb } else { face_n },
                nc: if nc.len_sq() > 0.5 { nc } else { face_n },
                face_normal: face_n,
                uv: t.uv,
                is_translucent: t.is_translucent,
            });
        }

        if let Some(id) = tag {
            if tag_bb.is_valid() {
                self.part_bounds
                    .entry(id.to_string())
                    .and_modify(|existing| existing.expand_aabb(&tag_bb))
                    .or_insert(tag_bb);
            }
        }
        // Propagate inner tagged sub-parts transformed by `mat`
        for (sub_id, sub_bb) in &src.part_bounds {
            let mut xbb = Aabb::empty();
            for sx in [sub_bb.min.x, sub_bb.max.x] {
                for sy in [sub_bb.min.y, sub_bb.max.y] {
                    for sz in [sub_bb.min.z, sub_bb.max.z] {
                        xbb.expand(mat.transform_point(Vec3::new(sx, sy, sz)));
                    }
                }
            }
            self.part_bounds
                .entry(sub_id.clone())
                .and_modify(|e| e.expand_aabb(&xbb))
                .or_insert(xbb);
        }
    }
}

fn axis_to_idx(c: char) -> usize {
    match c.to_ascii_lowercase() {
        'x' => 0,
        'y' => 1,
        _ => 2,
    }
}

fn get_comp(v: Vec3, i: usize) -> f32 {
    match i {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

fn set_comp(v: &mut Vec3, i: usize, val: f32) {
    match i {
        0 => v.x = val,
        1 => v.y = val,
        _ => v.z = val,
    }
}
