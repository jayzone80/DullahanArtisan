use std::collections::HashMap;

use crate::common::math::{Aabb, Mat4, Vec2, Vec3};
use crate::mesh::anim::AnimAction;
use crate::mesh::morph::{apply_morph_targets, MorphTarget};
use crate::mesh::palette::ResolvedMat;
use crate::mesh::skel::Bone;

#[derive(Debug, Clone)]
pub struct TexMaterial {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub tile_w: f32,
    pub tile_h: f32,
    pub basecolor_rgba: Vec<u8>,
    pub normal_rgb: Option<Vec<u8>>,
    pub orm_rgb: Option<Vec<u8>>,
    pub basecolor_png: Vec<u8>,
    pub normal_png: Option<Vec<u8>>,
    pub orm_png: Option<Vec<u8>>,
}

impl TexMaterial {
    #[inline]
    fn bilinear_coords(&self, u: f32, v: f32) -> (usize, usize, usize, usize, f32, f32) {
        let w = self.width.max(1) as usize;
        let h = self.height.max(1) as usize;
        let fx = u.rem_euclid(1.0) * w as f32 - 0.5;
        let fy = v.rem_euclid(1.0) * h as f32 - 0.5;
        let x0i = fx.floor() as isize;
        let y0i = fy.floor() as isize;
        let tx = fx - x0i as f32;
        let ty = fy - y0i as f32;
        let x0 = x0i.rem_euclid(w as isize) as usize;
        let x1 = (x0i + 1).rem_euclid(w as isize) as usize;
        let y0 = y0i.rem_euclid(h as isize) as usize;
        let y1 = (y0i + 1).rem_euclid(h as isize) as usize;
        (x0, x1, y0, y1, tx, ty)
    }

    pub fn sample_basecolor_bilinear(&self, u: f32, v: f32) -> [f32; 4] {
        let w = self.width.max(1) as usize;
        let (x0, x1, y0, y1, tx, ty) = self.bilinear_coords(u, v);
        let px = |x: usize, y: usize| {
            let p = (y * w + x) * 4;
            [
                self.basecolor_rgba[p] as f32 / 255.0,
                self.basecolor_rgba[p + 1] as f32 / 255.0,
                self.basecolor_rgba[p + 2] as f32 / 255.0,
                self.basecolor_rgba[p + 3] as f32 / 255.0,
            ]
        };
        let c00 = px(x0, y0);
        let c10 = px(x1, y0);
        let c01 = px(x0, y1);
        let c11 = px(x1, y1);
        let mut out = [0.0f32; 4];
        for k in 0..4 {
            let a = c00[k] + (c10[k] - c00[k]) * tx;
            let b = c01[k] + (c11[k] - c01[k]) * tx;
            out[k] = a + (b - a) * ty;
        }
        out
    }

