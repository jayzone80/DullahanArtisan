/// Terrain heightfield + weightmap -> low-poly `.glb` mesher with optional Garland-Heckbert
/// QEM curvature-preserving edge-collapse decimation, palette-atlas biome coloring, optional
/// translucent water sheet, and solid diorama skirt.

use std::cmp::Ordering;
use std::collections::BinaryHeap;

use crate::common::math::Vec3;
use crate::mesh::geom::{ColliderMesh, Mesh};
use crate::mesh::palette::{Palette, ResolvedMat};
use crate::terrain::sim::TerrainData;

#[derive(Debug, Clone)]
pub struct TerrainMeshOptions {
    pub grid: usize,                // initial subdivision per axis (e.g. 64 -> 64x64 quads)
    pub target_tris: Option<usize>, // optional QEM decimation target triangle count
    pub skirt: bool,                // solid diorama base skirt
    pub smooth: bool,               // smooth vertex normals (false = crisp stylized low-poly facets)
    pub collider_name: Option<String>,
}

impl Default for TerrainMeshOptions {
    fn default() -> Self {
        Self {
            grid: 64,
            target_tris: None,
            skirt: true,
            smooth: false,
            collider_name: None,
        }
    }
}

/// Symmetric 4x4 error quadric stored as 10 upper-triangular coefficients.
#[derive(Clone, Copy, Default)]
struct Quadric([f64; 10]);

impl Quadric {
    fn from_plane(a: f64, b: f64, c: f64, d: f64) -> Self {
        Self([
            a * a, a * b, a * c, a * d,
                   b * b, b * c, b * d,
                          c * c, c * d,
                                 d * d,
        ])
    }

    fn add(&self, o: &Self) -> Self {
        let mut r = [0.0; 10];
        for i in 0..10 {
            r[i] = self.0[i] + o.0[i];
        }
        Self(r)
    }

    fn eval(&self, p: Vec3) -> f64 {
        let (x, y, z) = (p.x as f64, p.y as f64, p.z as f64);
        let q = &self.0;
        q[0] * x * x
            + 2.0 * q[1] * x * y
            + 2.0 * q[2] * x * z
            + 2.0 * q[3] * x
            + q[4] * y * y
            + 2.0 * q[5] * y * z
            + 2.0 * q[6] * y
            + q[7] * z * z
            + 2.0 * q[8] * z
            + q[9]
    }
}

#[derive(Clone, Copy)]
struct CollapseCand {
    cost: f64,
    u: usize,
    v: usize,
    u_ver: u32,
    v_ver: u32,
    target: Vec3,
}

impl PartialEq for CollapseCand {
    fn eq(&self, other: &Self) -> bool {
        self.cost == other.cost
    }
}
impl Eq for CollapseCand {}
impl PartialOrd for CollapseCand {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}
impl Ord for CollapseCand {
    fn cmp(&self, other: &Self) -> Ordering {
        // Min-heap by cost
        other.cost.total_cmp(&self.cost)
    }
}

