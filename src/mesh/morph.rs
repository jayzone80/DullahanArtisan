use crate::common::math::{Aabb, Mat4, Vec3};
use crate::mesh::anim::{eval_vec3_track, AnimAction, InterpMode, TrackKey};
use crate::mesh::geom::{Mesh, Triangle};

#[derive(Debug, Clone)]
pub struct MorphTarget {
    pub name: String,
    pub default_weight: f32,
    /// Per-triangle vertex position deltas `[da, db, dc]` (parallel to `Mesh.tris`).
    pub delta_pos: Vec<[Vec3; 3]>,
    /// Per-triangle vertex normal deltas `[dna, dnb, dnc]` (parallel to `Mesh.tris`).
    pub delta_norm: Vec<[Vec3; 3]>,
}

#[derive(Debug, Clone)]
pub struct MorphKeyframe {
    pub time: f32,
    pub target: String,
    pub weight: f32,
    pub interp: InterpMode,
}

#[derive(Debug, Clone)]
pub enum MorphOp {
    Scale {
        tag: Option<String>,
        scale: Vec3,
        pivot: Option<Vec3>,
        radius: Option<f32>,
    },
    Move {
        tag: Option<String>,
        delta: Vec3,
        center: Option<Vec3>,
        radius: Option<f32>,
        mirror_x: bool,
    },
    Rotate {
        tag: Option<String>,
        rot_deg: Vec3,
        pivot: Option<Vec3>,
        mirror_x: bool,
    },
    Inflate {
        tag: Option<String>,
        amount: f32,
        center: Option<Vec3>,
        radius: Option<f32>,
        mirror_x: bool,
    },
}

fn smooth_falloff(p: Vec3, center: Vec3, radius: f32) -> f32 {
    if radius <= 1e-6 {
        return 1.0;
    }
    let d = (p - center).len();
    if d >= radius {
        return 0.0;
    }
    let u = 1.0 - d / radius;
    u * u * (3.0 - 2.0 * u)
}

fn tag_center(mesh: &Mesh, tag_id: Option<u16>) -> Vec3 {
    let mut bb = Aabb::empty();
    for t in &mesh.tris {
        if tag_id.is_none() || t.part_tag == tag_id {
            bb.expand(t.a);
            bb.expand(t.b);
            bb.expand(t.c);
        }
    }
    if bb.is_valid() {
        bb.center()
    } else {
        Vec3::ZERO
    }
}