    pub fn sample_orm_bilinear(&self, u: f32, v: f32) -> [f32; 3] {
        let Some(ref orm) = self.orm_rgb else {
            return [1.0, 0.85, 0.0];
        };
        let w = self.width.max(1) as usize;
        let (x0, x1, y0, y1, tx, ty) = self.bilinear_coords(u, v);
        let px = |x: usize, y: usize| {
            let p = (y * w + x) * 3;
            [
                orm[p] as f32 / 255.0,
                orm[p + 1] as f32 / 255.0,
                orm[p + 2] as f32 / 255.0,
            ]
        };
        let c00 = px(x0, y0);
        let c10 = px(x1, y0);
        let c01 = px(x0, y1);
        let c11 = px(x1, y1);
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let a = c00[k] + (c10[k] - c00[k]) * tx;
            let b = c01[k] + (c11[k] - c01[k]) * tx;
            out[k] = a + (b - a) * ty;
        }
        out
    }

    pub fn sample_normal_bilinear(&self, u: f32, v: f32) -> Option<Vec3> {
        let nrm = self.normal_rgb.as_ref()?;
        let w = self.width.max(1) as usize;
        let (x0, x1, y0, y1, tx, ty) = self.bilinear_coords(u, v);
        let px = |x: usize, y: usize| {
            let p = (y * w + x) * 3;
            [
                nrm[p] as f32 / 255.0 * 2.0 - 1.0,
                nrm[p + 1] as f32 / 255.0 * 2.0 - 1.0,
                nrm[p + 2] as f32 / 255.0 * 2.0 - 1.0,
            ]
        };
        let c00 = px(x0, y0);
        let c10 = px(x1, y0);
        let c01 = px(x0, y1);
        let c11 = px(x1, y1);
        let mut out = [0.0f32; 3];
        for k in 0..3 {
            let a = c00[k] + (c10[k] - c00[k]) * tx;
            let b = c01[k] + (c11[k] - c01[k]) * tx;
            out[k] = a + (b - a) * ty;
        }
        Some(Vec3::new(out[0], out[1], out[2].max(0.05)).normalized())
    }
}

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
    pub uv_b: Vec2,
    pub uv_c: Vec2,
    pub is_translucent: bool,
    /// Optional external/baked PBR texture material index into `Mesh.textures` (`None` = shared palette atlas).
    pub tex_id: Option<u16>,
    /// Optional explicit rigid bone index (`bone=<name>`); `None` uses automatic smooth skinning when rigged.
    pub rigid_bone: Option<u16>,
    /// Optional `@part` tag index into `Mesh.tag_names` (used by `morph` blend-shape targeting).
    pub part_tag: Option<u16>,
}

impl Triangle {
    /// Compute tangent vector + glTF handedness sign (`+1` or `-1`) for normal mapping.
    /// Tangent points along +U, bitangent `sign * (N x T)` points along +V (downward in PNG, matching OpenGL +Y normal maps via `-B`).
    pub fn compute_tangent(&self, n: Vec3) -> (Vec3, f32) {
        let e1 = self.b - self.a;
        let e2 = self.c - self.a;
        let du1 = self.uv_b.x - self.uv.x;
        let dv1 = self.uv_b.y - self.uv.y;
        let du2 = self.uv_c.x - self.uv.x;
        let dv2 = self.uv_c.y - self.uv.y;
        let det = du1 * dv2 - du2 * dv1;
        if det.abs() > 1e-8 {
            let inv = 1.0 / det;
            let t_raw = (e1 * dv2 - e2 * dv1) * inv;
            let b_raw = (e2 * du1 - e1 * du2) * inv;
            // Gram-Schmidt orthogonalize tangent against vertex normal `n`
            let t_orth = t_raw - n * n.dot(t_raw);
            if t_orth.len_sq() > 1e-10 {
                let t = t_orth.normalized();
                // OpenGL/MikkTSpace tangent frame: B = sign * (N x T) aligns with -V (+Y up in PNG normal map)
                let sign = if n.cross(t).dot(-b_raw) < 0.0 { -1.0 } else { 1.0 };
                return (t, sign);
            }
        }
        // Fallback orthonormal tangent for degenerate/constant UVs
        let ref_v = if n.y.abs() < 0.9 { Vec3::Y } else { Vec3::X };
        let t = ref_v.cross(n).normalized();
        (t, 1.0)
    }
}

#[derive(Debug, Clone)]
pub struct ColliderMesh {
    pub name: String,
    pub tris: Vec<Triangle>,
}

#[derive(Debug, Clone)]
pub struct ChildNodeMesh {
    pub name: String,
    pub translation: Vec3,
    pub rotation_deg: Vec3,
    pub scale: Vec3,
    pub tris: Vec<Triangle>,
    pub children: Vec<ChildNodeMesh>,
}

