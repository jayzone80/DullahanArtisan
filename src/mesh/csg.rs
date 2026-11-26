/// Exact zero-dependency BSP-tree CSG boolean operations (`union`, `subtract`, `intersect`)
/// for crisp low-poly hard-surface modeling with per-triangle palette UV preservation.

use crate::common::math::{Aabb, Vec2, Vec3};
use crate::mesh::geom::{Mesh, Triangle};

const EPS: f32 = 1e-4;

#[derive(Clone, Copy)]
struct Plane {
    normal: Vec3,
    w: f32,
}

enum SplitResult {
    CoplanarFront(Polygon),
    CoplanarBack(Polygon),
    Front(Polygon),
    Back(Polygon),
    Split(Option<Polygon>, Option<Polygon>),
}

impl Plane {
    fn from_points(a: Vec3, b: Vec3, c: Vec3) -> Option<Self> {
        let n = (b - a).cross(c - a);
        let len = n.len();
        if len < 1e-8 {
            return None;
        }
        let normal = n / len;
        Some(Self { normal, w: normal.dot(a) })
    }

    fn flip(&mut self) {
        self.normal = -self.normal;
        self.w = -self.w;
    }

    fn split_polygon(&self, poly: Polygon) -> SplitResult {
        const COPLANAR: u8 = 0;
        const FRONT: u8 = 1;
        const BACK: u8 = 2;
        const SPANNING: u8 = 3;

        let mut poly_type = 0u8;
        let mut types = Vec::with_capacity(poly.verts.len());
        for &v in &poly.verts {
            let t = self.normal.dot(v) - self.w;
            let vt = if t < -EPS {
                BACK
            } else if t > EPS {
                FRONT
            } else {
                COPLANAR
            };
            poly_type |= vt;
            types.push(vt);
        }

        match poly_type {
            COPLANAR => {
                if self.normal.dot(poly.plane.normal) > 0.0 {
                    SplitResult::CoplanarFront(poly)
                } else {
                    SplitResult::CoplanarBack(poly)
                }
            }
            FRONT => SplitResult::Front(poly),
            BACK => SplitResult::Back(poly),
            _ => {
                let mut f_verts: Vec<Vec3> = Vec::with_capacity(poly.verts.len() + 1);
                let mut b_verts: Vec<Vec3> = Vec::with_capacity(poly.verts.len() + 1);
                let n = poly.verts.len();
                for i in 0..n {
                    let j = (i + 1) % n;
                    let ti = types[i];
                    let tj = types[j];
                    let vi = poly.verts[i];
                    let vj = poly.verts[j];
                    if ti != BACK {
                        push_vert(&mut f_verts, vi);
                    }
                    if ti != FRONT {
                        push_vert(&mut b_verts, vi);
                    }
                    if (ti | tj) == SPANNING {
                        let denom = self.normal.dot(vj - vi);
                        if denom.abs() > 1e-10 {
                            let t = ((self.w - self.normal.dot(vi)) / denom).clamp(0.0, 1.0);
                            let mid = vi.lerp(vj, t);
                            push_vert(&mut f_verts, mid);
                            push_vert(&mut b_verts, mid);
                        }
                    }
                }
                clean_ring(&mut f_verts);
                clean_ring(&mut b_verts);
                let fp = if f_verts.len() >= 3 {
                    Some(Polygon {
                        verts: f_verts,
                        plane: poly.plane,
                        uv: poly.uv,
                        is_translucent: poly.is_translucent,
                    })
                } else {
                    None
                };
                let bp = if b_verts.len() >= 3 {
                    Some(Polygon {
                        verts: b_verts,
                        plane: poly.plane,
                        uv: poly.uv,
                        is_translucent: poly.is_translucent,
                    })
                } else {
                    None
                };
                SplitResult::Split(fp, bp)
            }
        }
    }
}

fn push_vert(list: &mut Vec<Vec3>, v: Vec3) {
    if let Some(&last) = list.last() {
        if (v - last).len_sq() < 1e-10 {
            return;
        }
    }
    list.push(v);
}

fn clean_ring(list: &mut Vec<Vec3>) {
    while list.len() >= 2 && (list[0] - *list.last().unwrap()).len_sq() < 1e-10 {
        list.pop();
    }
}

#[derive(Clone)]
struct Polygon {
    verts: Vec<Vec3>,
    plane: Plane,
    uv: Vec2,
    is_translucent: bool,
}

impl Polygon {
    fn flip(&mut self) {
        self.verts.reverse();
        self.plane.flip();
    }
}

struct BspNode {
    plane: Option<Plane>,
    front: Option<Box<BspNode>>,
    back: Option<Box<BspNode>>,
    polygons: Vec<Polygon>,
}

impl BspNode {
    fn new() -> Self {
        Self { plane: None, front: None, back: None, polygons: Vec::new() }
    }

    fn from_polygons(polys: Vec<Polygon>) -> Self {
        let mut node = Self::new();
        node.build(polys, 0);
        node
    }

