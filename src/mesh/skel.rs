use std::collections::HashMap;

use crate::common::math::{Mat4, Vec3};
use crate::mesh::anim::{eval_vec3_track, AnimAction, InterpMode, TrackKey};
use crate::mesh::geom::{Mesh, Triangle};

#[derive(Debug, Clone)]
pub struct Bone {
    pub name: String,
    /// Parent bone index in `Mesh.bones` (`None` = root bone).
    pub parent: Option<usize>,
    /// World-space rest head (pivot) position.
    pub head: Vec3,
    /// World-space rest tail position (defines bone direction & length).
    pub tail: Vec3,
    /// Capsule influence radius for automatic smooth skinning.
    pub radius: f32,
    /// Distance falloff exponent for automatic smooth skinning (default 3.0).
    pub falloff: f32,
}

impl Bone {
    pub fn length(&self) -> f32 {
        (self.tail - self.head).len().max(1e-4)
    }

    pub fn rest_local_translation(&self, bones: &[Bone]) -> Vec3 {
        match self.parent {
            Some(pi) => self.head - bones[pi].head,
            None => self.head,
        }
    }
}

#[derive(Debug, Clone)]
pub struct IkKeyframe {
    pub time: f32,
    /// Effector bone name (by default its head hits `target`; if `use_tip` or chain reaches root, its tail hits `target`).
    pub effector: String,
    pub target: Vec3,
    pub pole: Option<Vec3>,
    pub chain_len: usize,
    pub use_tip: bool,
    pub interp: InterpMode,
}

#[derive(Debug, Clone, Copy)]
pub struct VertexSkin {
    pub joints: [u16; 4],
    pub weights: [f32; 4],
}

impl Default for VertexSkin {
    fn default() -> Self {
        Self {
            joints: [0, 0, 0, 0],
            weights: [1.0, 0.0, 0.0, 0.0],
        }
    }
}