/// Evaluate a list of `MorphOp`s against `mesh.tris` and compute per-triangle `(delta_pos, delta_norm)`.
pub fn build_morph_target(
    mesh: &Mesh,
    name: String,
    default_weight: f32,
    ops: &[MorphOp],
) -> Result<MorphTarget, String> {
    let n = mesh.tris.len();
    let mut pos: Vec<[Vec3; 3]> = mesh.tris.iter().map(|t| [t.a, t.b, t.c]).collect();

    let resolve_tag = |tag: &Option<String>| -> Result<Option<u16>, String> {
        let Some(tname) = tag else {
            return Ok(None);
        };
        if tname.eq_ignore_ascii_case("all") || tname == "*" {
            return Ok(None);
        }
        let idx = mesh
            .tag_names
            .iter()
            .position(|s| s == tname)
            .ok_or_else(|| format!("morph '{name}' references unknown @part tag '@{tname}'"))?;
        Ok(Some(idx as u16))
    };

    for op in ops {
        match op {
            MorphOp::Scale {
                tag,
                scale,
                pivot,
                radius,
            } => {
                let tid = resolve_tag(tag)?;
                let piv = pivot.unwrap_or_else(|| tag_center(mesh, tid));
                for (ti, t) in mesh.tris.iter().enumerate() {
                    if tid.is_some() && t.part_tag != tid {
                        continue;
                    }
                    for v in &mut pos[ti] {
                        let w = radius.map(|r| smooth_falloff(*v, piv, r)).unwrap_or(1.0);
                        if w > 1e-6 {
                            let rel = *v - piv;
                            let target = piv + Vec3::new(rel.x * scale.x, rel.y * scale.y, rel.z * scale.z);
                            *v = v.lerp(target, w);
                        }
                    }
                }
            }
            MorphOp::Move {
                tag,
                delta,
                center,
                radius,
                mirror_x,
            } => {
                let tid = resolve_tag(tag)?;
                let centers: Vec<(Option<Vec3>, Vec3)> = if *mirror_x {
                    let c0 = *center;
                    let c1 = c0.map(|c| Vec3::new(-c.x, c.y, c.z));
                    vec![(c0, *delta), (c1, Vec3::new(-delta.x, delta.y, delta.z))]
                } else {
                    vec![(*center, *delta)]
                };
                for (ti, t) in mesh.tris.iter().enumerate() {
                    if tid.is_some() && t.part_tag != tid {
                        continue;
                    }
                    for v in &mut pos[ti] {
                        let mut acc = Vec3::ZERO;
                        for &(c_opt, d_vec) in &centers {
                            let w = match (c_opt, radius) {
                                (Some(c), Some(r)) => smooth_falloff(*v, c, *r),
                                _ => 1.0,
                            };
                            if w > 1e-6 {
                                acc = acc + d_vec * w;
                            }
                        }
                        *v = *v + acc;
                    }
                }
            }
            MorphOp::Rotate {
                tag,
                rot_deg,
                pivot,
                mirror_x,
            } => {
                let tid = resolve_tag(tag)?;
                let piv0 = pivot.unwrap_or_else(|| tag_center(mesh, tid));
                let rmat0 = Mat4::from_trs(Vec3::ZERO, *rot_deg, Vec3::ONE);
                let piv1 = Vec3::new(-piv0.x, piv0.y, piv0.z);
                let rmat1 = Mat4::from_trs(Vec3::ZERO, Vec3::new(rot_deg.x, -rot_deg.y, -rot_deg.z), Vec3::ONE);
                for (ti, t) in mesh.tris.iter().enumerate() {
                    if tid.is_some() && t.part_tag != tid {
                        continue;
                    }
                    let use_mirror = *mirror_x && ((t.a.x + t.b.x + t.c.x) * (1.0 / 3.0) < 0.0);
                    let (piv, rmat) = if use_mirror { (piv1, rmat1) } else { (piv0, rmat0) };
                    for v in &mut pos[ti] {
                        *v = piv + rmat.transform_point(*v - piv);
                    }
                }
            }
            MorphOp::Inflate {
                tag,
                amount,
                center,
                radius,
                mirror_x,
            } => {
                let tid = resolve_tag(tag)?;
                let centers: Vec<Option<Vec3>> = if *mirror_x {
                    vec![*center, center.map(|c| Vec3::new(-c.x, c.y, c.z))]
                } else {
                    vec![*center]
                };
                for (ti, t) in mesh.tris.iter().enumerate() {
                    if tid.is_some() && t.part_tag != tid {
                        continue;
                    }
                    let norms = [t.na, t.nb, t.nc];
                    for k in 0..3 {
                        let v = &mut pos[ti][k];
                        let mut w_tot = 0.0f32;
                        for &c_opt in &centers {
                            let w = match (c_opt, radius) {
                                (Some(c), Some(r)) => smooth_falloff(*v, c, *r),
                                _ => 1.0,
                            };
                            w_tot = (w_tot + w).min(1.0);
                        }
                        if w_tot > 1e-6 {
                            *v = *v + norms[k].normalized() * (*amount * w_tot);
                        }
                    }
                }
            }
        }
    }

    // Compute exact per-vertex position deltas and updated normal deltas
    let mut delta_pos = Vec::with_capacity(n);
    let mut delta_norm = Vec::with_capacity(n);
    for (ti, t) in mesh.tris.iter().enumerate() {
        let [pa, pb, pc] = pos[ti];
        let da = pa - t.a;
        let db = pb - t.b;
        let dc = pc - t.c;
        let moved = da.len_sq() + db.len_sq() + dc.len_sq() > 1e-12;
        if !moved {
            delta_pos.push([Vec3::ZERO; 3]);
            delta_norm.push([Vec3::ZERO; 3]);
        } else {
            delta_pos.push([da, db, dc]);
            let new_fn = (pb - pa).cross(pc - pa).normalized();
            if new_fn.len_sq() > 0.5 && t.face_normal.len_sq() > 0.5 {
                // Rotate each original smooth vertex normal by the triangle's face-normal change
                let q = crate::mesh::skel::Quat::from_to(t.face_normal, new_fn);
                let na1 = q.rotate_vec3(t.na).normalized();
                let nb1 = q.rotate_vec3(t.nb).normalized();
                let nc1 = q.rotate_vec3(t.nc).normalized();
                delta_norm.push([na1 - t.na, nb1 - t.nb, nc1 - t.nc]);
            } else {
                delta_norm.push([Vec3::ZERO; 3]);
            }
        }
    }

    Ok(MorphTarget {
        name,
        default_weight,
        delta_pos,
        delta_norm,
    })
}

/// Evaluate all morph target weights at time `t` for an `AnimAction`.
pub fn evaluate_morph_weights(morphs: &[MorphTarget], action: &AnimAction, t: f32) -> Vec<f32> {
    morphs
        .iter()
        .map(|m| {
            let mut keys: Vec<TrackKey> = action
                .morph_keys
                .iter()
                .filter(|mk| mk.target == m.name)
                .map(|mk| TrackKey {
                    time: mk.time,
                    value: Vec3::new(mk.weight, 0.0, 0.0),
                    interp: mk.interp,
                })
                .collect();
            keys.sort_by(|a, b| a.time.partial_cmp(&b.time).unwrap_or(std::cmp::Ordering::Equal));
            eval_vec3_track(&keys, t, Vec3::new(m.default_weight, 0.0, 0.0)).x
        })
        .collect()
}

/// Apply morph target deltas to a slice of `Triangle`s given per-morph `weights` (runs BEFORE LBS skinning!).
pub fn apply_morph_targets(tris: &[Triangle], morphs: &[MorphTarget], weights: &[f32]) -> Vec<Triangle> {
    let any_active = weights.iter().any(|&w| w.abs() > 1e-6);
    if morphs.is_empty() || !any_active {
        return tris.to_vec();
    }
    let mut out = tris.to_vec();
    for (mi, mt) in morphs.iter().enumerate() {
        let w = weights.get(mi).copied().unwrap_or(0.0);
        if w.abs() <= 1e-6 || mt.delta_pos.len() != out.len() {
            continue;
        }
        for (ti, t) in out.iter_mut().enumerate() {
            let dp = mt.delta_pos[ti];
            let dn = mt.delta_norm[ti];
            t.a = t.a + dp[0] * w;
            t.b = t.b + dp[1] * w;
            t.c = t.c + dp[2] * w;
            t.na = (t.na + dn[0] * w).normalized();
            t.nb = (t.nb + dn[1] * w).normalized();
            t.nc = (t.nc + dn[2] * w).normalized();
            let fn_v = (t.b - t.a).cross(t.c - t.a).normalized();
            if fn_v.len_sq() > 0.5 {
                t.face_normal = fn_v;
            }
        }
    }
    out
}