    fn invert(&mut self) {
        for p in &mut self.polygons {
            p.flip();
        }
        if let Some(ref mut pl) = self.plane {
            pl.flip();
        }
        if let Some(ref mut f) = self.front {
            f.invert();
        }
        if let Some(ref mut b) = self.back {
            b.invert();
        }
        std::mem::swap(&mut self.front, &mut self.back);
    }

    fn clip_polygons(&self, polygons: Vec<Polygon>) -> Vec<Polygon> {
        let Some(plane) = self.plane else {
            return polygons;
        };
        let mut front = Vec::new();
        let mut back = Vec::new();
        for poly in polygons {
            match plane.split_polygon(poly) {
                SplitResult::CoplanarFront(p) | SplitResult::Front(p) => front.push(p),
                SplitResult::CoplanarBack(p) | SplitResult::Back(p) => back.push(p),
                SplitResult::Split(fp, bp) => {
                    if let Some(p) = fp {
                        front.push(p);
                    }
                    if let Some(p) = bp {
                        back.push(p);
                    }
                }
            }
        }
        if let Some(ref f) = self.front {
            front = f.clip_polygons(front);
        }
        if let Some(ref b) = self.back {
            back = b.clip_polygons(back);
        } else {
            back.clear();
        }
        front.extend(back);
        front
    }

    fn clip_to(&mut self, other: &BspNode) {
        let polys = std::mem::take(&mut self.polygons);
        self.polygons = other.clip_polygons(polys);
        if let Some(ref mut f) = self.front {
            f.clip_to(other);
        }
        if let Some(ref mut b) = self.back {
            b.clip_to(other);
        }
    }

    fn all_polygons(&self, out: &mut Vec<Polygon>) {
        out.extend(self.polygons.iter().cloned());
        if let Some(ref f) = self.front {
            f.all_polygons(out);
        }
        if let Some(ref b) = self.back {
            b.all_polygons(out);
        }
    }

    fn build(&mut self, polygons: Vec<Polygon>, depth: usize) {
        if polygons.is_empty() {
            return;
        }
        if depth > 64 {
            self.polygons.extend(polygons);
            return;
        }
        if self.plane.is_none() {
            // Pick a balanced splitting plane from up to 8 evenly spaced candidates
            let chosen = choose_split_plane(&polygons);
            self.plane = Some(chosen);
        }
        let plane = self.plane.unwrap();
        let mut front = Vec::new();
        let mut back = Vec::new();
        for poly in polygons {
            match plane.split_polygon(poly) {
                SplitResult::CoplanarFront(p) | SplitResult::CoplanarBack(p) => self.polygons.push(p),
                SplitResult::Front(p) => front.push(p),
                SplitResult::Back(p) => back.push(p),
                SplitResult::Split(fp, bp) => {
                    if let Some(p) = fp {
                        front.push(p);
                    }
                    if let Some(p) = bp {
                        back.push(p);
                    }
                }
            }
        }
        if !front.is_empty() {
            self.front
                .get_or_insert_with(|| Box::new(BspNode::new()))
                .build(front, depth + 1);
        }
        if !back.is_empty() {
            self.back
                .get_or_insert_with(|| Box::new(BspNode::new()))
                .build(back, depth + 1);
        }
    }
}

fn choose_split_plane(polygons: &[Polygon]) -> Plane {
    if polygons.len() <= 4 {
        return polygons[0].plane;
    }
    let samples = polygons.len().min(8);
    let step = (polygons.len() / samples).max(1);
    let check_step = (polygons.len() / 24).max(1);
    let mut best_plane = polygons[0].plane;
    let mut best_score = i64::MAX;

    for s in 0..samples {
        let cand = polygons[(s * step) % polygons.len()].plane;
        let mut f_cnt = 0i64;
        let mut b_cnt = 0i64;
        let mut splits = 0i64;
        let mut idx = 0usize;
        while idx < polygons.len() {
            let p = &polygons[idx];
            let mut has_f = false;
            let mut has_b = false;
            for &v in &p.verts {
                let d = cand.normal.dot(v) - cand.w;
                if d > EPS {
                    has_f = true;
                } else if d < -EPS {
                    has_b = true;
                }
            }
            if has_f && has_b {
                splits += 1;
            } else if has_f {
                f_cnt += 1;
            } else if has_b {
                b_cnt += 1;
            }
            idx += check_step;
        }
        let score = (f_cnt - b_cnt).abs() + splits * 4;
        if score < best_score {
            best_score = score;
            best_plane = cand;
        }
    }
    best_plane
}

fn mesh_to_polygons(mesh: &Mesh) -> Vec<Polygon> {
    let mut polys = Vec::with_capacity(mesh.tris.len());
    for t in &mesh.tris {
        if let Some(plane) = Plane::from_points(t.a, t.b, t.c) {
            polys.push(Polygon {
                verts: vec![t.a, t.b, t.c],
                plane,
                uv: t.uv,
                is_translucent: t.is_translucent,
            });
        }
    }
    polys
}