/// Unit quaternion `[x, y, z, w]` utilities shared across rigging, IK, and glTF export.
#[derive(Debug, Clone, Copy)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Self = Self { x: 0.0, y: 0.0, z: 0.0, w: 1.0 };

    #[inline]
    pub fn array(self) -> [f32; 4] {
        [self.x, self.y, self.z, self.w]
    }

    #[inline]
    pub fn normalized(self) -> Self {
        let l = (self.x * self.x + self.y * self.y + self.z * self.z + self.w * self.w).sqrt();
        if l > 1e-12 {
            Self { x: self.x / l, y: self.y / l, z: self.z / l, w: self.w / l }
        } else {
            Self::IDENTITY
        }
    }

    #[inline]
    pub fn conjugate(self) -> Self {
        Self { x: -self.x, y: -self.y, z: -self.z, w: self.w }
    }

    #[inline]
    pub fn mul(self, b: Self) -> Self {
        Self {
            x: self.w * b.x + self.x * b.w + self.y * b.z - self.z * b.y,
            y: self.w * b.y - self.x * b.z + self.y * b.w + self.z * b.x,
            z: self.w * b.z + self.x * b.y - self.y * b.x + self.z * b.w,
            w: self.w * b.w - self.x * b.x - self.y * b.y - self.z * b.z,
        }
    }

    #[inline]
    pub fn dot(self, b: Self) -> f32 {
        self.x * b.x + self.y * b.y + self.z * b.z + self.w * b.w
    }

    #[inline]
    pub fn rotate_vec3(self, v: Vec3) -> Vec3 {
        let qv = Vec3::new(self.x, self.y, self.z);
        let t = qv.cross(v) * 2.0;
        v + t * self.w + qv.cross(t)
    }

    pub fn from_euler_xyz_deg(rot: Vec3) -> Self {
        let (sx, cx) = (rot.x.to_radians() * 0.5).sin_cos();
        let (sy, cy) = (rot.y.to_radians() * 0.5).sin_cos();
        let (sz, cz) = (rot.z.to_radians() * 0.5).sin_cos();
        let qx = Self { x: sx, y: 0.0, z: 0.0, w: cx };
        let qy = Self { x: 0.0, y: sy, z: 0.0, w: cy };
        let qz = Self { x: 0.0, y: 0.0, z: sz, w: cz };
        qz.mul(qy).mul(qx).normalized()
    }

    pub fn from_axis_angle(axis: Vec3, rad: f32) -> Self {
        let a = axis.normalized();
        if a.len_sq() < 0.5 {
            return Self::IDENTITY;
        }
        let (s, c) = (rad * 0.5).sin_cos();
        Self { x: a.x * s, y: a.y * s, z: a.z * s, w: c }.normalized()
    }

    /// Shortest-arc quaternion rotating unit vector `from` to unit vector `to`.
    pub fn from_to(from: Vec3, to: Vec3) -> Self {
        let f = from.normalized();
        let t = to.normalized();
        if f.len_sq() < 0.5 || t.len_sq() < 0.5 {
            return Self::IDENTITY;
        }
        let d = f.dot(t).clamp(-1.0, 1.0);
        if d > 0.99999 {
            return Self::IDENTITY;
        }
        if d < -0.99999 {
            // 180-degree flip around an arbitrary orthogonal axis
            let ortho = if f.x.abs() < 0.8 { Vec3::X } else { Vec3::Y };
            let axis = f.cross(ortho).normalized();
            return Self { x: axis.x, y: axis.y, z: axis.z, w: 0.0 };
        }
        let c = f.cross(t);
        Self { x: c.x, y: c.y, z: c.z, w: 1.0 + d }.normalized()
    }

    pub fn to_mat4(self, translation: Vec3, scale: Vec3) -> Mat4 {
        let q = self.normalized();
        let (x, y, z, w) = (q.x, q.y, q.z, q.w);
        let xx = x * x;
        let yy = y * y;
        let zz = z * z;
        let xy = x * y;
        let xz = x * z;
        let yz = y * z;
        let wx = w * x;
        let wy = w * y;
        let wz = w * z;

        Mat4 {
            m: [
                [(1.0 - 2.0 * (yy + zz)) * scale.x, (2.0 * (xy + wz)) * scale.x, (2.0 * (xz - wy)) * scale.x, 0.0],
                [(2.0 * (xy - wz)) * scale.y, (1.0 - 2.0 * (xx + zz)) * scale.y, (2.0 * (yz + wx)) * scale.y, 0.0],
                [(2.0 * (xz + wy)) * scale.z, (2.0 * (yz - wx)) * scale.z, (1.0 - 2.0 * (xx + yy)) * scale.z, 0.0],
                [translation.x, translation.y, translation.z, 1.0],
            ],
        }
    }
}

/// Mirror common bone naming conventions (`_L` <-> `_R`, `.L` <-> `.R`, `Left` <-> `Right`).
pub fn mirror_bone_name(name: &str) -> String {
    if let Some(stem) = name.strip_suffix("_L") {
        return format!("{stem}_R");
    }
    if let Some(stem) = name.strip_suffix("_R") {
        return format!("{stem}_L");
    }
    if let Some(stem) = name.strip_suffix("_l") {
        return format!("{stem}_r");
    }
    if let Some(stem) = name.strip_suffix("_r") {
        return format!("{stem}_l");
    }
    if let Some(stem) = name.strip_suffix(".L") {
        return format!("{stem}.R");
    }
    if let Some(stem) = name.strip_suffix(".R") {
        return format!("{stem}.L");
    }
    if name.contains("Left") {
        return name.replacen("Left", "Right", 1);
    }
    if name.contains("Right") {
        return name.replacen("Right", "Left", 1);
    }
    format!("{name}_R")
}

