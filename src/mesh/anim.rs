use std::collections::HashMap;

use crate::common::math::{Aabb, Mat4, Vec3};
use crate::mesh::geom::{ChildNodeMesh, Mesh};
use crate::mesh::morph::{apply_morph_targets, evaluate_morph_weights, MorphKeyframe};
use crate::mesh::skel::{
    apply_lbs_skinning, compute_auto_skinning, evaluate_skeleton_pose, IkKeyframe,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InterpMode {
    Step,
    Linear,
    Ease,
    EaseIn,
    EaseOut,
    Cubic,
}

impl InterpMode {
    pub fn parse(s: &str) -> Option<Self> {
        match s.to_ascii_lowercase().as_str() {
            "step" | "hold" | "const" => Some(Self::Step),
            "lin" | "linear" => Some(Self::Linear),
            "ease" | "smooth" | "inout" | "easeinout" => Some(Self::Ease),
            "easein" | "in" => Some(Self::EaseIn),
            "easeout" | "out" => Some(Self::EaseOut),
            "cubic" | "spline" | "catmull" => Some(Self::Cubic),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct AnimKeyframe {
    pub time: f32,
    pub target: String,
    pub pos: Option<Vec3>,
    pub rot_deg: Option<Vec3>,
    pub scale: Option<Vec3>,
    pub interp: InterpMode,
}

#[derive(Debug, Clone)]
pub struct AnimEvent {
    pub time: f32,
    pub name: String,
    pub data: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AnimAction {
    pub name: String,
    pub duration: f32,
    pub fps: f32,
    pub looping: bool,
    pub keyframes: Vec<AnimKeyframe>,
    pub ik_keys: Vec<IkKeyframe>,
    pub morph_keys: Vec<MorphKeyframe>,
    pub events: Vec<AnimEvent>,
}

#[derive(Debug, Clone, Copy)]
pub struct TrackKey {
    pub time: f32,
    pub value: Vec3,
    pub interp: InterpMode,
}

#[derive(Debug, Clone, Default)]
pub struct NodeTracks {
    pub pos: Vec<TrackKey>,
    pub rot: Vec<TrackKey>,
    pub scale: Vec<TrackKey>,
}

impl AnimAction {
    /// Collect sorted per-node channel tracks (`pos`, `rot`, `scale`).
    pub fn collect_tracks(&self) -> HashMap<String, NodeTracks> {
        let mut map: HashMap<String, NodeTracks> = HashMap::new();
        for kf in &self.keyframes {
            let entry = map.entry(kf.target.clone()).or_default();
            if let Some(p) = kf.pos {
                entry.pos.push(TrackKey {
                    time: kf.time,
                    value: p,
                    interp: kf.interp,
                });
            }
            if let Some(r) = kf.rot_deg {
                entry.rot.push(TrackKey {
                    time: kf.time,
                    value: r,
                    interp: kf.interp,
                });
            }
            if let Some(s) = kf.scale {
                entry.scale.push(TrackKey {
                    time: kf.time,
                    value: s,
                    interp: kf.interp,
                });
            }
        }
        let dedup_sort = |keys: &mut Vec<TrackKey>| {
            keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
            // Keep latest value if duplicate timestamps exist
            let mut out: Vec<TrackKey> = Vec::with_capacity(keys.len());
            for k in keys.drain(..) {
                if let Some(last) = out.last_mut() {
                    if (last.time - k.time).abs() < 1e-5 {
                        *last = k;
                        continue;
                    }
                }
                out.push(k);
            }
            *keys = out;
        };
        for tr in map.values_mut() {
            dedup_sort(&mut tr.pos);
            dedup_sort(&mut tr.rot);
            dedup_sort(&mut tr.scale);
        }
        map
    }

    pub fn animated_targets(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        for kf in &self.keyframes {
            if !out.contains(&kf.target) {
                out.push(kf.target.clone());
            }
        }
        for ik in &self.ik_keys {
            if !out.contains(&ik.effector) {
                out.push(ik.effector.clone());
            }
        }
        out
    }
}

/// Evaluate a Vec3 channel track at time `t` with exact interpolation (Step, Linear, Ease/In/Out, Catmull-Rom Cubic).
pub fn eval_vec3_track(keys: &[TrackKey], t: f32, default: Vec3) -> Vec3 {
    if keys.is_empty() {
        return default;
    }
    if keys.len() == 1 || t <= keys[0].time {
        return keys[0].value;
    }
    let last = keys.len() - 1;
    if t >= keys[last].time {
        return keys[last].value;
    }
    let mut i = 0usize;
    while i + 1 < keys.len() && t > keys[i + 1].time {
        i += 1;
    }
    let k0 = keys[i];
    let k1 = keys[i + 1];
    let dt = (k1.time - k0.time).max(1e-6);
    let u = ((t - k0.time) / dt).clamp(0.0, 1.0);

    match k0.interp {
        InterpMode::Step => k0.value,
        InterpMode::Linear => k0.value.lerp(k1.value, u),
        InterpMode::Ease => {
            let s = u * u * (3.0 - 2.0 * u);
            k0.value.lerp(k1.value, s)
        }
        InterpMode::EaseIn => {
            let s = u * u;
            k0.value.lerp(k1.value, s)
        }
        InterpMode::EaseOut => {
            let inv = 1.0 - u;
            let s = 1.0 - inv * inv;
            k0.value.lerp(k1.value, s)
        }
        InterpMode::Cubic => {
            // Non-uniform Hermite / Catmull-Rom tangent interpolation
            let (m0, m1) = catmull_tangents(keys, i);
            eval_hermite(k0.value, m0 * dt, k1.value, m1 * dt, u)
        }
    }
}

/// Compute per-key (in_tangent, value, out_tangent) slopes (units/sec) for native glTF CUBICSPLINE export.
pub fn build_cubicspline_triples(keys: &[TrackKey]) -> Vec<(Vec3, Vec3, Vec3)> {
    let n = keys.len();
    let mut out = Vec::with_capacity(n);
    for i in 0..n {
        let v = keys[i].value;
        // Out-tangent controls segment [i -> i+1] using keys[i].interp
        let out_t = if i + 1 < n {
            let dt = (keys[i + 1].time - keys[i].time).max(1e-6);
            let dv = keys[i + 1].value - keys[i].value;
            match keys[i].interp {
                InterpMode::Linear => dv / dt,
                InterpMode::Ease => Vec3::ZERO,
                InterpMode::EaseIn => Vec3::ZERO,
                InterpMode::EaseOut => dv * (2.0 / dt),
                InterpMode::Cubic => catmull_tangents(keys, i).0,
                InterpMode::Step => Vec3::ZERO,
            }
        } else {
            Vec3::ZERO
        };

        // In-tangent controls segment [i-1 -> i] using keys[i-1].interp
        let in_t = if i > 0 {
            let dt = (keys[i].time - keys[i - 1].time).max(1e-6);
            let dv = keys[i].value - keys[i - 1].value;
            match keys[i - 1].interp {
                InterpMode::Linear => dv / dt,
                InterpMode::Ease => Vec3::ZERO,
                InterpMode::EaseIn => dv * (2.0 / dt),
                InterpMode::EaseOut => Vec3::ZERO,
                InterpMode::Cubic => catmull_tangents(keys, i - 1).1,
                InterpMode::Step => Vec3::ZERO,
            }
        } else {
            Vec3::ZERO
        };

        out.push((in_t, v, out_t));
    }
    out
}

fn catmull_tangents(keys: &[TrackKey], i: usize) -> (Vec3, Vec3) {
    let n = keys.len();
    let m0 = if i == 0 {
        let dt = (keys[1].time - keys[0].time).max(1e-6);
        (keys[1].value - keys[0].value) / dt
    } else {
        let dt = (keys[i + 1].time - keys[i - 1].time).max(1e-6);
        (keys[i + 1].value - keys[i - 1].value) / dt
    };
    let m1 = if i + 2 >= n {
        let dt = (keys[i + 1].time - keys[i].time).max(1e-6);
        (keys[i + 1].value - keys[i].value) / dt
    } else {
        let dt = (keys[i + 2].time - keys[i].time).max(1e-6);
        (keys[i + 2].value - keys[i].value) / dt
    };
    (m0, m1)
}

fn eval_hermite(p0: Vec3, m0: Vec3, p1: Vec3, m1: Vec3, u: f32) -> Vec3 {
    let u2 = u * u;
    let u3 = u2 * u;
    let h00 = 2.0 * u3 - 3.0 * u2 + 1.0;
    let h10 = u3 - 2.0 * u2 + u;
    let h01 = -2.0 * u3 + 3.0 * u2;
    let h11 = u3 - u2;
    p0 * h00 + m0 * h10 + p1 * h01 + m1 * h11
}

/// Evaluate `mesh` posed under `action` at time `t`, returning the posed flat visual mesh
/// and a map of each child node's world-space pivot position (and geometric center).
pub fn evaluate_pose(
    mesh: &Mesh,
    action: &AnimAction,
    t: f32,
) -> (Mesh, HashMap<String, Vec3>) {
    let tracks = action.collect_tracks();
    // 1. Apply morph targets FIRST in rest/bind pose (exact glTF 2.0 / GPU order!)
    let morphed_tris = if !mesh.morphs.is_empty() {
        let weights = evaluate_morph_weights(&mesh.morphs, action, t);
        apply_morph_targets(&mesh.tris, &mesh.morphs, &weights)
    } else {
        mesh.tris.clone()
    };

    // 2. Apply skeletal LBS skinning if rigged
    let (root_tris, mut pivots, posed_bones) = if !mesh.bones.is_empty() {
        let pose = evaluate_skeleton_pose(&mesh.bones, action, t);
        let mut bpts = HashMap::new();
        for (bi, b) in mesh.bones.iter().enumerate() {
            let has_child = mesh.bones.iter().any(|o| o.parent == Some(bi));
            let pt = if has_child {
                pose.bone_world_head(bi)
            } else {
                pose.bone_world_tail(&mesh.bones, bi)
            };
            bpts.insert(b.name.clone(), pt);
        }
        // Compute skinning from bind-pose `mesh.tris`, deform `morphed_tris`!
        let skins = compute_auto_skinning(&mesh.tris, &mesh.bones);
        let skinned = apply_lbs_skinning(&morphed_tris, &skins, &mesh.bones, &pose);
        let pb: Vec<crate::mesh::skel::Bone> = mesh
            .bones
            .iter()
            .enumerate()
            .map(|(bi, b)| {
                let mut nb = b.clone();
                nb.head = pose.bone_world_head(bi);
                nb.tail = pose.bone_world_tail(&mesh.bones, bi);
                nb
            })
            .collect();
        (skinned, bpts, pb)
    } else {
        (morphed_tris, HashMap::new(), Vec::new())
    };
    let mut out = Mesh {
        tris: root_tris,
        part_bounds: mesh.part_bounds.clone(),
        colliders: Vec::new(),
        nodes: Vec::new(),
        actions: Vec::new(),
        bones: posed_bones,
        morphs: Vec::new(),
        tag_names: Vec::new(),
        textures: mesh.textures.clone(),
    };
    for node in &mesh.nodes {
        eval_node_recursive(node, Mat4::IDENTITY, &tracks, t, &mut out, &mut pivots);
    }
    (out, pivots)
}

fn eval_node_recursive(
    node: &ChildNodeMesh,
    parent_mat: Mat4,
    tracks: &HashMap<String, NodeTracks>,
    t: f32,
    out: &mut Mesh,
    pivots: &mut HashMap<String, Vec3>,
) {
    let (pos, rot, scale) = if let Some(tr) = tracks.get(&node.name) {
        (
            eval_vec3_track(&tr.pos, t, node.translation),
            eval_vec3_track(&tr.rot, t, node.rotation_deg),
            eval_vec3_track(&tr.scale, t, node.scale),
        )
    } else {
        (node.translation, node.rotation_deg, node.scale)
    };
    let world_mat = parent_mat.mul(Mat4::from_trs(pos, rot, scale));
    // Track world-space visual center of this node's geometry if non-empty, else its pivot
    let track_pt = if !node.tris.is_empty() {
        let mut bb = Aabb::empty();
        for tri in &node.tris {
            bb.expand(tri.a);
            bb.expand(tri.b);
            bb.expand(tri.c);
        }
        world_mat.transform_point(bb.center())
    } else {
        world_mat.transform_point(Vec3::ZERO)
    };
    pivots.insert(node.name.clone(), track_pt);

    if !node.tris.is_empty() {
        let sub = Mesh {
            tris: node.tris.clone(),
            ..Mesh::default()
        };
        out.append_transformed(&sub, world_mat, Some(&node.name));
    }
    for ch in &node.children {
        eval_node_recursive(ch, world_mat, tracks, t, out, pivots);
    }
}

/// Compute unified AABB across the entire animation so camera framing stays rock-steady across frames.
pub fn compute_action_bounds(mesh: &Mesh, action: &AnimAction, samples: usize) -> Aabb {
    let mut bb = mesh.compute_bounds();
    let n = samples.max(4);
    for i in 0..=n {
        let t = action.duration * (i as f32 / n as f32);
        let (posed, _) = evaluate_pose(mesh, action, t);
        bb.expand_aabb(&posed.compute_bounds());
    }
    bb
}