pub fn build_terrain_mesh(
    terrain: &TerrainData,
    palette: &Palette,
    opts: &TerrainMeshOptions,
) -> Mesh {
    let g = opts.grid.clamp(8, 160);
    let stride = g + 1;
    let [wx, wy, wz] = terrain.world;
    let base_y = if opts.skirt { (wy * 0.14).max(wx.min(wz) * 0.035) } else { 0.0 };

    // 1. Sample initial heightfield grid vertices
    let mut verts: Vec<Vec3> = Vec::with_capacity(stride * stride);
    let mut is_border: Vec<bool> = Vec::with_capacity(stride * stride);
    for j in 0..=g {
        let v = j as f32 / g as f32;
        let z = (v - 0.5) * wz;
        for i in 0..=g {
            let u = i as f32 / g as f32;
            let x = (u - 0.5) * wx;
            let y = base_y + terrain.sample_height(u, v) * wy;
            verts.push(Vec3::new(x, y, z));
            is_border.push(i == 0 || i == g || j == 0 || j == g);
        }
    }

    // 2. Build outward-CCW (+Y up) triangles with alternating checkerboard diagonals
    let mut tris: Vec<[usize; 3]> = Vec::with_capacity(g * g * 2);
    for j in 0..g {
        for i in 0..g {
            let v00 = j * stride + i;
            let v10 = v00 + 1;
            let v01 = v00 + stride;
            let v11 = v01 + 1;
            // In right-handed Y-up (+X right, +Z front), (v00, v01, v10) has +Y upward normal!
            if (i + j) % 2 == 0 {
                tris.push([v00, v01, v11]);
                tris.push([v00, v11, v10]);
            } else {
                tris.push([v00, v01, v10]);
                tris.push([v10, v01, v11]);
            }
        }
    }

    // 3. Optional QEM curvature-preserving decimation (locks border so skirt stays exact)
    if let Some(target_t) = opts.target_tris {
        let target_t = target_t.max(16);
        if target_t < tris.len() {
            qem_decimate(&mut verts, &is_border, &mut tris, target_t);
        }
    }

    // Resolve palette swatches for the 4 splat channels + skirt + optional sea water
    let fallback_mat = palette
        .resolve_mat("cell:0,0")
        .unwrap_or(ResolvedMat { uv: crate::common::math::Vec2::new(0.5, 0.5), is_translucent: false });
    let ch_mats: [ResolvedMat; 4] = [
        palette.resolve_mat(&terrain.channel_mats[0]).unwrap_or(fallback_mat),
        palette.resolve_mat(&terrain.channel_mats[1]).unwrap_or(fallback_mat),
        palette.resolve_mat(&terrain.channel_mats[2]).unwrap_or(fallback_mat),
        palette.resolve_mat(&terrain.channel_mats[3]).unwrap_or(fallback_mat),
    ];
    let skirt_mat = palette
        .resolve_mat("dirt_dark")
        .or_else(|_| palette.resolve_mat("stone_dark"))
        .unwrap_or(ch_mats[2]);

    let mut mesh = Mesh::new();
    for &[a_i, b_i, c_i] in &tris {
        let a = verts[a_i];
        let b = verts[b_i];
        let c = verts[c_i];
        let cent = (a + b + c) * (1.0 / 3.0);
        let u = (cent.x / wx + 0.5).clamp(0.0, 1.0);
        let v = (cent.z / wz + 0.5).clamp(0.0, 1.0);
        let sp = terrain.sample_splat(u, v);
        let mut best_ch = 0usize;
        let mut best_w = sp[0];
        for k in 1..4 {
            if sp[k] > best_w {
                best_w = sp[k];
                best_ch = k;
            }
        }
        mesh.add_tri(a, b, c, ch_mats[best_ch]);
    }

    if opts.smooth {
        mesh.compute_smooth_normals();
    }

    // Optional separate collider node before adding decorative skirt/water
    if let Some(ref cname) = opts.collider_name {
        mesh.colliders.push(ColliderMesh {
            name: cname.clone(),
            tris: mesh.tris.clone(),
        });
    }

    // 4. Optional watertight diorama skirt around outer perimeter + flat bottom
    if opts.skirt {
        // North edge (j = 0, -Z): facing -Z
        for i in 0..g {
            let p0 = verts[i];
            let p1 = verts[i + 1];
            let b0 = Vec3::new(p0.x, 0.0, p0.z);
            let b1 = Vec3::new(p1.x, 0.0, p1.z);
            mesh.add_quad(p0, p1, b1, b0, skirt_mat);
        }
        // South edge (j = g, +Z): facing +Z
        for i in 0..g {
            let p0 = verts[g * stride + i];
            let p1 = verts[g * stride + i + 1];
            let b0 = Vec3::new(p0.x, 0.0, p0.z);
            let b1 = Vec3::new(p1.x, 0.0, p1.z);
            mesh.add_quad(p1, p0, b0, b1, skirt_mat);
        }
        // West edge (i = 0, -X): facing -X
        for j in 0..g {
            let p0 = verts[j * stride];
            let p1 = verts[(j + 1) * stride];
            let b0 = Vec3::new(p0.x, 0.0, p0.z);
            let b1 = Vec3::new(p1.x, 0.0, p1.z);
            mesh.add_quad(p1, p0, b0, b1, skirt_mat);
        }
        // East edge (i = g, +X): facing +X
        for j in 0..g {
            let p0 = verts[j * stride + g];
            let p1 = verts[(j + 1) * stride + g];
            let b0 = Vec3::new(p0.x, 0.0, p0.z);
            let b1 = Vec3::new(p1.x, 0.0, p1.z);
            mesh.add_quad(p0, p1, b1, b0, skirt_mat);
        }
        // Flat bottom cap at Y=0 facing -Y
        let c00 = Vec3::new(-wx * 0.5, 0.0, -wz * 0.5);
        let c10 = Vec3::new(wx * 0.5, 0.0, -wz * 0.5);
        let c11 = Vec3::new(wx * 0.5, 0.0, wz * 0.5);
        let c01 = Vec3::new(-wx * 0.5, 0.0, wz * 0.5);
        mesh.add_quad(c00, c10, c11, c01, skirt_mat);
    }

    // 5. Optional translucent water sheet (+ side water walls when skirt is enabled) at sea_level
    if terrain.sea_level > 0.005 {
        if let Ok(water_mat) = palette
            .resolve_mat("water_deep")
            .or_else(|_| palette.resolve_mat("glass_tint"))
        {
            let sy = base_y + terrain.sea_level * wy;
            let x0 = -wx * 0.5;
            let x1 = wx * 0.5;
            let z0 = -wz * 0.5;
            let z1 = wz * 0.5;
            mesh.add_quad(
                Vec3::new(x0, sy, z0),
                Vec3::new(x0, sy, z1),
                Vec3::new(x1, sy, z1),
                Vec3::new(x1, sy, z0),
                water_mat,
            );
            if opts.skirt {
                // Add translucent cutaway water sides along edges where seabed is below sea level
                for i in 0..g {
                    let p0 = verts[i];
                    let p1 = verts[i + 1];
                    if p0.y < sy || p1.y < sy {
                        let w0 = Vec3::new(p0.x, sy, p0.z);
                        let w1 = Vec3::new(p1.x, sy, p1.z);
                        mesh.add_quad(w0, w1, Vec3::new(p1.x, p1.y.min(sy), p1.z), Vec3::new(p0.x, p0.y.min(sy), p0.z), water_mat);
                    }
                    let q0 = verts[g * stride + i];
                    let q1 = verts[g * stride + i + 1];
                    if q0.y < sy || q1.y < sy {
                        let w0 = Vec3::new(q0.x, sy, q0.z);
                        let w1 = Vec3::new(q1.x, sy, q1.z);
                        mesh.add_quad(w1, w0, Vec3::new(q0.x, q0.y.min(sy), q0.z), Vec3::new(q1.x, q1.y.min(sy), q1.z), water_mat);
                    }
                }
                for j in 0..g {
                    let p0 = verts[j * stride];
                    let p1 = verts[(j + 1) * stride];
                    if p0.y < sy || p1.y < sy {
                        let w0 = Vec3::new(p0.x, sy, p0.z);
                        let w1 = Vec3::new(p1.x, sy, p1.z);
                        mesh.add_quad(w1, w0, Vec3::new(p0.x, p0.y.min(sy), p0.z), Vec3::new(p1.x, p1.y.min(sy), p1.z), water_mat);
                    }
                    let q0 = verts[j * stride + g];
                    let q1 = verts[(j + 1) * stride + g];
                    if q0.y < sy || q1.y < sy {
                        let w0 = Vec3::new(q0.x, sy, q0.z);
                        let w1 = Vec3::new(q1.x, sy, q1.z);
                        mesh.add_quad(w0, w1, Vec3::new(q1.x, q1.y.min(sy), q1.z), Vec3::new(q0.x, q0.y.min(sy), q0.z), water_mat);
                    }
                }
            }
        }
    }

    mesh
}