#[derive(Debug, Clone, Default)]
pub struct Mesh {
    pub tris: Vec<Triangle>,
    /// Optional bounding boxes keyed by `@id` tags for `--focus @id` close-up renders.
    pub part_bounds: HashMap<String, Aabb>,
    /// Optional separate collider sub-meshes exported as named child glTF nodes (e.g. `hull-convcolonly`, `UBX_crate_01`).
    pub colliders: Vec<ColliderMesh>,
    /// Optional named visual child nodes with local pivots for animated sub-parts (e.g. `LeverPivot`, `TargetMast`).
    pub nodes: Vec<ChildNodeMesh>,
    /// Optional keyframe animation actions (`anim`/`action` blocks) exported to glTF 2.0 `animations`.
    pub actions: Vec<AnimAction>,
    /// Optional skeletal rig bones (`skel ... end`) exported to glTF 2.0 `skins`.
    pub bones: Vec<Bone>,
    /// Optional morph targets / blend shapes (`morph ... end`) exported to glTF 2.0 `primitive.targets`.
    pub morphs: Vec<MorphTarget>,
    /// Interned `@id` tag names indexed by `Triangle.part_tag`.
    pub tag_names: Vec<String>,
    /// Optional baked/external PBR texture materials (`tex <name> <path>`) indexed by `Triangle.tex_id`.
    pub textures: Vec<TexMaterial>,
}

