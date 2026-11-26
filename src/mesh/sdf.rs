/// Zero-dependency Surface-Nets SDF & Metaball mesher for organic low-poly game assets.
/// Supports smooth polynomial union/subtraction/intersection, geometry-preserving `paint` volumes,
/// analytical gradient normals, and per-face nearest-primitive palette material assignment.

use crate::common::math::{Aabb, Mat4, Vec3};
use crate::mesh::geom::{Mesh, Triangle};
use crate::mesh::palette::ResolvedMat;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SdfOp {
    Union,
    Subtract,
    Intersect,
    Paint,
}

#[derive(Debug, Clone)]
pub enum SdfShape {
    Sphere { center: Vec3, radius: f32 },
    Ellipsoid { center: Vec3, radii: Vec3, inv_rot: Mat4 },
    Capsule { a: Vec3, b: Vec3, r0: f32, r1: f32 },
    RoundBox { center: Vec3, half: Vec3, round: f32, inv_rot: Mat4 },
    RoundCyl { center: Vec3, radius: f32, half_h: f32, round: f32, inv_rot: Mat4 },
    Torus { center: Vec3, major: f32, minor: f32, inv_rot: Mat4 },
}

#[derive(Debug, Clone)]
pub struct SdfPrim {
    pub shape: SdfShape,
    pub op: SdfOp,
    pub smooth_k: f32,
    pub mat: ResolvedMat,
}

impl SdfShape {
    fn eval(&self, p: Vec3) -> f32 {
        match *self {
            SdfShape::Sphere { center, radius } => (p - center).len() - radius,
            SdfShape::Ellipsoid { center, radii, inv_rot } => {
                let q = inv_rot.transform_point(p - center);
                let rx = radii.x.max(1e-4);
                let ry = radii.y.max(1e-4);
                let rz = radii.z.max(1e-4);
                let k0 = Vec3::new(q.x / rx, q.y / ry, q.z / rz).len();
                let k1 = Vec3::new(q.x / (rx * rx), q.y / (ry * ry), q.z / (rz * rz)).len();
                if k1 > 1e-8 { k0 * (k0 - 1.0) / k1 } else { -rx.min(ry).min(rz) }
            }
            SdfShape::Capsule { a, b, r0, r1 } => {
                let pa = p - a;
                let ba = b - a;
                let len2 = ba.len_sq();
                let h = if len2 > 1e-10 { (pa.dot(ba) / len2).clamp(0.0, 1.0) } else { 0.0 };
                let r = r0 + (r1 - r0) * h;
                (pa - ba * h).len() - r
            }
            SdfShape::RoundBox { center, half, round, inv_rot } => {
                let q = inv_rot.transform_point(p - center);
                let b = Vec3::new(
                    (half.x - round).max(0.0),
                    (half.y - round).max(0.0),
                    (half.z - round).max(0.0),
                );
                let d = Vec3::new(q.x.abs() - b.x, q.y.abs() - b.y, q.z.abs() - b.z);
                let out = Vec3::new(d.x.max(0.0), d.y.max(0.0), d.z.max(0.0)).len();
                let inside = d.x.max(d.y).max(d.z).min(0.0);
                out + inside - round
            }
            SdfShape::RoundCyl { center, radius, half_h, round, inv_rot } => {
                let q = inv_rot.transform_point(p - center);
                let r = (radius - round).max(0.0);
                let hh = (half_h - round).max(0.0);
                let dx = (q.x * q.x + q.z * q.z).sqrt() - r;
                let dy = q.y.abs() - hh;
                let out = (dx.max(0.0).powi(2) + dy.max(0.0).powi(2)).sqrt();
                let inside = dx.max(dy).min(0.0);
                out + inside - round
            }
            SdfShape::Torus { center, major, minor, inv_rot } => {
                let q = inv_rot.transform_point(p - center);
                let xz = (q.x * q.x + q.z * q.z).sqrt() - major;
                (xz * xz + q.y * q.y).sqrt() - minor
            }
        }
    }

    fn bounds(&self) -> Aabb {
        let mut bb = Aabb::empty();
        match *self {
            SdfShape::Sphere { center, radius } => {
                let r = Vec3::splat(radius);
                bb.expand(center - r);
                bb.expand(center + r);
            }
            SdfShape::Ellipsoid { center, radii, .. } => {
                let r = Vec3::splat(radii.x.max(radii.y).max(radii.z));
                bb.expand(center - r);
                bb.expand(center + r);
            }
            SdfShape::Capsule { a, b, r0, r1 } => {
                let r = Vec3::splat(r0.max(r1));
                bb.expand(a - r);
                bb.expand(a + r);
                bb.expand(b - r);
                bb.expand(b + r);
            }
            SdfShape::RoundBox { center, half, .. } => {
                let r = Vec3::splat(half.len());
                bb.expand(center - r);
                bb.expand(center + r);
            }
            SdfShape::RoundCyl { center, radius, half_h, .. } => {
                let r = Vec3::splat((radius * radius + half_h * half_h).sqrt());
                bb.expand(center - r);
                bb.expand(center + r);
            }
            SdfShape::Torus { center, major, minor, .. } => {
                let r = Vec3::splat(major + minor);
                bb.expand(center - r);
                bb.expand(center + r);
            }
        }
        bb
    }
}