fn qem_decimate(
    verts: &mut [Vec3],
    is_border: &[bool],
    tris: &mut Vec<[usize; 3]>,
    target_tris: usize,
) {
    let nv = verts.len();
    let nt = tris.len();
    let mut quadrics = vec![Quadric::default(); nv];
    let mut v_tris: Vec<Vec<usize>> = vec![Vec::new(); nv];
    let mut tri_alive = vec![true; nt];
    let mut v_ver = vec![0u32; nv];
    let mut v_alive = vec![true; nv];

    for (ti, &[a, b, c]) in tris.iter().enumerate() {
        v_tris[a].push(ti);
        v_tris[b].push(ti);
        v_tris[c].push(ti);
        let n = (verts[b] - verts[a]).cross(verts[c] - verts[a]).normalized();
        if n.len_sq() > 0.5 {
            let d = -n.dot(verts[a]);
            let q = Quadric::from_plane(n.x as f64, n.y as f64, n.z as f64, d as f64);
            quadrics[a] = quadrics[a].add(&q);
            quadrics[b] = quadrics[b].add(&q);
            quadrics[c] = quadrics[c].add(&q);
        }
    }

    let make_cand = |u: usize, v: usize, verts: &[Vec3], quadrics: &[Quadric], v_ver: &[u32]| -> Option<CollapseCand> {
        if is_border[u] && is_border[v] {
            return None;
        }
        let q = quadrics[u].add(&quadrics[v]);
        let (target, cost) = if is_border[u] {
            (verts[u], q.eval(verts[u]))
        } else if is_border[v] {
            (verts[v], q.eval(verts[v]))
        } else {
            let mid = verts[u].lerp(verts[v], 0.5);
            let cu = q.eval(verts[u]);
            let cv = q.eval(verts[v]);
            let cm = q.eval(mid);
            if cm <= cu && cm <= cv {
                (mid, cm)
            } else if cu <= cv {
                (verts[u], cu)
            } else {
                (verts[v], cv)
            }
        };
        Some(CollapseCand {
            cost: cost.max(0.0),
            u,
            v,
            u_ver: v_ver[u],
            v_ver: v_ver[v],
            target,
        })
    };

    let mut heap: BinaryHeap<CollapseCand> = BinaryHeap::with_capacity(nt * 2);
    for &[a, b, c] in tris.iter() {
        for (p, q) in [(a, b), (b, c), (c, a)] {
            if p < q {
                if let Some(cnd) = make_cand(p, q, verts, &quadrics, &v_ver) {
                    heap.push(cnd);
                }
            }
        }
    }

    let mut alive_tris = nt;
    while alive_tris > target_tris {
        let Some(cnd) = heap.pop() else {
            break;
        };
        let (u, v) = (cnd.u, cnd.v);
        if !v_alive[u] || !v_alive[v] || v_ver[u] != cnd.u_ver || v_ver[v] != cnd.v_ver {
            continue;
        }
        // Keep border vertex as survivor when one endpoint is on the border
        let (keep, drop) = if is_border[v] && !is_border[u] { (v, u) } else { (u, v) };
        let new_pos = cnd.target;

        // Verify no surviving triangle flips upside down (normal.y must remain > 0.08)
        let mut ok = true;
        for &ti in v_tris[keep].iter().chain(v_tris[drop].iter()) {
            if !tri_alive[ti] {
                continue;
            }
            let t = tris[ti];
            let has_k = t[0] == keep || t[1] == keep || t[2] == keep;
            let has_d = t[0] == drop || t[1] == drop || t[2] == drop;
            if has_k && has_d {
                continue; // collapsed away
            }
            let pa = if t[0] == keep || t[0] == drop { new_pos } else { verts[t[0]] };
            let pb = if t[1] == keep || t[1] == drop { new_pos } else { verts[t[1]] };
            let pc = if t[2] == keep || t[2] == drop { new_pos } else { verts[t[2]] };
            let n = (pb - pa).cross(pc - pa);
            let len = n.len();
            if len < 1e-8 || (n.y / len) < 0.08 {
                ok = false;
                break;
            }
        }
        if !ok {
            continue;
        }

        // Apply edge collapse: drop -> keep
        verts[keep] = new_pos;
        quadrics[keep] = quadrics[keep].add(&quadrics[drop]);
        v_alive[drop] = false;
        v_ver[keep] = v_ver[keep].wrapping_add(1);

        let drop_tris = std::mem::take(&mut v_tris[drop]);
        for ti in drop_tris {
            if !tri_alive[ti] {
                continue;
            }
            let t = &mut tris[ti];
            if t[0] == keep || t[1] == keep || t[2] == keep {
                tri_alive[ti] = false;
                alive_tris -= 1;
            } else {
                for slot in t.iter_mut() {
                    if *slot == drop {
                        *slot = keep;
                    }
                }
                v_tris[keep].push(ti);
            }
        }
        v_tris[keep].retain(|&ti| tri_alive[ti]);

        // Re-score incident edges around `keep`
        let mut neighbors: Vec<usize> = Vec::new();
        for &ti in &v_tris[keep] {
            for &vi in &tris[ti] {
                if vi != keep && v_alive[vi] && !neighbors.contains(&vi) {
                    neighbors.push(vi);
                }
            }
        }
        for nb in neighbors {
            if let Some(nc) = make_cand(keep, nb, verts, &quadrics, &v_ver) {
                heap.push(nc);
            }
        }
    }

    let mut compacted = Vec::with_capacity(alive_tris);
    for (ti, &t) in tris.iter().enumerate() {
        if tri_alive[ti] {
            compacted.push(t);
        }
    }
    *tris = compacted;
}