/// Detect bone handedness (`-1` = right / -X, `+1` = left / +X, `0` = center).
fn bone_side_hint(b: &Bone) -> i8 {
    let mid_x = 0.5 * (b.head.x + b.tail.x);
    let nl = b.name.to_ascii_lowercase();
    let is_l = nl.ends_with("_l") || nl.ends_with(".l") || nl.contains("left");
    let is_r = nl.ends_with("_r") || nl.ends_with(".r") || nl.contains("right");
    if is_l || mid_x > 0.04 {
        1
    } else if is_r || mid_x < -0.04 {
        -1
    } else {
        0
    }
}

/// Distance from point `p` to 3D line segment `a -> b`.
fn point_to_segment_dist(p: Vec3, a: Vec3, b: Vec3) -> f32 {
    let ab = b - a;
    let l2 = ab.len_sq();
    if l2 < 1e-10 {
        return (p - a).len();
    }
    let t = ((p - a).dot(ab) / l2).clamp(0.0, 1.0);
    (p - (a + ab * t)).len()
}

/// Compute per-triangle-vertex skin weights (`[VertexSkin; 3]` per triangle) using:
/// 1) Explicit per-part rigid `bone=` binding when specified on a triangle (`rigid_bone != None`),
/// 2) Automatic capsule-distance falloff + L/R symmetry guard + 3-pass mesh-connectivity Laplacian smoothing!
pub fn compute_auto_skinning(tris: &[Triangle], bones: &[Bone]) -> Vec<[VertexSkin; 3]> {
    if bones.is_empty() || tris.is_empty() {
        return vec![[VertexSkin::default(); 3]; tris.len()];
    }

    let quant = |p: Vec3| -> (i32, i32, i32) {
        (
            (p.x * 5000.0).round() as i32,
            (p.y * 5000.0).round() as i32,
            (p.z * 5000.0).round() as i32,
        )
    };

    // Map unique auto-skinned vertices -> index, and build edge adjacency for topology-aware smoothing
    let mut v_idx_map: HashMap<(i32, i32, i32), usize> = HashMap::new();
    let mut v_pos: Vec<Vec3> = Vec::new();
    let mut tri_v_idx: Vec<[usize; 3]> = Vec::with_capacity(tris.len());
    let mut v_rigid: Vec<Option<u16>> = Vec::new();

    for t in tris {
        let mut idxs = [0usize; 3];
        for (ci, p) in [t.a, t.b, t.c].iter().enumerate() {
            let k = quant(*p);
            let idx = *v_idx_map.entry(k).or_insert_with(|| {
                let id = v_pos.len();
                v_pos.push(*p);
                v_rigid.push(t.rigid_bone);
                id
            });
            // If any triangle sharing this vertex is auto-skinned while another is rigid, keep rigid only if all rigid
            if t.rigid_bone.is_none() {
                v_rigid[idx] = None;
            }
            idxs[ci] = idx;
        }
        tri_v_idx.push(idxs);
    }

    let nv = v_pos.len();
    let nb = bones.len();
    let mut adj: Vec<Vec<usize>> = vec![Vec::new(); nv];
    for &[a, b, c] in &tri_v_idx {
        for (u, v) in [(a, b), (b, c), (c, a)] {
            if u != v {
                if !adj[u].contains(&v) {
                    adj[u].push(v);
                }
                if !adj[v].contains(&u) {
                    adj[v].push(u);
                }
            }
        }
    }

    let sides: Vec<i8> = bones.iter().map(bone_side_hint).collect();

    // Initial capsule-distance weights (dense over nb bones per vertex for clean smoothing)
    let mut weights: Vec<Vec<f32>> = vec![vec![0.0; nb]; nv];
    for vi in 0..nv {
        if let Some(rb) = v_rigid[vi] {
            if (rb as usize) < nb {
                weights[vi][rb as usize] = 1.0;
                continue;
            }
        }
        let p = v_pos[vi];
        let mut sum = 0.0f32;
        for (bi, b) in bones.iter().enumerate() {
            let d = point_to_segment_dist(p, b.head, b.tail);
            let r = b.radius.max(0.03);
            let falloff = b.falloff.clamp(1.2, 8.0);
            // Cross-symmetry penalty prevents left-leg / right-leg bleed on close limbs
            let side_pen = if (sides[bi] > 0 && p.x < -0.025) || (sides[bi] < 0 && p.x > 0.025) {
                0.04
            } else {
                1.0
            };
            let dn = (d / r).max(0.06);
            let w = side_pen / (dn.powf(falloff) * (1.0 + (d / (r * 2.4)).powi(4)));
            weights[vi][bi] = w;
            sum += w;
        }
        if sum > 1e-12 {
            for w in &mut weights[vi] {
                *w /= sum;
            }
        } else {
            weights[vi][0] = 1.0;
        }
    }

    // 3 passes of mesh-connectivity Laplacian smoothing (only smooths auto-skinned vertices along connected edges!)
    for _ in 0..3 {
        let prev = weights.clone();
        for vi in 0..nv {
            if v_rigid[vi].is_some() || adj[vi].is_empty() {
                continue;
            }
            let deg = adj[vi].len() as f32;
            let mut sum = 0.0f32;
            for bi in 0..nb {
                let mut nbr = 0.0f32;
                for &ni in &adj[vi] {
                    nbr += prev[ni][bi];
                }
                let blended = prev[vi][bi] * 0.65 + (nbr / deg) * 0.35;
                weights[vi][bi] = blended;
                sum += blended;
            }
            if sum > 1e-12 {
                for w in &mut weights[vi] {
                    *w /= sum;
                }
            }
        }
    }

    // Compress each vertex to top-4 glTF joints + normalized weights
    let mut v_skin: Vec<VertexSkin> = Vec::with_capacity(nv);
    for vi in 0..nv {
        let mut pairs: Vec<(u16, f32)> = weights[vi]
            .iter()
            .enumerate()
            .filter(|(_, &w)| w > 1e-4)
            .map(|(bi, &w)| (bi as u16, w))
            .collect();
        pairs.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        pairs.truncate(4);
        // Drop tiny noise weights (< 2%) if at least 1 strong bone exists
        if pairs.len() > 1 {
            let max_w = pairs[0].1;
            pairs.retain(|&(_, w)| w >= max_w * 0.02);
        }
        let sum: f32 = pairs.iter().map(|p| p.1).sum();
        let mut js = [0u16; 4];
        let mut ws = [0.0f32; 4];
        if sum > 1e-12 {
            for (k, &(bi, w)) in pairs.iter().enumerate() {
                js[k] = bi;
                ws[k] = w / sum;
            }
        } else {
            ws[0] = 1.0;
        }
        v_skin.push(VertexSkin { joints: js, weights: ws });
    }

    // Assemble per-triangle vertex skins (explicit per-triangle rigid_bone overrides even at shared seams)
    let mut out = Vec::with_capacity(tris.len());
    for (ti, t) in tris.iter().enumerate() {
        if let Some(rb) = t.rigid_bone {
            if (rb as usize) < nb {
                let rs = VertexSkin {
                    joints: [rb, 0, 0, 0],
                    weights: [1.0, 0.0, 0.0, 0.0],
                };
                out.push([rs, rs, rs]);
                continue;
            }
        }
        let [ia, ib, ic] = tri_v_idx[ti];
        out.push([v_skin[ia], v_skin[ib], v_skin[ic]]);
    }
    out
}