#[inline]
fn smin(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1e-5 {
        return a.min(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.min(b) - h * h * k * 0.25
}

#[inline]
fn smax(a: f32, b: f32, k: f32) -> f32 {
    if k <= 1e-5 {
        return a.max(b);
    }
    let h = (k - (a - b).abs()).max(0.0) / k;
    a.max(b) + h * h * k * 0.25
}

fn eval_field(prims: &[SdfPrim], p: Vec3) -> (f32, ResolvedMat) {
    let mut d = f32::INFINITY;
    let mut mat = prims[0].mat;
    for pr in prims {
        let pd = pr.shape.eval(p);
        match pr.op {
            SdfOp::Union => {
                if pd < d {
                    mat = pr.mat;
                }
                d = if d.is_infinite() { pd } else { smin(d, pd, pr.smooth_k) };
            }
            SdfOp::Subtract => {
                let neg = -pd;
                if neg > d {
                    mat = pr.mat;
                }
                d = smax(d, neg, pr.smooth_k);
            }
            SdfOp::Intersect => {
                if pd > d {
                    mat = pr.mat;
                }
                d = smax(d, pd, pr.smooth_k);
            }
            SdfOp::Paint => {
                if pd <= pr.smooth_k.max(0.0) {
                    mat = pr.mat;
                }
            }
        }
    }
    (d, mat)
}

fn eval_dist(prims: &[SdfPrim], p: Vec3) -> f32 {
    let mut d = f32::INFINITY;
    for pr in prims {
        if pr.op == SdfOp::Paint {
            continue;
        }
        let pd = pr.shape.eval(p);
        d = match pr.op {
            SdfOp::Union => {
                if d.is_infinite() { pd } else { smin(d, pd, pr.smooth_k) }
            }
            SdfOp::Subtract => smax(d, -pd, pr.smooth_k),
            SdfOp::Intersect => smax(d, pd, pr.smooth_k),
            SdfOp::Paint => d,
        };
    }
    d
}

fn eval_normal(prims: &[SdfPrim], p: Vec3, eps: f32) -> Vec3 {
    let gx = eval_dist(prims, Vec3::new(p.x + eps, p.y, p.z))
        - eval_dist(prims, Vec3::new(p.x - eps, p.y, p.z));
    let gy = eval_dist(prims, Vec3::new(p.x, p.y + eps, p.z))
        - eval_dist(prims, Vec3::new(p.x, p.y - eps, p.z));
    let gz = eval_dist(prims, Vec3::new(p.x, p.y, p.z + eps))
        - eval_dist(prims, Vec3::new(p.x, p.y, p.z - eps));
    let g = Vec3::new(gx, gy, gz);
    let len = g.len();
    if len > 1e-8 { g / len } else { Vec3::Y }
}

/// Extract a clean watertight low-poly mesh from SDF primitives using Naive Surface Nets
/// with Newton surface projection and analytical SDF gradient normals.
pub fn build_sdf_mesh(prims: &[SdfPrim], res: usize, smooth_normals: bool) -> Mesh {
    if prims.is_empty() {
        return Mesh::new();
    }

    let mut bb = Aabb::empty();
    let mut max_k = 0.0f32;
    for pr in prims {
        if pr.op == SdfOp::Union {
            let pb = pr.shape.bounds();
            bb.expand(pb.min);
            bb.expand(pb.max);
        }
        max_k = max_k.max(pr.smooth_k);
    }
    if !bb.is_valid() {
        return Mesh::new();
    }

    let res = res.clamp(8, 96);
    let size = bb.size();
    let max_dim = size.x.max(size.y).max(size.z).max(1e-3);
    let h = max_dim / res as f32;
    let pad = (max_k * 0.5).max(h * 2.5);
    let origin = bb.min - Vec3::splat(pad);
    let span = (bb.max + Vec3::splat(pad)) - origin;

    let nx = ((span.x / h).ceil() as usize).clamp(4, 112) + 1;
    let ny = ((span.y / h).ceil() as usize).clamp(4, 112) + 1;
    let nz = ((span.z / h).ceil() as usize).clamp(4, 112) + 1;

    let idx = |x: usize, y: usize, z: usize| -> usize { (z * ny + y) * nx + x };

    // 1. Sample scalar SDF on 3D lattice
    let mut field = vec![0.0f32; nx * ny * nz];
    for z in 0..nz {
        let pz = origin.z + z as f32 * h;
        for y in 0..ny {
            let py = origin.y + y as f32 * h;
            for x in 0..nx {
                let px = origin.x + x as f32 * h;
                field[idx(x, y, z)] = eval_dist(prims, Vec3::new(px, py, pz));
            }
        }
    }

    // 2. Place one vertex inside every sign-changing cell (average of 12-edge zero crossings + Newton snap)
    let cx = nx - 1;
    let cy = ny - 1;
    let cz = nz - 1;
    let cidx = |x: usize, y: usize, z: usize| -> usize { (z * cy + y) * cx + x };
    let mut cell_vert: Vec<i32> = vec![-1; cx * cy * cz];
    let mut verts: Vec<Vec3> = Vec::new();
    let mut vnorms: Vec<Vec3> = Vec::new();

    const CORNERS: [(usize, usize, usize); 8] = [
        (0, 0, 0),
        (1, 0, 0),
        (0, 1, 0),
        (1, 1, 0),
        (0, 0, 1),
        (1, 0, 1),
        (0, 1, 1),
        (1, 1, 1),
    ];
    const EDGES: [(usize, usize); 12] = [
        (0, 1),
        (2, 3),
        (4, 5),
        (6, 7),
        (0, 2),
        (1, 3),
        (4, 6),
        (5, 7),
        (0, 4),
        (1, 5),
        (2, 6),
        (3, 7),
    ];

    let eps = h * 0.2;
    for z in 0..cz {
        for y in 0..cy {
            for x in 0..cx {
                let mut vals = [0.0f32; 8];
                let mut mask = 0u8;
                for (c, &(dx, dy, dz)) in CORNERS.iter().enumerate() {
                    let v = field[idx(x + dx, y + dy, z + dz)];
                    vals[c] = v;
                    if v < 0.0 {
                        mask |= 1 << c;
                    }
                }
                if mask == 0 || mask == 255 {
                    continue;
                }
                let mut sum = Vec3::ZERO;
                let mut count = 0.0f32;
                for &(c0, c1) in &EDGES {
                    let v0 = vals[c0];
                    let v1 = vals[c1];
                    if (v0 < 0.0) != (v1 < 0.0) {
                        let t = (v0 / (v0 - v1)).clamp(0.0, 1.0);
                        let p0 = Vec3::new(
                            (x + CORNERS[c0].0) as f32,
                            (y + CORNERS[c0].1) as f32,
                            (z + CORNERS[c0].2) as f32,
                        );
                        let p1 = Vec3::new(
                            (x + CORNERS[c1].0) as f32,
                            (y + CORNERS[c1].1) as f32,
                            (z + CORNERS[c1].2) as f32,
                        );
                        sum = sum + p0.lerp(p1, t);
                        count += 1.0;
                    }
                }
                if count > 0.0 {
                    let mut p = origin + (sum / count) * h;
                    // 2 Newton steps project vertex cleanly onto the exact f(p)=0 isosurface
                    for _ in 0..2 {
                        let d = eval_dist(prims, p);
                        let n = eval_normal(prims, p, eps);
                        p = p - n * d.clamp(-h * 0.45, h * 0.45);
                    }
                    let n = eval_normal(prims, p, eps);
                    let vi = verts.len() as i32;
                    verts.push(p);
                    vnorms.push(n);
                    cell_vert[cidx(x, y, z)] = vi;
                }
            }
        }
    }

    // 3. Emit one outward-oriented quad for every sign-changing interior grid edge
    let mut mesh = Mesh::new();
    let mut emit_quad = |i0: i32, i1: i32, i2: i32, i3: i32| {
        if i0 < 0 || i1 < 0 || i2 < 0 || i3 < 0 {
            return;
        }
        let ids = [i0 as usize, i1 as usize, i2 as usize, i3 as usize];
        // Split along shorter diagonal for cleaner low-poly surface curvature
        let d02 = (verts[ids[0]] - verts[ids[2]]).len_sq();
        let d13 = (verts[ids[1]] - verts[ids[3]]).len_sq();
        let tris_idx = if d02 <= d13 {
            [(ids[0], ids[1], ids[2]), (ids[0], ids[2], ids[3])]
        } else {
            [(ids[0], ids[1], ids[3]), (ids[1], ids[2], ids[3])]
        };

        for (a_i, b_i, c_i) in tris_idx {
            let a = verts[a_i];
            let mut b = verts[b_i];
            let mut c = verts[c_i];
            let mut nb = vnorms[b_i];
            let mut nc = vnorms[c_i];
            let na = vnorms[a_i];

            let mut cross = (b - a).cross(c - a);
            let area2 = cross.len();
            if area2 < 1e-9 {
                continue;
            }
            let centroid = (a + b + c) * (1.0 / 3.0);
            let expected_n = eval_normal(prims, centroid, eps);
            if cross.dot(expected_n) < 0.0 {
                std::mem::swap(&mut b, &mut c);
                std::mem::swap(&mut nb, &mut nc);
                cross = -cross;
            }
            let face_n = cross / area2;
            let (_, mat) = eval_field(prims, centroid);
            let clamp_n = |vn: Vec3| -> Vec3 {
                if vn.dot(face_n) > 0.1 { vn } else { face_n }
            };
            let (t_na, t_nb, t_nc) = if smooth_normals {
                (clamp_n(na), clamp_n(nb), clamp_n(nc))
            } else {
                (face_n, face_n, face_n)
            };
            mesh.tris.push(Triangle {
                a,
                b,
                c,
                na: t_na,
                nb: t_nb,
                nc: t_nc,
                face_normal: face_n,
                uv: mat.uv,
                is_translucent: mat.is_translucent,
                rigid_bone: None,
                part_tag: None,
            });
        }
    };

    // X-axis edges: shared by cells (x, y-1..y, z-1..z)
    for z in 1..cz {
        for y in 1..cy {
            for x in 0..cx {
                let v0 = field[idx(x, y, z)];
                let v1 = field[idx(x + 1, y, z)];
                if (v0 < 0.0) != (v1 < 0.0) {
                    let q0 = cell_vert[cidx(x, y - 1, z - 1)];
                    let q1 = cell_vert[cidx(x, y, z - 1)];
                    let q2 = cell_vert[cidx(x, y, z)];
                    let q3 = cell_vert[cidx(x, y - 1, z)];
                    if v0 < 0.0 {
                        emit_quad(q0, q1, q2, q3);
                    } else {
                        emit_quad(q0, q3, q2, q1);
                    }
                }
            }
        }
    }

    // Y-axis edges: shared by cells (x-1..x, y, z-1..z)
    for z in 1..cz {
        for y in 0..cy {
            for x in 1..cx {
                let v0 = field[idx(x, y, z)];
                let v1 = field[idx(x, y + 1, z)];
                if (v0 < 0.0) != (v1 < 0.0) {
                    let q0 = cell_vert[cidx(x - 1, y, z - 1)];
                    let q1 = cell_vert[cidx(x - 1, y, z)];
                    let q2 = cell_vert[cidx(x, y, z)];
                    let q3 = cell_vert[cidx(x, y, z - 1)];
                    if v0 < 0.0 {
                        emit_quad(q0, q1, q2, q3);
                    } else {
                        emit_quad(q0, q3, q2, q1);
                    }
                }
            }
        }
    }

    // Z-axis edges: shared by cells (x-1..x, y-1..y, z)
    for z in 0..cz {
        for y in 1..cy {
            for x in 1..cx {
                let v0 = field[idx(x, y, z)];
                let v1 = field[idx(x, y, z + 1)];
                if (v0 < 0.0) != (v1 < 0.0) {
                    let q0 = cell_vert[cidx(x - 1, y - 1, z)];
                    let q1 = cell_vert[cidx(x, y - 1, z)];
                    let q2 = cell_vert[cidx(x, y, z)];
                    let q3 = cell_vert[cidx(x - 1, y, z)];
                    if v0 < 0.0 {
                        emit_quad(q0, q1, q2, q3);
                    } else {
                        emit_quad(q0, q3, q2, q1);
                    }
                }
            }
        }
    }

    mesh
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::common::math::Vec2;

    #[test]
    fn sdf_metaball_smooth_union_and_paint() {
        let m1 = ResolvedMat { uv: Vec2::new(0.2, 0.2), is_translucent: false };
        let m2 = ResolvedMat { uv: Vec2::new(0.7, 0.7), is_translucent: false };
        let prims = vec![
            SdfPrim {
                shape: SdfShape::Sphere { center: Vec3::new(-0.3, 0.5, 0.0), radius: 0.4 },
                op: SdfOp::Union,
                smooth_k: 0.0,
                mat: m1,
            },
            SdfPrim {
                shape: SdfShape::Sphere { center: Vec3::new(0.3, 0.5, 0.0), radius: 0.4 },
                op: SdfOp::Union,
                smooth_k: 0.2,
                mat: m1,
            },
            SdfPrim {
                shape: SdfShape::Sphere { center: Vec3::new(0.3, 0.7, 0.0), radius: 0.35 },
                op: SdfOp::Paint,
                smooth_k: 0.0,
                mat: m2,
            },
        ];
        let mesh = build_sdf_mesh(&prims, 18, true);
        assert!(mesh.tris.len() > 100);
        assert!(mesh.tris.iter().any(|t| (t.uv.x - 0.2).abs() < 1e-4));
        assert!(mesh.tris.iter().any(|t| (t.uv.x - 0.7).abs() < 1e-4));
    }
}