fn polygons_to_mesh(polys: Vec<Polygon>) -> Mesh {
    let mut out = Mesh::new();
    for p in polys {
        if p.verts.len() < 3 {
            continue;
        }
        let v0 = p.verts[0];
        for i in 1..p.verts.len() - 1 {
            let mut v1 = p.verts[i];
            let mut v2 = p.verts[i + 1];
            let mut cross = (v1 - v0).cross(v2 - v0);
            let area2 = cross.len();
            if area2 < 1e-8 {
                continue;
            }
            if cross.dot(p.plane.normal) < 0.0 {
                std::mem::swap(&mut v1, &mut v2);
                cross = -cross;
            }
            let n = cross / area2;
            out.tris.push(Triangle {
                a: v0,
                b: v1,
                c: v2,
                na: n,
                nb: n,
                nc: n,
                face_normal: n,
                uv: p.uv,
                is_translucent: p.is_translucent,
                rigid_bone: None,
                part_tag: None,
            });
        }
    }
    out
}

fn aabbs_overlap(a: Aabb, b: Aabb) -> bool {
    if !a.is_valid() || !b.is_valid() {
        return false;
    }
    let pad = EPS * 2.0;
    a.min.x <= b.max.x + pad
        && a.max.x >= b.min.x - pad
        && a.min.y <= b.max.y + pad
        && a.max.y >= b.min.y - pad
        && a.min.z <= b.max.z + pad
        && a.max.z >= b.min.z - pad
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CsgOp {
    Union,
    Subtract,
    Intersect,
}

pub fn apply_csg(a_mesh: &Mesh, b_mesh: &Mesh, op: CsgOp) -> Mesh {
    if a_mesh.tris.is_empty() {
        return match op {
            CsgOp::Union => b_mesh.clone(),
            CsgOp::Subtract | CsgOp::Intersect => Mesh::new(),
        };
    }
    if b_mesh.tris.is_empty() {
        return match op {
            CsgOp::Union | CsgOp::Subtract => a_mesh.clone(),
            CsgOp::Intersect => Mesh::new(),
        };
    }

    let bb_a = a_mesh.compute_bounds();
    let bb_b = b_mesh.compute_bounds();
    if !aabbs_overlap(bb_a, bb_b) {
        return match op {
            CsgOp::Union => {
                let mut m = a_mesh.clone();
                m.tris.extend_from_slice(&b_mesh.tris);
                m
            }
            CsgOp::Subtract => a_mesh.clone(),
            CsgOp::Intersect => Mesh::new(),
        };
    }

    let mut a = BspNode::from_polygons(mesh_to_polygons(a_mesh));
    let mut b = BspNode::from_polygons(mesh_to_polygons(b_mesh));

    match op {
        CsgOp::Union => {
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let mut bp = Vec::new();
            b.all_polygons(&mut bp);
            a.build(bp, 0);
        }
        CsgOp::Subtract => {
            a.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            b.invert();
            b.clip_to(&a);
            b.invert();
            let mut bp = Vec::new();
            b.all_polygons(&mut bp);
            a.build(bp, 0);
            a.invert();
        }
        CsgOp::Intersect => {
            a.invert();
            b.clip_to(&a);
            b.invert();
            a.clip_to(&b);
            b.clip_to(&a);
            let mut bp = Vec::new();
            b.all_polygons(&mut bp);
            a.build(bp, 0);
            a.invert();
        }
    }

    let mut out_polys = Vec::new();
    a.all_polygons(&mut out_polys);
    polygons_to_mesh(out_polys)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::palette::ResolvedMat;
    use crate::mesh::primitives::{build_box, FaceMats};

    #[test]
    fn csg_subtract_preserves_cutter_material() {
        let mat_a = ResolvedMat { uv: Vec2::new(0.1, 0.1), is_translucent: false };
        let mat_b = ResolvedMat { uv: Vec2::new(0.8, 0.8), is_translucent: false };
        let mut a = build_box(Vec3::new(2.0, 2.0, 2.0), Vec2::new(1.0, 1.0), Vec2::ZERO, 0.0, mat_a, &FaceMats::default());
        let mut b = build_box(Vec3::new(1.0, 1.0, 3.0), Vec2::new(1.0, 1.0), Vec2::ZERO, 0.0, mat_b, &FaceMats::default());
        a.translate(Vec3::new(0.0, 1.0, 0.0));
        b.translate(Vec3::new(0.0, 1.2, 0.0));

        let res = apply_csg(&a, &b, CsgOp::Subtract);
        assert!(res.tris.len() > 12);
        let has_a = res.tris.iter().any(|t| (t.uv.x - 0.1).abs() < 1e-4);
        let has_b = res.tris.iter().any(|t| (t.uv.x - 0.8).abs() < 1e-4);
        assert!(has_a && has_b, "expected both outer body and carved inner cutter materials");
    }
}