/// Pose of a skeleton at a specific timestamp: local `(translation, rotation, scale)` and world `Mat4` per bone.
#[derive(Debug, Clone)]
pub struct SkeletonPose {
    pub local_pos: Vec<Vec3>,
    pub local_rot: Vec<Quat>,
    pub local_scale: Vec<Vec3>,
    pub world_mats: Vec<Mat4>,
}

impl SkeletonPose {
    pub fn rest(bones: &[Bone]) -> Self {
        let n = bones.len();
        let local_pos: Vec<Vec3> = bones.iter().map(|b| b.rest_local_translation(bones)).collect();
        let local_rot = vec![Quat::IDENTITY; n];
        let local_scale = vec![Vec3::ONE; n];
        let mut pose = Self {
            local_pos,
            local_rot,
            local_scale,
            world_mats: vec![Mat4::IDENTITY; n],
        };
        pose.recompute_world(bones);
        pose
    }

    pub fn recompute_world(&mut self, bones: &[Bone]) {
        for i in 0..bones.len() {
            let local = self.local_rot[i].to_mat4(self.local_pos[i], self.local_scale[i]);
            self.world_mats[i] = match bones[i].parent {
                Some(pi) => self.world_mats[pi].mul(local),
                None => local,
            };
        }
    }

    pub fn world_rot(&self, bones: &[Bone], idx: usize) -> Quat {
        let mut q = self.local_rot[idx];
        let mut cur = bones[idx].parent;
        while let Some(pi) = cur {
            q = self.local_rot[pi].mul(q);
            cur = bones[pi].parent;
        }
        q.normalized()
    }