impl Mesh {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn intern_tag(&mut self, tag: &str) -> u16 {
        if let Some(i) = self.tag_names.iter().position(|s| s == tag) {
            return i as u16;
        }
        let id = self.tag_names.len() as u16;
        self.tag_names.push(tag.to_string());
        id
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
            uv_b: mat.uv,
            uv_c: mat.uv,
            is_translucent: mat.is_translucent,
            tex_id: None,
            rigid_bone: None,
            part_tag: None,
        });
    }

    pub fn add_tri_uv(
        &mut self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        ua: Vec2,
        ub: Vec2,
        uc: Vec2,
        tex_id: Option<u16>,
        is_translucent: bool,
    ) {
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
            uv: ua,
            uv_b: ub,
            uv_c: uc,
            is_translucent,
            tex_id,
            rigid_bone: None,
            part_tag: None,
        });
    }

    /// Outward-CCW quad split into (a, b, c) and (a, c, d).
    pub fn add_quad(&mut self, a: Vec3, b: Vec3, c: Vec3, d: Vec3, mat: ResolvedMat) {
        self.add_tri(a, b, c, mat);
        self.add_tri(a, c, d, mat);
    }

    /// Outward-CCW quad with explicit per-vertex UVs `(ua, ub, uc, ud)`.
    pub fn add_quad_uv(
        &mut self,
        a: Vec3,
        b: Vec3,
        c: Vec3,
        d: Vec3,
        ua: Vec2,
        ub: Vec2,
        uc: Vec2,
        ud: Vec2,
        tex_id: Option<u16>,
        is_translucent: bool,
    ) {
        self.add_tri_uv(a, b, c, ua, ub, uc, tex_id, is_translucent);
        self.add_tri_uv(a, c, d, ua, uc, ud, tex_id, is_translucent);
    }

    pub fn compute_bounds(&self) -> Aabb {
        let mut bb = Aabb::empty();
        for t in &self.tris {
            bb.expand(t.a);
            bb.expand(t.b);
            bb.expand(t.c);
        }
        fn expand_node(node: &ChildNodeMesh, parent: Mat4, bb: &mut Aabb) {
            let mat = parent.mul(Mat4::from_trs(node.translation, node.rotation_deg, node.scale));
            for t in &node.tris {
                bb.expand(mat.transform_point(t.a));
                bb.expand(mat.transform_point(t.b));
                bb.expand(mat.transform_point(t.c));
            }
            for ch in &node.children {
                expand_node(ch, mat, bb);
            }
        }
        for node in &self.nodes {
            expand_node(node, Mat4::IDENTITY, &mut bb);
        }
        bb
    }

    /// Total visual triangle count across root mesh and all named visual child nodes (recursively).
    pub fn total_visual_tris(&self) -> usize {
        fn count_node(n: &ChildNodeMesh) -> usize {
            n.tris.len() + n.children.iter().map(count_node).sum::<usize>()
        }
        self.tris.len() + self.nodes.iter().map(count_node).sum::<usize>()
    }

    /// Total visual child node count (recursively).
    pub fn total_node_count(&self) -> usize {
        fn count_n(n: &ChildNodeMesh) -> usize {
            1 + n.children.iter().map(count_n).sum::<usize>()
        }
        self.nodes.iter().map(count_n).sum()
    }

    /// Flatten root geometry + default morph weights + all transformed visual child nodes (for software rendering).
    pub fn flattened_visual(&self) -> Mesh {
        let base_tris = if self.morphs.is_empty() {
            self.tris.clone()
        } else {
            let weights: Vec<f32> = self.morphs.iter().map(|m| m.default_weight).collect();
            apply_morph_targets(&self.tris, &self.morphs, &weights)
        };
        if self.nodes.is_empty() {
            let mut out = self.clone();
            out.tris = base_tris;
            return out;
        }
        let mut out = Mesh {
            tris: base_tris,
            part_bounds: self.part_bounds.clone(),
            colliders: self.colliders.clone(),
            nodes: Vec::new(),
            actions: Vec::new(),
            bones: self.bones.clone(),
            morphs: Vec::new(),
            tag_names: Vec::new(),
            textures: self.textures.clone(),
        };
        fn append_node(node: &ChildNodeMesh, parent: Mat4, out: &mut Mesh) {
            let mat = parent.mul(Mat4::from_trs(node.translation, node.rotation_deg, node.scale));
            if !node.tris.is_empty() {
                let sub = Mesh {
                    tris: node.tris.clone(),
                    ..Mesh::default()
                };
                out.append_transformed(&sub, mat, Some(&node.name));
            }
            for ch in &node.children {
                append_node(ch, mat, out);
            }
        }
        for node in &self.nodes {
            append_node(node, Mat4::IDENTITY, &mut out);
        }
        out
    }

    pub fn translate(&mut self, delta: Vec3) {
        for t in &mut self.tris {
            t.a = t.a + delta;
            t.b = t.b + delta;
            t.c = t.c + delta;
        }
    }

    /// Apply crease-angle-aware, area-weighted smooth vertex normals across shared positions within this local shape.
    /// Preserves sharp seams across different palette cells/UVs and across creases sharper than ~75.5 deg (`cos < 0.25`),
    /// which keeps flat cylinder caps crisp and mathematically prevents inverted vertex normals on beveled frustums.
    pub fn compute_smooth_normals(&mut self) {
        const COS_CREASE: f32 = 0.25; // ~75.5 deg: smooths seg>=5 cyls & 45 deg bevels, keeps 90+ deg caps/creases crisp
        let quant = |p: Vec3, t: &Triangle| -> (i32, i32, i32, i32, i32) {
            let (k0, k1) = match t.tex_id {
                Some(id) => (-(id as i32) - 1, -1),
                None => ((t.uv.x * 10000.0).round() as i32, (t.uv.y * 10000.0).round() as i32),
            };
            (
                (p.x * 10000.0).round() as i32,
                (p.y * 10000.0).round() as i32,
                (p.z * 10000.0).round() as i32,
                k0,
                k1,
            )
        };
        let mut buckets: HashMap<(i32, i32, i32, i32, i32), Vec<(Vec3, Vec3)>> = HashMap::new();
        for t in &self.tris {
            let weighted = (t.b - t.a).cross(t.c - t.a); // magnitude = 2 * area
            let fn_u = weighted.normalized();
            if fn_u.len_sq() < 0.5 {
                continue;
            }
            for p in [t.a, t.b, t.c] {
                buckets.entry(quant(p, t)).or_default().push((weighted, fn_u));
            }
        }
        for t in &mut self.tris {
            let fn_u = (t.b - t.a).cross(t.c - t.a).normalized();
            let face_n = if fn_u.len_sq() > 0.5 { fn_u } else { t.face_normal };
            t.face_normal = face_n;

            let smooth_at = |p: Vec3, t_ref: &Triangle| -> Vec3 {
                let k = quant(p, t_ref);
                let mut sum = Vec3::ZERO;
                if let Some(entries) = buckets.get(&k) {
                    for &(w, n_other) in entries {
                        if n_other.dot(face_n) >= COS_CREASE {
                            sum = sum + w;
                        }
                    }
                }
                let n = sum.normalized();
                if n.len_sq() > 0.5 && n.dot(face_n) > 0.0 {
                    n
                } else {
                    face_n
                }
            };

            let t_copy = *t;
            t.na = smooth_at(t.a, &t_copy);
            t.nb = smooth_at(t.b, &t_copy);
            t.nc = smooth_at(t.c, &t_copy);
        }
    }

    /// Apply Blender-style Face-Weighted Normals (`wn=1`, default `area_power = 4.0`).
    /// Computes each coplanar polygon face's true total area (eliminating quad-diagonal and ear-clipping bias),
    /// weights each face plane by `face_area^area_power`, and respects the ~75.5 deg crease threshold so
    /// large flat panels stay dead-flat while 1-segment bevels interpolate smoothly with zero inverted normals.
    pub fn compute_weighted_normals(&mut self, area_power: f32) {
        const COS_CREASE: f32 = 0.25;
        let power = area_power.clamp(1.0, 12.0);

        let quant_v = |p: Vec3, uv: Vec2| -> (i32, i32, i32, i32, i32) {
            (
                (p.x * 10000.0).round() as i32,
                (p.y * 10000.0).round() as i32,
                (p.z * 10000.0).round() as i32,
                (uv.x * 10000.0).round() as i32,
                (uv.y * 10000.0).round() as i32,
            )
        };
        let quant_plane = |n: Vec3, p: Vec3, uv: Vec2| -> (i32, i32, i32, i32, i32, i32) {
            let d = n.dot(p);
            (
                (n.x * 500.0).round() as i32,
                (n.y * 500.0).round() as i32,
                (n.z * 500.0).round() as i32,
                (d * 1000.0).round() as i32,
                (uv.x * 10000.0).round() as i32,
                (uv.y * 10000.0).round() as i32,
            )
        };

        // Pass 1: Total polygon area per coplanar face plane (so quad diagonals never bias corners)
        let mut plane_areas: HashMap<(i32, i32, i32, i32, i32, i32), f32> = HashMap::new();
        let mut tri_planes = Vec::with_capacity(self.tris.len());
        for t in &self.tris {
            let cross = (t.b - t.a).cross(t.c - t.a);
            let area = cross.len() * 0.5;
            let fn_u = cross.normalized();
            if area < 1e-10 || fn_u.len_sq() < 0.5 {
                tri_planes.push(None);
                continue;
            }
            let pk = quant_plane(fn_u, t.a, t.uv);
            *plane_areas.entry(pk).or_insert(0.0) += area;
            tri_planes.push(Some((pk, fn_u)));
        }

        // Pass 2: Collect unique face planes touching each vertex (deduped by plane key)
        let mut v_planes: HashMap<(i32, i32, i32, i32, i32), Vec<((i32, i32, i32, i32, i32, i32), Vec3, f32)>> =
            HashMap::new();
        for (ti, t) in self.tris.iter().enumerate() {
            let Some((pk, fn_u)) = tri_planes[ti] else {
                continue;
            };
            let p_area = plane_areas.get(&pk).copied().unwrap_or(0.0);
            for p in [t.a, t.b, t.c] {
                let vk = quant_v(p, t.uv);
                let list = v_planes.entry(vk).or_default();
                if !list.iter().any(|&(existing_pk, _, _)| existing_pk == pk) {
                    list.push((pk, fn_u, p_area));
                }
            }
        }

        // Pass 3: Compute area^power weighted normal per vertex (normalized against max local area to avoid underflow)
        for (ti, t) in self.tris.iter_mut().enumerate() {
            let Some((_, face_n)) = tri_planes[ti] else {
                continue;
            };
            t.face_normal = face_n;

            let weighted_at = |p: Vec3| -> Vec3 {
                let vk = quant_v(p, t.uv);
                let Some(planes) = v_planes.get(&vk) else {
                    return face_n;
                };
                let mut max_a = 0.0f32;
                for &(_, n_other, a) in planes {
                    if n_other.dot(face_n) >= COS_CREASE && a > max_a {
                        max_a = a;
                    }
                }
                if max_a < 1e-12 {
                    return face_n;
                }
                let mut sum = Vec3::ZERO;
                for &(_, n_other, a) in planes {
                    if n_other.dot(face_n) >= COS_CREASE {
                        let w = (a / max_a).powf(power);
                        sum = sum + n_other * w;
                    }
                }
                let n = sum.normalized();
                if n.len_sq() > 0.5 && n.dot(face_n) > 0.0 {
                    n
                } else {
                    face_n
                }
            };

            t.na = weighted_at(t.a);
            t.nb = weighted_at(t.b);
            t.nc = weighted_at(t.c);
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
                // Split polygon (triangle) into below-plane and above-plane convex pieces (preserving CCW winding & UVs)
                let uvs = [t.uv, t.uv_b, t.uv_c];
                let mut below: Vec<(Vec3, Vec3, Vec2)> = Vec::with_capacity(4);
                let mut above: Vec<(Vec3, Vec3, Vec2)> = Vec::with_capacity(4);
                for i in 0..3 {
                    let j = (i + 1) % 3;
                    let (p0, n0) = verts[i];
                    let (p1, n1) = verts[j];
                    let uv0 = uvs[i];
                    let uv1 = uvs[j];
                    let d0 = d[i];
                    let d1 = d[j];
                    if d0 <= 1e-6 {
                        below.push((p0, n0, uv0));
                    }
                    if d0 >= -1e-6 {
                        above.push((p0, n0, uv0));
                    }
                    if (d0 < -1e-6 && d1 > 1e-6) || (d0 > 1e-6 && d1 < -1e-6) {
                        let u = d0 / (d0 - d1);
                        let pm = p0.lerp(p1, u);
                        let nm = n0.lerp(n1, u).normalized();
                        let uvm = Vec2::new(uv0.x + (uv1.x - uv0.x) * u, uv0.y + (uv1.y - uv0.y) * u);
                        below.push((pm, nm, uvm));
                        above.push((pm, nm, uvm));
                    }
                }
                for poly in [&below, &above] {
                    if poly.len() >= 3 {
                        for m in 1..(poly.len() - 1) {
                            let (pa, na, uva) = poly[0];
                            let (pb, nb, uvb) = poly[m];
                            let (pc, nc, uvc) = poly[m + 1];
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
                                    uv: uva,
                                    uv_b: uvb,
                                    uv_c: uvc,
                                    is_translucent: t.is_translucent,
                                    tex_id: t.tex_id,
                                    rigid_bone: t.rigid_bone,
                                    part_tag: t.part_tag,
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
        let outer_tag_id = tag.map(|s| self.intern_tag(s));
        // Remap inner part tags from `src` into `self.tag_names`
        let inner_tag_remap: Vec<u16> = src.tag_names.iter().map(|s| self.intern_tag(s)).collect();

        for t in &src.tris {
            let pa = mat.transform_point(t.a);
            let pb = mat.transform_point(t.b);
            let pc = mat.transform_point(t.c);

            let (a, b, c, na, nb, nc, ua, ub, uc) = if flip {
                (
                    pa,
                    pc,
                    pb,
                    mat.transform_normal(t.na),
                    mat.transform_normal(t.nc),
                    mat.transform_normal(t.nb),
                    t.uv,
                    t.uv_c,
                    t.uv_b,
                )
            } else {
                (
                    pa,
                    pb,
                    pc,
                    mat.transform_normal(t.na),
                    mat.transform_normal(t.nb),
                    mat.transform_normal(t.nc),
                    t.uv,
                    t.uv_b,
                    t.uv_c,
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
                uv: ua,
                uv_b: ub,
                uv_c: uc,
                is_translucent: t.is_translucent,
                tex_id: t.tex_id,
                rigid_bone: t.rigid_bone,
                part_tag: t
                    .part_tag
                    .and_then(|id| inner_tag_remap.get(id as usize).copied())
                    .or(outer_tag_id),
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