    pub fn bone_world_head(&self, idx: usize) -> Vec3 {
        self.world_mats[idx].transform_point(Vec3::ZERO)
    }

    pub fn bone_world_tail(&self, bones: &[Bone], idx: usize) -> Vec3 {
        let local_tail = bones[idx].tail - bones[idx].head;
        self.world_mats[idx].transform_point(local_tail)
    }
}

/// Solve an IK chain of `chain_bones` (ordered root-to-leaf) so that the end point
/// (`tail` of last bone in `chain_bones`) reaches `target_world`, using exact 2-bone analytical IK
/// when `chain_bones.len() == 2` and FABRIK when `chain_bones.len() >= 3`.
pub fn solve_ik_chain(
    bones: &[Bone],
    pose: &mut SkeletonPose,
    chain_bones: &[usize],
    target_world: Vec3,
    pole_world: Option<Vec3>,
) {
    if chain_bones.is_empty() {
        return;
    }
    pose.recompute_world(bones);

    // 1-bone aim solver
    if chain_bones.len() == 1 {
        let b0 = chain_bones[0];
        let root_pos = pose.bone_world_head(b0);
        let parent_wrot = bones[b0]
            .parent
            .map(|p| pose.world_rot(bones, p))
            .unwrap_or(Quat::IDENTITY);
        let rest_dir = (bones[b0].tail - bones[b0].head).normalized();
        let cur_dir = parent_wrot.rotate_vec3(rest_dir);
        let want_dir = (target_world - root_pos).normalized();
        if want_dir.len_sq() > 0.5 {
            let dq_world = Quat::from_to(cur_dir, want_dir);
            let new_wrot = dq_world.mul(parent_wrot).normalized();
            pose.local_rot[b0] = parent_wrot.conjugate().mul(new_wrot).normalized();
            pose.recompute_world(bones);
        }
        return;
    }

    // 2-bone exact analytical Law-of-Cosines IK
    if chain_bones.len() == 2 {
        let b0 = chain_bones[0];
        let b1 = chain_bones[1];
        let p0 = pose.bone_world_head(b0);
        let l1 = bones[b0].length();
        let l2 = bones[b1].length();

        let to_t = target_world - p0;
        let dist_raw = to_t.len();
        if dist_raw < 1e-6 {
            return;
        }
        let dir = to_t / dist_raw;
        let min_d = (l1 - l2).abs() + 1e-4;
        let max_d = (l1 + l2) - 1e-4;
        let d = dist_raw.clamp(min_d, max_d);

        // Law of cosines for shoulder/hip angle alpha
        let cos_a = ((l1 * l1 + d * d - l2 * l2) / (2.0 * l1 * d)).clamp(-1.0, 1.0);
        let sin_a = (1.0 - cos_a * cos_a).max(0.0).sqrt();

        // Bend plane normal from pole vector (or fallback to current mid-joint bend / +Z)
        let pole_vec = if let Some(pole) = pole_world {
            pole - p0
        } else {
            let cur_mid = pose.bone_world_head(b1);
            let v = cur_mid - p0;
            let perp = v - dir * v.dot(dir);
            if perp.len_sq() > 1e-6 {
                perp
            } else {
                Vec3::Z
            }
        };
        let mut bend_dir = pole_vec - dir * pole_vec.dot(dir);
        if bend_dir.len_sq() < 1e-8 {
            let fallback = if dir.z.abs() < 0.9 { Vec3::Z } else { Vec3::X };
            bend_dir = fallback - dir * fallback.dot(dir);
        }
        bend_dir = bend_dir.normalized();

        let desired_mid = p0 + dir * (l1 * cos_a) + bend_dir * (l1 * sin_a);
        let desired_end = p0 + dir * d;

        // Orient b0 so rest_dir0 aligns with (desired_mid - p0)
        let parent0_wrot = bones[b0]
            .parent
            .map(|p| pose.world_rot(bones, p))
            .unwrap_or(Quat::IDENTITY);
        let rest_dir0 = (bones[b0].tail - bones[b0].head).normalized();
        let base_dir0 = parent0_wrot.rotate_vec3(rest_dir0);
        let want_dir0 = (desired_mid - p0).normalized();
        let q0_world = Quat::from_to(base_dir0, want_dir0).mul(parent0_wrot).normalized();
        pose.local_rot[b0] = parent0_wrot.conjugate().mul(q0_world).normalized();
        pose.recompute_world(bones);

        // Orient b1 so rest_dir1 aligns with (desired_end - actual_mid)
        let actual_mid = pose.bone_world_head(b1);
        let parent1_wrot = pose.world_rot(bones, b0);
        let rest_dir1 = (bones[b1].tail - bones[b1].head).normalized();
        let base_dir1 = parent1_wrot.rotate_vec3(rest_dir1);
        let want_dir1 = (desired_end - actual_mid).normalized();
        let q1_world = Quat::from_to(base_dir1, want_dir1).mul(parent1_wrot).normalized();
        pose.local_rot[b1] = parent1_wrot.conjugate().mul(q1_world).normalized();
        pose.recompute_world(bones);
        return;
    }

    // N-bone FABRIK IK (chain_bones.len() >= 3)
    let m = chain_bones.len();
    let lengths: Vec<f32> = chain_bones.iter().map(|&bi| bones[bi].length()).collect();
    let origin = pose.bone_world_head(chain_bones[0]);
    let mut pts: Vec<Vec3> = Vec::with_capacity(m + 1);
    for &bi in chain_bones {
        pts.push(pose.bone_world_head(bi));
    }
    pts.push(pose.bone_world_tail(bones, *chain_bones.last().unwrap()));

    // Optional gentle pole nudge on interior joints before FABRIK iterations
    if let Some(pole) = pole_world {
        for k in 1..m {
            pts[k] = pts[k].lerp(pole, 0.15);
        }
    }

    let total_len: f32 = lengths.iter().sum();
    if (target_world - origin).len() >= total_len - 1e-4 {
        let dir = (target_world - origin).normalized();
        pts[0] = origin;
        for k in 0..m {
            pts[k + 1] = pts[k] + dir * lengths[k];
        }
    } else {
        for _ in 0..16 {
            // Backward pass: pin end effector to target
            pts[m] = target_world;
            for k in (0..m).rev() {
                let d = (pts[k] - pts[k + 1]).normalized();
                pts[k] = pts[k + 1] + (if d.len_sq() > 0.5 { d } else { Vec3::Y }) * lengths[k];
            }
            // Forward pass: pin root back to origin
            pts[0] = origin;
            for k in 0..m {
                let d = (pts[k + 1] - pts[k]).normalized();
                pts[k + 1] = pts[k] + (if d.len_sq() > 0.5 { d } else { Vec3::Y }) * lengths[k];
            }
            if (pts[m] - target_world).len() < 1e-4 {
                break;
            }
        }
    }

    // Extract per-bone local quaternions sequentially along the chain
    for k in 0..m {
        let bi = chain_bones[k];
        let parent_wrot = bones[bi]
            .parent
            .map(|p| pose.world_rot(bones, p))
            .unwrap_or(Quat::IDENTITY);
        let rest_dir = (bones[bi].tail - bones[bi].head).normalized();
        let base_dir = parent_wrot.rotate_vec3(rest_dir);
        let want_dir = (pts[k + 1] - pts[k]).normalized();
        if want_dir.len_sq() > 0.5 {
            let q_world = Quat::from_to(base_dir, want_dir).mul(parent_wrot).normalized();
            pose.local_rot[bi] = parent_wrot.conjugate().mul(q_world).normalized();
            pose.recompute_world(bones);
        }
    }
}

/// Resolve the ordered root-to-leaf bone indices for an `IkKeyframe` target.
pub fn resolve_ik_chain_bones(bones: &[Bone], effector_name: &str, chain_len: usize, use_tip: bool) -> Option<Vec<usize>> {
    let eff_idx = bones.iter().position(|b| b.name == effector_name)?;
    let n = chain_len.max(1);
    // Walk ancestors of eff_idx
    let mut anc = Vec::new();
    let mut cur = bones[eff_idx].parent;
    while let Some(pi) = cur {
        anc.push(pi);
        cur = bones[pi].parent;
    }
    let mut chain = Vec::new();
    if !use_tip && anc.len() >= n {
        // Solve the `n` bones leading up to `eff_idx`'s head (e.g. Foot_L -> [UpperLeg_L, LowerLeg_L])
        for &bi in anc.iter().take(n) {
            chain.push(bi);
        }
    } else {
        // Include `eff_idx` itself so `eff_idx`'s tail hits the target (e.g. robot arm / 2-bone arm leaf)
        chain.push(eff_idx);
        for &bi in anc.iter().take(n.saturating_sub(1)) {
            chain.push(bi);
        }
    }
    chain.reverse();
    Some(chain)
}

/// Evaluate the full skeleton pose (FK keyframes + IK chains) at time `t`.
pub fn evaluate_skeleton_pose(bones: &[Bone], action: &AnimAction, t: f32) -> SkeletonPose {
    let mut pose = SkeletonPose::rest(bones);
    let tracks = action.collect_tracks();

    // 1. Apply FK bone tracks (pos / delta-pos / rot / scale)
    for (bi, b) in bones.iter().enumerate() {
        if let Some(tr) = tracks.get(&b.name) {
            let rest_p = b.rest_local_translation(bones);
            pose.local_pos[bi] = eval_vec3_track(&tr.pos, t, rest_p);
            let rot_deg = eval_vec3_track(&tr.rot, t, Vec3::ZERO);
            pose.local_rot[bi] = Quat::from_euler_xyz_deg(rot_deg);
            pose.local_scale[bi] = eval_vec3_track(&tr.scale, t, Vec3::ONE);
        }
    }
    pose.recompute_world(bones);

    // 2. Group and evaluate IK target tracks per effector bone
    if !action.ik_keys.is_empty() {
        let mut groups: Vec<(String, usize, bool, Vec<TrackKey>, Vec<TrackKey>)> = Vec::new();
        for ik in &action.ik_keys {
            let pos_k = TrackKey {
                time: ik.time,
                value: ik.target,
                interp: ik.interp,
            };
            let pole_k = ik.pole.map(|p| TrackKey {
                time: ik.time,
                value: p,
                interp: ik.interp,
            });
            if let Some(g) = groups.iter_mut().find(|g| g.0 == ik.effector) {
                g.3.push(pos_k);
                if let Some(pk) = pole_k {
                    g.4.push(pk);
                }
            } else {
                groups.push((
                    ik.effector.clone(),
                    ik.chain_len,
                    ik.use_tip,
                    vec![pos_k],
                    pole_k.into_iter().collect(),
                ));
            }
        }
        for (eff, clen, use_tip, mut t_keys, mut p_keys) in groups {
            t_keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
            p_keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
            let target = eval_vec3_track(&t_keys, t, t_keys[0].value);
            let pole = if p_keys.is_empty() {
                None
            } else {
                Some(eval_vec3_track(&p_keys, t, p_keys[0].value))
            };
            if let Some(chain) = resolve_ik_chain_bones(bones, &eff, clen, use_tip) {
                solve_ik_chain(bones, &mut pose, &chain, target, pole);
            }
        }
    }

    pose
}

/// Apply CPU Linear Blend Skinning (LBS) to `mesh.tris` using `bones` and `pose`.
pub fn apply_lbs_skinning(
    tris: &[Triangle],
    skins: &[[VertexSkin; 3]],
    bones: &[Bone],
    pose: &SkeletonPose,
) -> Vec<Triangle> {
    let skin_mats: Vec<Mat4> = bones
        .iter()
        .enumerate()
        .map(|(i, b)| pose.world_mats[i].mul(Mat4::translation(-b.head)))
        .collect();

    let deform = |p: Vec3, n: Vec3, vs: VertexSkin| -> (Vec3, Vec3) {
        let mut out_p = Vec3::ZERO;
        let mut out_n = Vec3::ZERO;
        for k in 0..4 {
            let w = vs.weights[k];
            if w <= 1e-6 {
                continue;
            }
            let m = skin_mats[vs.joints[k] as usize];
            out_p = out_p + m.transform_point(p) * w;
            out_n = out_n + m.transform_normal(n) * w;
        }
        let nn = out_n.normalized();
        (out_p, if nn.len_sq() > 0.5 { nn } else { n })
    };

    let mut out = Vec::with_capacity(tris.len());
    for (ti, t) in tris.iter().enumerate() {
        let s = skins[ti];
        let (a, na) = deform(t.a, t.na, s[0]);
        let (b, nb) = deform(t.b, t.nb, s[1]);
        let (c, nc) = deform(t.c, t.nc, s[2]);
        let fn_v = (b - a).cross(c - a).normalized();
        out.push(Triangle {
            a,
            b,
            c,
            na,
            nb,
            nc,
            face_normal: if fn_v.len_sq() > 0.5 { fn_v } else { t.face_normal },
            uv: t.uv,
            is_translucent: t.is_translucent,
            rigid_bone: t.rigid_bone,
            part_tag: t.part_tag,
        });
    }
    out
}

/// Helper for `evaluate_pose`: skin the root mesh if `mesh.bones` is non-empty and record world bone positions.
pub fn skin_mesh_for_action(
    mesh: &Mesh,
    action: &AnimAction,
    t: f32,
) -> (Vec<Triangle>, HashMap<String, Vec3>, SkeletonPose) {
    let pose = evaluate_skeleton_pose(&mesh.bones, action, t);
    let mut bone_pts = HashMap::new();
    for (bi, b) in mesh.bones.iter().enumerate() {
        // Track bone head (or tail for leaf bones without children)
        let has_child = mesh.bones.iter().any(|o| o.parent == Some(bi));
        let pt = if has_child {
            pose.bone_world_head(bi)
        } else {
            pose.bone_world_tail(&mesh.bones, bi)
        };
        bone_pts.insert(b.name.clone(), pt);
    }
    let skins = compute_auto_skinning(&mesh.tris, &mesh.bones);
    let skinned_tris = apply_lbs_skinning(&mesh.tris, &skins, &mesh.bones, &pose);
    (skinned_tris, bone_pts, pose)
}
