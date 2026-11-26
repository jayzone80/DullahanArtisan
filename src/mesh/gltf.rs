use std::collections::{HashMap, HashSet};

use crate::common::base64;
use crate::common::json::{obj, JsonVal};
use crate::common::math::{Aabb, Mat4, Vec3};
use crate::mesh::anim::{build_cubicspline_triples, eval_vec3_track, InterpMode, TrackKey};
use crate::mesh::geom::{ChildNodeMesh, Mesh, Triangle};
use crate::mesh::morph::{evaluate_morph_weights, MorphTarget};
use crate::mesh::palette::Palette;
use crate::mesh::skel::{
    compute_auto_skinning, evaluate_skeleton_pose, resolve_ik_chain_bones, VertexSkin,
};

fn quat_mul(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let [ax, ay, az, aw] = a;
    let [bx, by, bz, bw] = b;
    [
        aw * bx + ax * bw + ay * bz - az * by,
        aw * by - ax * bz + ay * bw + az * bx,
        aw * bz + ax * by - ay * bx + az * bw,
        aw * bw - ax * bx - ay * by - az * bz,
    ]
}

fn quat_from_euler_xyz_deg(rot: Vec3) -> [f32; 4] {
    let (sx, cx) = (rot.x.to_radians() * 0.5).sin_cos();
    let (sy, cy) = (rot.y.to_radians() * 0.5).sin_cos();
    let (sz, cz) = (rot.z.to_radians() * 0.5).sin_cos();
    quat_mul(quat_mul([0.0, 0.0, sz, cz], [0.0, sy, 0.0, cy]), [sx, 0.0, 0.0, cx])
}

#[derive(Debug, Clone)]
pub struct ExportStats {
    pub draw_calls: usize,
    pub has_translucent: bool,
    pub total_tris: usize,
    pub total_verts: usize,
}

/// Export mesh + 3 PBR atlases to standalone `.gltf` (JSON with embedded base64 or external URIs).
pub fn export_gltf_json(
    mesh: &Mesh,
    palette: &Palette,
    name: &str,
    external_palette_prefix: Option<&str>,
) -> Result<(String, ExportStats), String> {
    let (json, _bin, stats) = build_gltf_payload(mesh, palette, name, false, external_palette_prefix)?;
    Ok((json.to_string_compact(), stats))
}

/// Export mesh + 3 PBR atlases to binary `.glb` (glTF 2.0 binary container with embedded PNG bufferViews).
pub fn export_glb(
    mesh: &Mesh,
    palette: &Palette,
    name: &str,
    external_palette_prefix: Option<&str>,
) -> Result<(Vec<u8>, ExportStats), String> {
    let (json, mut bin, stats) = build_gltf_payload(mesh, palette, name, true, external_palette_prefix)?;
    let mut json_bytes = json.to_string_compact().into_bytes();
    while json_bytes.len() % 4 != 0 {
        json_bytes.push(b' ');
    }
    while bin.len() % 4 != 0 {
        bin.push(0u8);
    }

    let total_len = 12 + 8 + json_bytes.len() + 8 + bin.len();
    let mut glb = Vec::with_capacity(total_len);
    // 12-byte GLB header: magic "glTF", version 2, total byte length
    glb.extend_from_slice(&0x4654_6C67u32.to_le_bytes());
    glb.extend_from_slice(&2u32.to_le_bytes());
    glb.extend_from_slice(&(total_len as u32).to_le_bytes());

    // Chunk 0: JSON (0x4E4F534A)
    glb.extend_from_slice(&(json_bytes.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x4E4F_534Au32.to_le_bytes());
    glb.extend_from_slice(&json_bytes);

    // Chunk 1: BIN (0x004E4942)
    glb.extend_from_slice(&(bin.len() as u32).to_le_bytes());
    glb.extend_from_slice(&0x004E_4942u32.to_le_bytes());
    glb.extend_from_slice(&bin);

    Ok((glb, stats))
}

fn pad4(buf: &mut Vec<u8>) {
    while buf.len() % 4 != 0 {
        buf.push(0);
    }
}

fn append_primitive_buffers(
    tris: &[&Triangle],
    include_tangents: bool,
    skins: Option<&[[VertexSkin; 3]]>,
    morph_deltas: Option<&[(Vec<[Vec3; 3]>, Vec<[Vec3; 3]>)]>,
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> JsonVal {
    let vert_count = tris.len() * 3;
    let mut pos_bytes = Vec::with_capacity(vert_count * 12);
    let mut norm_bytes = Vec::with_capacity(vert_count * 12);
    let mut uv_bytes = Vec::with_capacity(vert_count * 8);
    let mut tan_bytes = if include_tangents { Vec::with_capacity(vert_count * 16) } else { Vec::new() };
    let mut bb = Aabb::empty();

    for t in tris {
        for (p, n, uv) in [(t.a, t.na, t.uv), (t.b, t.nb, t.uv_b), (t.c, t.nc, t.uv_c)] {
            bb.expand(p);
            pos_bytes.extend_from_slice(&p.x.to_le_bytes());
            pos_bytes.extend_from_slice(&p.y.to_le_bytes());
            pos_bytes.extend_from_slice(&p.z.to_le_bytes());
            let nn = n.normalized();
            norm_bytes.extend_from_slice(&nn.x.to_le_bytes());
            norm_bytes.extend_from_slice(&nn.y.to_le_bytes());
            norm_bytes.extend_from_slice(&nn.z.to_le_bytes());
            uv_bytes.extend_from_slice(&uv.x.to_le_bytes());
            uv_bytes.extend_from_slice(&uv.y.to_le_bytes());
            if include_tangents {
                let (tv, sgn) = t.compute_tangent(nn);
                tan_bytes.extend_from_slice(&tv.x.to_le_bytes());
                tan_bytes.extend_from_slice(&tv.y.to_le_bytes());
                tan_bytes.extend_from_slice(&tv.z.to_le_bytes());
                tan_bytes.extend_from_slice(&sgn.to_le_bytes());
            }
        }
    }

    let use_u32 = vert_count >= 65535;
    let mut idx_bytes = Vec::with_capacity(vert_count * if use_u32 { 4 } else { 2 });
    if use_u32 {
        for i in 0..vert_count as u32 {
            idx_bytes.extend_from_slice(&i.to_le_bytes());
        }
    } else {
        for i in 0..vert_count as u16 {
            idx_bytes.extend_from_slice(&i.to_le_bytes());
        }
    }

    // POSITION bufferView & accessor
    pad4(bin);
    let pos_off = bin.len();
    bin.extend_from_slice(&pos_bytes);
    let pos_bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(pos_off as i64)),
        ("byteLength", JsonVal::Int(pos_bytes.len() as i64)),
        ("target", JsonVal::Int(34962)),
    ]));
    let pos_acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(pos_bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(vert_count as i64)),
        ("type", JsonVal::Str("VEC3".into())),
        (
            "min",
            JsonVal::Arr(vec![
                JsonVal::Num(bb.min.x as f64),
                JsonVal::Num(bb.min.y as f64),
                JsonVal::Num(bb.min.z as f64),
            ]),
        ),
        (
            "max",
            JsonVal::Arr(vec![
                JsonVal::Num(bb.max.x as f64),
                JsonVal::Num(bb.max.y as f64),
                JsonVal::Num(bb.max.z as f64),
            ]),
        ),
    ]));

    // NORMAL bufferView & accessor
    pad4(bin);
    let norm_off = bin.len();
    bin.extend_from_slice(&norm_bytes);
    let norm_bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(norm_off as i64)),
        ("byteLength", JsonVal::Int(norm_bytes.len() as i64)),
        ("target", JsonVal::Int(34962)),
    ]));
    let norm_acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(norm_bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(vert_count as i64)),
        ("type", JsonVal::Str("VEC3".into())),
    ]));

    // TEXCOORD_0 bufferView & accessor
    pad4(bin);
    let uv_off = bin.len();
    bin.extend_from_slice(&uv_bytes);
    let uv_bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(uv_off as i64)),
        ("byteLength", JsonVal::Int(uv_bytes.len() as i64)),
        ("target", JsonVal::Int(34962)),
    ]));
    let uv_acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(uv_bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(vert_count as i64)),
        ("type", JsonVal::Str("VEC2".into())),
    ]));

    // INDICES bufferView & accessor
    pad4(bin);
    let idx_off = bin.len();
    bin.extend_from_slice(&idx_bytes);
    let idx_bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(idx_off as i64)),
        ("byteLength", JsonVal::Int(idx_bytes.len() as i64)),
        ("target", JsonVal::Int(34963)),
    ]));
    let idx_acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(idx_bv as i64)),
        ("componentType", JsonVal::Int(if use_u32 { 5125 } else { 5123 })),
        ("count", JsonVal::Int(vert_count as i64)),
        ("type", JsonVal::Str("SCALAR".into())),
    ]));

    let mut attr_fields = vec![
        ("POSITION", JsonVal::Int(pos_acc as i64)),
        ("NORMAL", JsonVal::Int(norm_acc as i64)),
        ("TEXCOORD_0", JsonVal::Int(uv_acc as i64)),
    ];

    if include_tangents {
        pad4(bin);
        let tan_off = bin.len();
        bin.extend_from_slice(&tan_bytes);
        let tan_bv = buffer_views.len();
        buffer_views.push(obj(vec![
            ("buffer", JsonVal::Int(0)),
            ("byteOffset", JsonVal::Int(tan_off as i64)),
            ("byteLength", JsonVal::Int(tan_bytes.len() as i64)),
            ("target", JsonVal::Int(34962)),
        ]));
        let tan_acc = accessors.len();
        accessors.push(obj(vec![
            ("bufferView", JsonVal::Int(tan_bv as i64)),
            ("componentType", JsonVal::Int(5126)),
            ("count", JsonVal::Int(vert_count as i64)),
            ("type", JsonVal::Str("VEC4".into())),
        ]));
        attr_fields.push(("TANGENT", JsonVal::Int(tan_acc as i64)));
    }

    if let Some(sk) = skins {
        let mut j_bytes = Vec::with_capacity(vert_count * 8);
        let mut w_bytes = Vec::with_capacity(vert_count * 16);
        for tri_sk in sk {
            for vs in tri_sk {
                for j in vs.joints {
                    j_bytes.extend_from_slice(&j.to_le_bytes());
                }
                for w in vs.weights {
                    w_bytes.extend_from_slice(&w.to_le_bytes());
                }
            }
        }
        pad4(bin);
        let j_off = bin.len();
        bin.extend_from_slice(&j_bytes);
        let j_bv = buffer_views.len();
        buffer_views.push(obj(vec![
            ("buffer", JsonVal::Int(0)),
            ("byteOffset", JsonVal::Int(j_off as i64)),
            ("byteLength", JsonVal::Int(j_bytes.len() as i64)),
            ("target", JsonVal::Int(34962)),
        ]));
        let j_acc = accessors.len();
        accessors.push(obj(vec![
            ("bufferView", JsonVal::Int(j_bv as i64)),
            ("componentType", JsonVal::Int(5123)), // UNSIGNED_SHORT
            ("count", JsonVal::Int(vert_count as i64)),
            ("type", JsonVal::Str("VEC4".into())),
        ]));

        pad4(bin);
        let w_off = bin.len();
        bin.extend_from_slice(&w_bytes);
        let w_bv = buffer_views.len();
        buffer_views.push(obj(vec![
            ("buffer", JsonVal::Int(0)),
            ("byteOffset", JsonVal::Int(w_off as i64)),
            ("byteLength", JsonVal::Int(w_bytes.len() as i64)),
            ("target", JsonVal::Int(34962)),
        ]));
        let w_acc = accessors.len();
        accessors.push(obj(vec![
            ("bufferView", JsonVal::Int(w_bv as i64)),
            ("componentType", JsonVal::Int(5126)), // FLOAT
            ("count", JsonVal::Int(vert_count as i64)),
            ("type", JsonVal::Str("VEC4".into())),
        ]));
        attr_fields.push(("JOINTS_0", JsonVal::Int(j_acc as i64)));
        attr_fields.push(("WEIGHTS_0", JsonVal::Int(w_acc as i64)));
    }

    let mut prim_fields = vec![
        ("attributes", obj(attr_fields)),
        ("indices", JsonVal::Int(idx_acc as i64)),
    ];

    if let Some(targets_data) = morph_deltas {
        if !targets_data.is_empty() {
            let mut gltf_targets = Vec::with_capacity(targets_data.len());
            for (dpos_tris, dnorm_tris) in targets_data {
                let mut dp_bytes = Vec::with_capacity(vert_count * 12);
                let mut dn_bytes = Vec::with_capacity(vert_count * 12);
                let mut dbb = Aabb::empty();
                for dp3 in dpos_tris {
                    for p in dp3 {
                        dbb.expand(*p);
                        dp_bytes.extend_from_slice(&p.x.to_le_bytes());
                        dp_bytes.extend_from_slice(&p.y.to_le_bytes());
                        dp_bytes.extend_from_slice(&p.z.to_le_bytes());
                    }
                }
                for dn3 in dnorm_tris {
                    for n in dn3 {
                        dn_bytes.extend_from_slice(&n.x.to_le_bytes());
                        dn_bytes.extend_from_slice(&n.y.to_le_bytes());
                        dn_bytes.extend_from_slice(&n.z.to_le_bytes());
                    }
                }
                pad4(bin);
                let dp_off = bin.len();
                bin.extend_from_slice(&dp_bytes);
                let dp_bv = buffer_views.len();
                buffer_views.push(obj(vec![
                    ("buffer", JsonVal::Int(0)),
                    ("byteOffset", JsonVal::Int(dp_off as i64)),
                    ("byteLength", JsonVal::Int(dp_bytes.len() as i64)),
                    ("target", JsonVal::Int(34962)),
                ]));
                let dp_acc = accessors.len();
                accessors.push(obj(vec![
                    ("bufferView", JsonVal::Int(dp_bv as i64)),
                    ("componentType", JsonVal::Int(5126)),
                    ("count", JsonVal::Int(vert_count as i64)),
                    ("type", JsonVal::Str("VEC3".into())),
                    (
                        "min",
                        JsonVal::Arr(vec![
                            JsonVal::Num(dbb.min.x as f64),
                            JsonVal::Num(dbb.min.y as f64),
                            JsonVal::Num(dbb.min.z as f64),
                        ]),
                    ),
                    (
                        "max",
                        JsonVal::Arr(vec![
                            JsonVal::Num(dbb.max.x as f64),
                            JsonVal::Num(dbb.max.y as f64),
                            JsonVal::Num(dbb.max.z as f64),
                        ]),
                    ),
                ]));

                pad4(bin);
                let dn_off = bin.len();
                bin.extend_from_slice(&dn_bytes);
                let dn_bv = buffer_views.len();
                buffer_views.push(obj(vec![
                    ("buffer", JsonVal::Int(0)),
                    ("byteOffset", JsonVal::Int(dn_off as i64)),
                    ("byteLength", JsonVal::Int(dn_bytes.len() as i64)),
                    ("target", JsonVal::Int(34962)),
                ]));
                let dn_acc = accessors.len();
                accessors.push(obj(vec![
                    ("bufferView", JsonVal::Int(dn_bv as i64)),
                    ("componentType", JsonVal::Int(5126)),
                    ("count", JsonVal::Int(vert_count as i64)),
                    ("type", JsonVal::Str("VEC3".into())),
                ]));

                gltf_targets.push(obj(vec![
                    ("POSITION", JsonVal::Int(dp_acc as i64)),
                    ("NORMAL", JsonVal::Int(dn_acc as i64)),
                ]));
            }
            prim_fields.push(("targets", JsonVal::Arr(gltf_targets)));
        }
    }

    obj(prim_fields)
}

fn append_mat4_f32_accessor(
    mats: &[Mat4],
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> usize {
    pad4(bin);
    let off = bin.len();
    for m in mats {
        for c in 0..4 {
            for r in 0..4 {
                bin.extend_from_slice(&m.m[c][r].to_le_bytes());
            }
        }
    }
    let bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(off as i64)),
        ("byteLength", JsonVal::Int((mats.len() * 64) as i64)),
    ]));
    let acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(mats.len() as i64)),
        ("type", JsonVal::Str("MAT4".into())),
    ]));
    acc
}

fn append_scalar_f32_accessor(
    values: &[f32],
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> usize {
    pad4(bin);
    let off = bin.len();
    let mut mn = f32::INFINITY;
    let mut mx = f32::NEG_INFINITY;
    for &v in values {
        mn = mn.min(v);
        mx = mx.max(v);
        bin.extend_from_slice(&v.to_le_bytes());
    }
    let bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(off as i64)),
        ("byteLength", JsonVal::Int((values.len() * 4) as i64)),
    ]));
    let acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(values.len() as i64)),
        ("type", JsonVal::Str("SCALAR".into())),
        ("min", JsonVal::Arr(vec![JsonVal::Num(mn as f64)])),
        ("max", JsonVal::Arr(vec![JsonVal::Num(mx as f64)])),
    ]));
    acc
}

fn append_vec3_f32_accessor(
    values: &[Vec3],
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> usize {
    pad4(bin);
    let off = bin.len();
    for v in values {
        bin.extend_from_slice(&v.x.to_le_bytes());
        bin.extend_from_slice(&v.y.to_le_bytes());
        bin.extend_from_slice(&v.z.to_le_bytes());
    }
    let bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(off as i64)),
        ("byteLength", JsonVal::Int((values.len() * 12) as i64)),
    ]));
    let acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(values.len() as i64)),
        ("type", JsonVal::Str("VEC3".into())),
    ]));
    acc
}

fn append_vec4_f32_accessor(
    values: &[[f32; 4]],
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> usize {
    pad4(bin);
    let off = bin.len();
    for v in values {
        for c in v {
            bin.extend_from_slice(&c.to_le_bytes());
        }
    }
    let bv = buffer_views.len();
    buffer_views.push(obj(vec![
        ("buffer", JsonVal::Int(0)),
        ("byteOffset", JsonVal::Int(off as i64)),
        ("byteLength", JsonVal::Int((values.len() * 16) as i64)),
    ]));
    let acc = accessors.len();
    accessors.push(obj(vec![
        ("bufferView", JsonVal::Int(bv as i64)),
        ("componentType", JsonVal::Int(5126)),
        ("count", JsonVal::Int(values.len() as i64)),
        ("type", JsonVal::Str("VEC4".into())),
    ]));
    acc
}

fn build_gltf_payload(
    mesh: &Mesh,
    palette: &Palette,
    name: &str,
    is_glb: bool,
    external_palette_prefix: Option<&str>,
) -> Result<(JsonVal, Vec<u8>, ExportStats), String> {
    if mesh.tris.is_empty() && mesh.nodes.is_empty() {
        return Err("cannot export empty mesh (0 triangles)".into());
    }

    fn node_has_pal_opaque(n: &ChildNodeMesh) -> bool {
        n.tris.iter().any(|t| t.tex_id.is_none() && !t.is_translucent)
            || n.children.iter().any(node_has_pal_opaque)
    }
    fn node_has_pal_trans(n: &ChildNodeMesh) -> bool {
        n.tris.iter().any(|t| t.tex_id.is_none() && t.is_translucent)
            || n.children.iter().any(node_has_pal_trans)
    }
    let any_pal_opaque = mesh.tris.iter().any(|t| t.tex_id.is_none() && !t.is_translucent)
        || mesh.nodes.iter().any(node_has_pal_opaque)
        || mesh.colliders.iter().any(|c| !c.tris.is_empty());
    let has_translucent = mesh.tris.iter().any(|t| t.tex_id.is_none() && t.is_translucent)
        || mesh.nodes.iter().any(node_has_pal_trans);

    let mut bin: Vec<u8> = Vec::new();
    let mut buffer_views: Vec<JsonVal> = Vec::new();
    let mut accessors: Vec<JsonVal> = Vec::new();
    let mut materials: Vec<JsonVal> = Vec::new();
    let mut images: Vec<JsonVal> = Vec::new();
    let mut gltf_textures: Vec<JsonVal> = Vec::new();
    let mut gltf_samplers: Vec<JsonVal> = vec![obj(vec![
        ("magFilter", JsonVal::Int(9728)),
        ("minFilter", JsonVal::Int(9728)),
        ("wrapS", JsonVal::Int(33071)),
        ("wrapT", JsonVal::Int(33071)),
    ])];

    // 3 Palette Atlas images & textures (indices 0, 1, 2)
    if let Some(prefix) = external_palette_prefix {
        images.push(obj(vec![
            ("name", JsonVal::Str("palette_basecolor".into())),
            ("uri", JsonVal::Str(format!("{prefix}_basecolor.png"))),
        ]));
        images.push(obj(vec![
            ("name", JsonVal::Str("palette_orm".into())),
            ("uri", JsonVal::Str(format!("{prefix}_orm.png"))),
        ]));
        images.push(obj(vec![
            ("name", JsonVal::Str("palette_emissive".into())),
            ("uri", JsonVal::Str(format!("{prefix}_emissive.png"))),
        ]));
    } else {
        let pngs = [
            ("palette_basecolor", palette.encode_basecolor_png()),
            ("palette_orm", palette.encode_orm_png()),
            ("palette_emissive", palette.encode_emissive_png()),
        ];
        for (img_name, bytes) in pngs {
            if is_glb {
                pad4(&mut bin);
                let off = bin.len();
                bin.extend_from_slice(&bytes);
                let bv = buffer_views.len();
                buffer_views.push(obj(vec![
                    ("buffer", JsonVal::Int(0)),
                    ("byteOffset", JsonVal::Int(off as i64)),
                    ("byteLength", JsonVal::Int(bytes.len() as i64)),
                ]));
                images.push(obj(vec![
                    ("name", JsonVal::Str(img_name.into())),
                    ("mimeType", JsonVal::Str("image/png".into())),
                    ("bufferView", JsonVal::Int(bv as i64)),
                ]));
            } else {
                let uri = format!("data:image/png;base64,{}", base64::encode(&bytes));
                images.push(obj(vec![
                    ("name", JsonVal::Str(img_name.into())),
                    ("mimeType", JsonVal::Str("image/png".into())),
                    ("uri", JsonVal::Str(uri)),
                ]));
            }
        }
    }
    for src_i in 0..3 {
        gltf_textures.push(obj(vec![
            ("sampler", JsonVal::Int(0)),
            ("source", JsonVal::Int(src_i)),
        ]));
    }

    let make_mat = |mat_name: &str, alpha_mode: &str| -> JsonVal {
        obj(vec![
            ("name", JsonVal::Str(mat_name.into())),
            ("alphaMode", JsonVal::Str(alpha_mode.into())),
            (
                "pbrMetallicRoughness",
                obj(vec![
                    (
                        "baseColorFactor",
                        JsonVal::Arr(vec![
                            JsonVal::Num(1.0),
                            JsonVal::Num(1.0),
                            JsonVal::Num(1.0),
                            JsonVal::Num(1.0),
                        ]),
                    ),
                    ("baseColorTexture", obj(vec![("index", JsonVal::Int(0))])),
                    ("metallicFactor", JsonVal::Num(1.0)),
                    ("roughnessFactor", JsonVal::Num(1.0)),
                    ("metallicRoughnessTexture", obj(vec![("index", JsonVal::Int(1))])),
                ]),
            ),
            (
                "occlusionTexture",
                obj(vec![("index", JsonVal::Int(1)), ("strength", JsonVal::Num(1.0))]),
            ),
            ("emissiveTexture", obj(vec![("index", JsonVal::Int(2))])),
            (
                "emissiveFactor",
                JsonVal::Arr(vec![JsonVal::Num(1.0), JsonVal::Num(1.0), JsonVal::Num(1.0)]),
            ),
        ])
    };

    let opaque_mat_idx = if any_pal_opaque {
        let idx = materials.len();
        materials.push(make_mat("palette_opaque", "OPAQUE"));
        Some(idx)
    } else {
        None
    };
    let trans_mat_idx = if has_translucent {
        let idx = materials.len();
        materials.push(make_mat("palette_translucent", "BLEND"));
        Some(idx)
    } else {
        None
    };

    // Emit PBR textures & materials for any `tex <name> ...` materials referenced by the mesh!
    let mut tex_mat_indices: Vec<(usize, bool)> = Vec::new(); // (material_idx, has_normal_map)
    if !mesh.textures.is_empty() {
        let pbr_sampler = gltf_samplers.len() as i64;
        gltf_samplers.push(obj(vec![
            ("magFilter", JsonVal::Int(9729)), // LINEAR
            ("minFilter", JsonVal::Int(9987)), // LINEAR_MIPMAP_LINEAR
            ("wrapS", JsonVal::Int(10497)),    // REPEAT
            ("wrapT", JsonVal::Int(10497)),    // REPEAT
        ]));

        let mut push_png_tex = |img_name: String, bytes: &[u8]| -> i64 {
            let img_idx = images.len() as i64;
            if is_glb {
                pad4(&mut bin);
                let off = bin.len();
                bin.extend_from_slice(bytes);
                let bv = buffer_views.len();
                buffer_views.push(obj(vec![
                    ("buffer", JsonVal::Int(0)),
                    ("byteOffset", JsonVal::Int(off as i64)),
                    ("byteLength", JsonVal::Int(bytes.len() as i64)),
                ]));
                images.push(obj(vec![
                    ("name", JsonVal::Str(img_name)),
                    ("mimeType", JsonVal::Str("image/png".into())),
                    ("bufferView", JsonVal::Int(bv as i64)),
                ]));
            } else {
                let uri = format!("data:image/png;base64,{}", base64::encode(bytes));
                images.push(obj(vec![
                    ("name", JsonVal::Str(img_name)),
                    ("mimeType", JsonVal::Str("image/png".into())),
                    ("uri", JsonVal::Str(uri)),
                ]));
            }
            let tex_idx = gltf_textures.len() as i64;
            gltf_textures.push(obj(vec![
                ("sampler", JsonVal::Int(pbr_sampler)),
                ("source", JsonVal::Int(img_idx)),
            ]));
            tex_idx
        };

        for tm in &mesh.textures {
            let bc_tex = push_png_tex(format!("{}_basecolor", tm.name), &tm.basecolor_png);
            let orm_tex = tm
                .orm_png
                .as_ref()
                .map(|b| push_png_tex(format!("{}_orm", tm.name), b));
            let nm_tex = tm
                .normal_png
                .as_ref()
                .map(|b| push_png_tex(format!("{}_normal", tm.name), b));

            let mut pbr_fields = vec![
                (
                    "baseColorFactor",
                    JsonVal::Arr(vec![
                        JsonVal::Num(1.0),
                        JsonVal::Num(1.0),
                        JsonVal::Num(1.0),
                        JsonVal::Num(1.0),
                    ]),
                ),
                ("baseColorTexture", obj(vec![("index", JsonVal::Int(bc_tex))])),
            ];
            if let Some(ot) = orm_tex {
                pbr_fields.push(("metallicFactor", JsonVal::Num(1.0)));
                pbr_fields.push(("roughnessFactor", JsonVal::Num(1.0)));
                pbr_fields.push(("metallicRoughnessTexture", obj(vec![("index", JsonVal::Int(ot))])));
            } else {
                pbr_fields.push(("metallicFactor", JsonVal::Num(0.0)));
                pbr_fields.push(("roughnessFactor", JsonVal::Num(0.85)));
            }

            let mut mf = vec![
                ("name", JsonVal::Str(tm.name.clone())),
                ("alphaMode", JsonVal::Str("OPAQUE".into())),
                ("pbrMetallicRoughness", obj(pbr_fields)),
            ];
            if let Some(ot) = orm_tex {
                mf.push((
                    "occlusionTexture",
                    obj(vec![("index", JsonVal::Int(ot)), ("strength", JsonVal::Num(1.0))]),
                ));
            }
            if let Some(nt) = nm_tex {
                mf.push(("normalTexture", obj(vec![("index", JsonVal::Int(nt))])));
            }
            let mi = materials.len();
            materials.push(obj(mf));
            tex_mat_indices.push((mi, nm_tex.is_some()));
        }
    }

    let mut build_prims = |tris: &[Triangle],
                           all_skins: Option<&[[VertexSkin; 3]]>,
                           morphs: Option<&[MorphTarget]>|
     -> Vec<JsonVal> {
        let mut opq: Vec<&Triangle> = Vec::new();
        let mut opq_idx: Vec<usize> = Vec::new();
        let mut opq_sk: Vec<[VertexSkin; 3]> = Vec::new();
        let mut trn: Vec<&Triangle> = Vec::new();
        let mut trn_idx: Vec<usize> = Vec::new();
        let mut trn_sk: Vec<[VertexSkin; 3]> = Vec::new();
        let mut tex_buckets: Vec<(Vec<&Triangle>, Vec<usize>, Vec<[VertexSkin; 3]>)> =
            vec![(Vec::new(), Vec::new(), Vec::new()); tex_mat_indices.len()];

        for (i, t) in tris.iter().enumerate() {
            if let Some(tid) = t.tex_id {
                if let Some(bucket) = tex_buckets.get_mut(tid as usize) {
                    bucket.0.push(t);
                    bucket.1.push(i);
                    if let Some(sk) = all_skins {
                        bucket.2.push(sk[i]);
                    }
                    continue;
                }
            }
            if !t.is_translucent {
                opq.push(t);
                opq_idx.push(i);
                if let Some(sk) = all_skins {
                    opq_sk.push(sk[i]);
                }
            } else {
                trn.push(t);
                trn_idx.push(i);
                if let Some(sk) = all_skins {
                    trn_sk.push(sk[i]);
                }
            }
        }
        let slice_morphs = |idxs: &[usize]| -> Option<Vec<(Vec<[Vec3; 3]>, Vec<[Vec3; 3]>)>> {
            let ms = morphs?;
            if ms.is_empty() {
                return None;
            }
            let mut out = Vec::with_capacity(ms.len());
            for mt in ms {
                let dp: Vec<[Vec3; 3]> = idxs.iter().map(|&i| mt.delta_pos[i]).collect();
                let dn: Vec<[Vec3; 3]> = idxs.iter().map(|&i| mt.delta_norm[i]).collect();
                out.push((dp, dn));
            }
            Some(out)
        };
        let mut prims = Vec::new();
        if !opq.is_empty() {
            let sk_arg = all_skins.map(|_| opq_sk.as_slice());
            let m_arg = slice_morphs(&opq_idx);
            let mut p = append_primitive_buffers(
                &opq,
                false,
                sk_arg,
                m_arg.as_deref(),
                &mut bin,
                &mut buffer_views,
                &mut accessors,
            );
            if let (JsonVal::Obj(ref mut fields), Some(mi)) = (&mut p, opaque_mat_idx) {
                fields.push(("material".into(), JsonVal::Int(mi as i64)));
            }
            prims.push(p);
        }
        if !trn.is_empty() {
            let sk_arg = all_skins.map(|_| trn_sk.as_slice());
            let m_arg = slice_morphs(&trn_idx);
            let mut p = append_primitive_buffers(
                &trn,
                false,
                sk_arg,
                m_arg.as_deref(),
                &mut bin,
                &mut buffer_views,
                &mut accessors,
            );
            if let (JsonVal::Obj(ref mut fields), Some(mi)) = (&mut p, trans_mat_idx) {
                fields.push(("material".into(), JsonVal::Int(mi as i64)));
            }
            prims.push(p);
        }
        for (bi, (b_tris, b_idx, b_sk)) in tex_buckets.iter().enumerate() {
            if b_tris.is_empty() {
                continue;
            }
            let (mat_idx, has_nm) = tex_mat_indices[bi];
            let sk_arg = all_skins.map(|_| b_sk.as_slice());
            let m_arg = slice_morphs(b_idx);
            let mut p = append_primitive_buffers(
                b_tris,
                has_nm,
                sk_arg,
                m_arg.as_deref(),
                &mut bin,
                &mut buffer_views,
                &mut accessors,
            );
            if let JsonVal::Obj(ref mut fields) = p {
                fields.push(("material".into(), JsonVal::Int(mat_idx as i64)));
            }
            prims.push(p);
        }
        prims
    };

    let mut gltf_meshes: Vec<JsonVal> = Vec::new();
    let mut all_nodes: Vec<JsonVal> = vec![JsonVal::Null]; // slot 0 reserved for root node
    let mut root_children: Vec<JsonVal> = Vec::new();
    let mut node_index_map: HashMap<String, usize> = HashMap::new();
    node_index_map.insert(name.to_string(), 0);
    node_index_map.insert("root".to_string(), 0);
    let mut total_draw_calls = 0usize;
    let mut total_tris = 0usize;

    let has_skin = !mesh.bones.is_empty();
    let root_skins_owned = if has_skin && !mesh.tris.is_empty() {
        Some(compute_auto_skinning(&mesh.tris, &mesh.bones))
    } else {
        None
    };

    let root_mesh_idx = if !mesh.tris.is_empty() {
        let m_arg = if mesh.morphs.is_empty() {
            None
        } else {
            Some(mesh.morphs.as_slice())
        };
        let prims = build_prims(&mesh.tris, root_skins_owned.as_deref(), m_arg);
        total_draw_calls += prims.len();
        total_tris += mesh.tris.len();
        let idx = gltf_meshes.len() as i64;
        let mut mf = vec![
            ("name", JsonVal::Str(name.into())),
            ("primitives", JsonVal::Arr(prims)),
        ];
        if !mesh.morphs.is_empty() {
            mf.push((
                "weights",
                JsonVal::Arr(mesh.morphs.iter().map(|m| JsonVal::Num(m.default_weight as f64)).collect()),
            ));
            mf.push((
                "extras",
                obj(vec![(
                    "targetNames",
                    JsonVal::Arr(mesh.morphs.iter().map(|m| JsonVal::Str(m.name.clone())).collect()),
                )]),
            ));
        }
        gltf_meshes.push(obj(mf));
        Some(idx)
    } else {
        None
    };

    // Track which glTF node holds the root mesh (needed for glTF `weights` animation channel!)
    let mut root_mesh_node_idx: usize = 0;

    // If skinned, place skinned mesh on a sibling child node of the joint tree (cleanest glTF 2.0 hierarchy!)
    let mut gltf_skins: Vec<JsonVal> = Vec::new();
    let mut pending_skin: Option<(Vec<JsonVal>, Vec<Mat4>, Option<usize>)> = None;
    let mut attach_root_mesh_to_root = true;
    if has_skin {
        if let Some(rmi) = root_mesh_idx {
            let sm_idx = all_nodes.len();
            root_mesh_node_idx = sm_idx;
            all_nodes.push(obj(vec![
                ("name", JsonVal::Str(format!("{name}_mesh"))),
                ("mesh", JsonVal::Int(rmi)),
                ("skin", JsonVal::Int(0)),
            ]));
            root_children.push(JsonVal::Int(sm_idx as i64));
            attach_root_mesh_to_root = false;
        }
        // Emit joint hierarchy nodes matching `mesh.bones`
        let base_joint_node = all_nodes.len();
        let mut bone_children: Vec<Vec<usize>> = vec![Vec::new(); mesh.bones.len()];
        let mut first_root_joint: Option<usize> = None;
        for (bi, b) in mesh.bones.iter().enumerate() {
            let n_idx = base_joint_node + bi;
            node_index_map.insert(b.name.clone(), n_idx);
            if let Some(pi) = b.parent {
                bone_children[pi].push(n_idx);
            } else {
                if first_root_joint.is_none() {
                    first_root_joint = Some(n_idx);
                }
                root_children.push(JsonVal::Int(n_idx as i64));
            }
        }
        let mut joint_node_ids = Vec::with_capacity(mesh.bones.len());
        let mut ibms = Vec::with_capacity(mesh.bones.len());
        for (bi, b) in mesh.bones.iter().enumerate() {
            let t = b.rest_local_translation(&mesh.bones);
            let mut jf = vec![("name", JsonVal::Str(b.name.clone()))];
            if t.len_sq() > 1e-12 {
                jf.push((
                    "translation",
                    JsonVal::Arr(vec![
                        JsonVal::Num(t.x as f64),
                        JsonVal::Num(t.y as f64),
                        JsonVal::Num(t.z as f64),
                    ]),
                ));
            }
            if !bone_children[bi].is_empty() {
                jf.push((
                    "children",
                    JsonVal::Arr(bone_children[bi].iter().map(|&c| JsonVal::Int(c as i64)).collect()),
                ));
            }
            all_nodes.push(obj(jf));
            joint_node_ids.push(JsonVal::Int((base_joint_node + bi) as i64));
            ibms.push(Mat4::translation(-b.head));
        }
        pending_skin = Some((joint_node_ids, ibms, first_root_joint));
    }

    // Recursively emit named visual child node hierarchy (supporting nested nodes & pure pivot nodes)
    #[allow(clippy::too_many_arguments)]
    fn emit_child_node<F>(
        cnode: &ChildNodeMesh,
        build_prims: &mut F,
        gltf_meshes: &mut Vec<JsonVal>,
        all_nodes: &mut Vec<JsonVal>,
        node_index_map: &mut HashMap<String, usize>,
        total_draw_calls: &mut usize,
        total_tris: &mut usize,
    ) -> usize
    where
        F: FnMut(&[Triangle], Option<&[[VertexSkin; 3]]>, Option<&[MorphTarget]>) -> Vec<JsonVal>,
    {
        let my_idx = all_nodes.len();
        all_nodes.push(JsonVal::Null);
        node_index_map.insert(cnode.name.clone(), my_idx);

        let mut nf = vec![("name", JsonVal::Str(cnode.name.clone()))];
        if !cnode.tris.is_empty() {
            let prims = build_prims(&cnode.tris, None, None);
            *total_draw_calls += prims.len();
            *total_tris += cnode.tris.len();
            let mesh_idx = gltf_meshes.len() as i64;
            gltf_meshes.push(obj(vec![
                ("name", JsonVal::Str(cnode.name.clone())),
                ("primitives", JsonVal::Arr(prims)),
            ]));
            nf.push(("mesh", JsonVal::Int(mesh_idx)));
        }
        if cnode.translation.len_sq() > 1e-12 {
            nf.push((
                "translation",
                JsonVal::Arr(vec![
                    JsonVal::Num(cnode.translation.x as f64),
                    JsonVal::Num(cnode.translation.y as f64),
                    JsonVal::Num(cnode.translation.z as f64),
                ]),
            ));
        }
        if cnode.rotation_deg.len_sq() > 1e-12 {
            let q = quat_from_euler_xyz_deg(cnode.rotation_deg);
            nf.push((
                "rotation",
                JsonVal::Arr(vec![
                    JsonVal::Num(q[0] as f64),
                    JsonVal::Num(q[1] as f64),
                    JsonVal::Num(q[2] as f64),
                    JsonVal::Num(q[3] as f64),
                ]),
            ));
        }
        if (cnode.scale.x - 1.0).abs() > 1e-6
            || (cnode.scale.y - 1.0).abs() > 1e-6
            || (cnode.scale.z - 1.0).abs() > 1e-6
        {
            nf.push((
                "scale",
                JsonVal::Arr(vec![
                    JsonVal::Num(cnode.scale.x as f64),
                    JsonVal::Num(cnode.scale.y as f64),
                    JsonVal::Num(cnode.scale.z as f64),
                ]),
            ));
        }
        if !cnode.children.is_empty() {
            let mut ch_idx = Vec::with_capacity(cnode.children.len());
            for ch in &cnode.children {
                let ci = emit_child_node(
                    ch,
                    build_prims,
                    gltf_meshes,
                    all_nodes,
                    node_index_map,
                    total_draw_calls,
                    total_tris,
                );
                ch_idx.push(JsonVal::Int(ci as i64));
            }
            nf.push(("children", JsonVal::Arr(ch_idx)));
        }
        all_nodes[my_idx] = obj(nf);
        my_idx
    }

    for cnode in &mesh.nodes {
        let idx = emit_child_node(
            cnode,
            &mut build_prims,
            &mut gltf_meshes,
            &mut all_nodes,
            &mut node_index_map,
            &mut total_draw_calls,
            &mut total_tris,
        );
        root_children.push(JsonVal::Int(idx as i64));
    }

    if let Some((joint_node_ids, ibms, first_root_joint)) = pending_skin {
        let ibm_acc = append_mat4_f32_accessor(&ibms, &mut bin, &mut buffer_views, &mut accessors);
        let mut skin_fields = vec![
            ("name", JsonVal::Str(format!("{name}_rig"))),
            ("joints", JsonVal::Arr(joint_node_ids)),
            ("inverseBindMatrices", JsonVal::Int(ibm_acc as i64)),
        ];
        if let Some(skel_root) = first_root_joint {
            skin_fields.push(("skeleton", JsonVal::Int(skel_root as i64)));
        }
        gltf_skins.push(obj(skin_fields));
    }

    for col in &mesh.colliders {
        if col.tris.is_empty() {
            continue;
        }
        let col_refs: Vec<&Triangle> = col.tris.iter().collect();
        let mut col_prim = append_primitive_buffers(
            &col_refs,
            false,
            None,
            None,
            &mut bin,
            &mut buffer_views,
            &mut accessors,
        );
        if let JsonVal::Obj(ref mut map) = col_prim {
            map.push(("material".into(), JsonVal::Int(opaque_mat_idx.unwrap_or(0) as i64)));
        }
        let mesh_idx = gltf_meshes.len() as i64;
        gltf_meshes.push(obj(vec![
            ("name", JsonVal::Str(col.name.clone())),
            ("primitives", JsonVal::Arr(vec![col_prim])),
        ]));
        let node_idx = all_nodes.len() as i64;
        root_children.push(JsonVal::Int(node_idx));
        all_nodes.push(obj(vec![
            ("name", JsonVal::Str(col.name.clone())),
            ("mesh", JsonVal::Int(mesh_idx)),
        ]));
    }

    let mut root_node_fields = vec![("name", JsonVal::Str(name.into()))];
    if attach_root_mesh_to_root {
        if let Some(rmi) = root_mesh_idx {
            root_node_fields.push(("mesh", JsonVal::Int(rmi)));
        }
    }
    if !root_children.is_empty() {
        root_node_fields.push(("children", JsonVal::Arr(root_children)));
    }
    all_nodes[0] = obj(root_node_fields);

    // Export glTF 2.0 `animations` + `extras.events` metadata
    let mut gltf_animations: Vec<JsonVal> = Vec::new();
    for action in &mesh.actions {
        let tracks_map = action.collect_tracks();
        let mut target_names: Vec<String> = tracks_map.keys().cloned().collect();
        target_names.sort();

        let mut channels: Vec<JsonVal> = Vec::new();
        let mut samplers: Vec<JsonVal> = Vec::new();

        let emit_vec3_channel = |keys: &[TrackKey],
                                 node_idx: usize,
                                 path: &str,
                                 bin: &mut Vec<u8>,
                                 buffer_views: &mut Vec<JsonVal>,
                                 accessors: &mut Vec<JsonVal>,
                                 samplers: &mut Vec<JsonVal>,
                                 channels: &mut Vec<JsonVal>| {
            if keys.is_empty() {
                return;
            }
            let all_step = keys.len() > 1 && keys[..keys.len() - 1].iter().all(|k| k.interp == InterpMode::Step);
            let all_lin = keys.len() == 1 || keys[..keys.len() - 1].iter().all(|k| k.interp == InterpMode::Linear);

            let (interp_str, in_acc, out_acc) = if all_step {
                let times: Vec<f32> = keys.iter().map(|k| k.time).collect();
                let vals: Vec<Vec3> = keys.iter().map(|k| k.value).collect();
                (
                    "STEP",
                    append_scalar_f32_accessor(&times, bin, buffer_views, accessors),
                    append_vec3_f32_accessor(&vals, bin, buffer_views, accessors),
                )
            } else if all_lin {
                let times: Vec<f32> = keys.iter().map(|k| k.time).collect();
                let vals: Vec<Vec3> = keys.iter().map(|k| k.value).collect();
                (
                    "LINEAR",
                    append_scalar_f32_accessor(&times, bin, buffer_views, accessors),
                    append_vec3_f32_accessor(&vals, bin, buffer_views, accessors),
                )
            } else {
                // Native glTF CUBICSPLINE (exact Hermite in_tangent, value, out_tangent triples!)
                let times: Vec<f32> = keys.iter().map(|k| k.time).collect();
                let triples = build_cubicspline_triples(keys);
                let mut flat = Vec::with_capacity(triples.len() * 3);
                for (it, v, ot) in triples {
                    flat.push(it);
                    flat.push(v);
                    flat.push(ot);
                }
                (
                    "CUBICSPLINE",
                    append_scalar_f32_accessor(&times, bin, buffer_views, accessors),
                    append_vec3_f32_accessor(&flat, bin, buffer_views, accessors),
                )
            };

            let s_idx = samplers.len() as i64;
            samplers.push(obj(vec![
                ("input", JsonVal::Int(in_acc as i64)),
                ("interpolation", JsonVal::Str(interp_str.into())),
                ("output", JsonVal::Int(out_acc as i64)),
            ]));
            channels.push(obj(vec![
                ("sampler", JsonVal::Int(s_idx)),
                (
                    "target",
                    obj(vec![
                        ("node", JsonVal::Int(node_idx as i64)),
                        ("path", JsonVal::Str(path.into())),
                    ]),
                ),
            ]));
        };

        // Identify bones driven by IK chains so we bake their solved FK quaternions directly into glTF!
        let mut ik_driven_bones: HashSet<usize> = HashSet::new();
        if !action.ik_keys.is_empty() && !mesh.bones.is_empty() {
            for ik in &action.ik_keys {
                if let Some(chain) = resolve_ik_chain_bones(&mesh.bones, &ik.effector, ik.chain_len, ik.use_tip) {
                    for bi in chain {
                        ik_driven_bones.insert(bi);
                    }
                }
            }
        }
        let ik_driven_names: HashSet<String> = ik_driven_bones
            .iter()
            .map(|&bi| mesh.bones[bi].name.clone())
            .collect();

        for tname in &target_names {
            let Some(&node_idx) = node_index_map.get(tname) else {
                return Err(format!(
                    "anim '{}' targets unknown node/bone '{}' (define it with `node` or `skel`)",
                    action.name, tname
                ));
            };
            let tr = &tracks_map[tname];
            emit_vec3_channel(
                &tr.pos,
                node_idx,
                "translation",
                &mut bin,
                &mut buffer_views,
                &mut accessors,
                &mut samplers,
                &mut channels,
            );
            emit_vec3_channel(
                &tr.scale,
                node_idx,
                "scale",
                &mut bin,
                &mut buffer_views,
                &mut accessors,
                &mut samplers,
                &mut channels,
            );
            if !tr.rot.is_empty() && !ik_driven_names.contains(tname) {
                let all_step = tr.rot.len() > 1
                    && tr.rot[..tr.rot.len() - 1].iter().all(|k| k.interp == InterpMode::Step);
                let all_lin = tr.rot.len() == 1
                    || tr.rot[..tr.rot.len() - 1].iter().all(|k| k.interp == InterpMode::Linear);
                let mut times = Vec::new();
                let mut quats: Vec<[f32; 4]> = Vec::new();
                let interp_str = if all_step {
                    for k in &tr.rot {
                        times.push(k.time);
                        quats.push(quat_from_euler_xyz_deg(k.value));
                    }
                    "STEP"
                } else if all_lin {
                    for k in &tr.rot {
                        times.push(k.time);
                        quats.push(quat_from_euler_xyz_deg(k.value));
                    }
                    "LINEAR"
                } else {
                    // Dense FPS-sampled LINEAR quaternions reproduce exact ease/cubic curves across all engines
                    let t_start = tr.rot[0].time;
                    let t_end = tr.rot.last().unwrap().time;
                    let steps = (((t_end - t_start) * action.fps).ceil() as usize).max(2);
                    for s in 0..=steps {
                        let t = t_start + (t_end - t_start) * (s as f32 / steps as f32);
                        let deg = eval_vec3_track(&tr.rot, t, Vec3::ZERO);
                        times.push(t);
                        quats.push(quat_from_euler_xyz_deg(deg));
                    }
                    "LINEAR"
                };
                // Enforce shortest-arc quaternion hemisphere continuity
                for k in 1..quats.len() {
                    let dot = quats[k - 1][0] * quats[k][0]
                        + quats[k - 1][1] * quats[k][1]
                        + quats[k - 1][2] * quats[k][2]
                        + quats[k - 1][3] * quats[k][3];
                    if dot < 0.0 {
                        for c in &mut quats[k] {
                            *c = -*c;
                        }
                    }
                }
                let in_acc = append_scalar_f32_accessor(&times, &mut bin, &mut buffer_views, &mut accessors);
                let out_acc = append_vec4_f32_accessor(&quats, &mut bin, &mut buffer_views, &mut accessors);
                let s_idx = samplers.len() as i64;
                samplers.push(obj(vec![
                    ("input", JsonVal::Int(in_acc as i64)),
                    ("interpolation", JsonVal::Str(interp_str.into())),
                    ("output", JsonVal::Int(out_acc as i64)),
                ]));
                channels.push(obj(vec![
                    ("sampler", JsonVal::Int(s_idx)),
                    (
                        "target",
                        obj(vec![
                            ("node", JsonVal::Int(node_idx as i64)),
                            ("path", JsonVal::Str("rotation".into())),
                        ]),
                    ),
                ]));
            }
        }

        // Bake IK-driven bone rotations into native FPS-sampled glTF LINEAR quaternion tracks
        if !ik_driven_bones.is_empty() {
            let steps = ((action.duration * action.fps).ceil() as usize).max(2);
            let mut times = Vec::with_capacity(steps + 1);
            let mut bone_list: Vec<usize> = ik_driven_bones.into_iter().collect();
            bone_list.sort();
            let mut per_bone_q: Vec<Vec<[f32; 4]>> =
                vec![Vec::with_capacity(steps + 1); bone_list.len()];
            for s in 0..=steps {
                let t = action.duration * (s as f32 / steps as f32);
                times.push(t);
                let pose = evaluate_skeleton_pose(&mesh.bones, action, t);
                for (k, &bi) in bone_list.iter().enumerate() {
                    let mut q = pose.local_rot[bi].normalized().array();
                    if let Some(prev) = per_bone_q[k].last() {
                        let dot = prev[0] * q[0] + prev[1] * q[1] + prev[2] * q[2] + prev[3] * q[3];
                        if dot < 0.0 {
                            for c in &mut q {
                                *c = -*c;
                            }
                        }
                    }
                    per_bone_q[k].push(q);
                }
            }
            let in_acc = append_scalar_f32_accessor(&times, &mut bin, &mut buffer_views, &mut accessors);
            for (k, &bi) in bone_list.iter().enumerate() {
                let Some(&node_idx) = node_index_map.get(&mesh.bones[bi].name) else {
                    continue;
                };
                let out_acc =
                    append_vec4_f32_accessor(&per_bone_q[k], &mut bin, &mut buffer_views, &mut accessors);
                let s_idx = samplers.len() as i64;
                samplers.push(obj(vec![
                    ("input", JsonVal::Int(in_acc as i64)),
                    ("interpolation", JsonVal::Str("LINEAR".into())),
                    ("output", JsonVal::Int(out_acc as i64)),
                ]));
                channels.push(obj(vec![
                    ("sampler", JsonVal::Int(s_idx)),
                    (
                        "target",
                        obj(vec![
                            ("node", JsonVal::Int(node_idx as i64)),
                            ("path", JsonVal::Str("rotation".into())),
                        ]),
                    ),
                ]));
            }
        }

        // Export glTF 2.0 `weights` channel for animated morph targets (`mw` keyframes)
        if !action.morph_keys.is_empty() {
            if mesh.morphs.is_empty() {
                return Err(format!(
                    "anim '{}' has morph keyframes (`mw`) but no `morph ... end` blocks are defined",
                    action.name
                ));
            }
            for mk in &action.morph_keys {
                if !mesh.morphs.iter().any(|m| m.name == mk.target) {
                    return Err(format!(
                        "anim '{}' references unknown morph target '{}'",
                        action.name, mk.target
                    ));
                }
            }
            let steps = ((action.duration * action.fps).ceil() as usize).max(2);
            let mut times = Vec::with_capacity(steps + 1);
            let mut flat_weights = Vec::with_capacity((steps + 1) * mesh.morphs.len());
            for s in 0..=steps {
                let t = action.duration * (s as f32 / steps as f32);
                times.push(t);
                flat_weights.extend(evaluate_morph_weights(&mesh.morphs, action, t));
            }
            let in_acc = append_scalar_f32_accessor(&times, &mut bin, &mut buffer_views, &mut accessors);
            let out_acc = append_scalar_f32_accessor(&flat_weights, &mut bin, &mut buffer_views, &mut accessors);
            let s_idx = samplers.len() as i64;
            samplers.push(obj(vec![
                ("input", JsonVal::Int(in_acc as i64)),
                ("interpolation", JsonVal::Str("LINEAR".into())),
                ("output", JsonVal::Int(out_acc as i64)),
            ]));
            channels.push(obj(vec![
                ("sampler", JsonVal::Int(s_idx)),
                (
                    "target",
                    obj(vec![
                        ("node", JsonVal::Int(root_mesh_node_idx as i64)),
                        ("path", JsonVal::Str("weights".into())),
                    ]),
                ),
            ]));
        }

        let mut ev_json = Vec::new();
        for ev in &action.events {
            let mut ef = vec![
                ("time", JsonVal::Num(ev.time as f64)),
                ("name", JsonVal::Str(ev.name.clone())),
            ];
            if let Some(ref d) = ev.data {
                ef.push(("data", JsonVal::Str(d.clone())));
            }
            ev_json.push(obj(ef));
        }
        let mut extras_fields = vec![
            ("duration", JsonVal::Num(action.duration as f64)),
            ("fps", JsonVal::Num(action.fps as f64)),
            ("loop", JsonVal::Bool(action.looping)),
        ];
        if !ev_json.is_empty() {
            extras_fields.push(("events", JsonVal::Arr(ev_json)));
        }

        gltf_animations.push(obj(vec![
            ("name", JsonVal::Str(action.name.clone())),
            ("channels", JsonVal::Arr(channels)),
            ("samplers", JsonVal::Arr(samplers)),
            ("extras", obj(extras_fields)),
        ]));
    }

    pad4(&mut bin);
    let buffer_obj = if is_glb {
        obj(vec![("byteLength", JsonVal::Int(bin.len() as i64))])
    } else {
        let uri = format!("data:application/octet-stream;base64,{}", base64::encode(&bin));
        obj(vec![
            ("byteLength", JsonVal::Int(bin.len() as i64)),
            ("uri", JsonVal::Str(uri)),
        ])
    };

    let mut root_fields = vec![
        (
            "asset",
            obj(vec![
                ("version", JsonVal::Str("2.0".into())),
                ("generator", JsonVal::Str("dullahan-artisan".into())),
            ]),
        ),
        ("scene", JsonVal::Int(0)),
        (
            "scenes",
            JsonVal::Arr(vec![obj(vec![(
                "nodes",
                JsonVal::Arr(vec![JsonVal::Int(0)]),
            )])]),
        ),
        ("nodes", JsonVal::Arr(all_nodes)),
        ("meshes", JsonVal::Arr(gltf_meshes)),
        ("materials", JsonVal::Arr(materials)),
        ("samplers", JsonVal::Arr(gltf_samplers)),
        ("textures", JsonVal::Arr(gltf_textures)),
        ("images", JsonVal::Arr(images)),
        ("buffers", JsonVal::Arr(vec![buffer_obj])),
        ("bufferViews", JsonVal::Arr(buffer_views)),
        ("accessors", JsonVal::Arr(accessors)),
    ];
    if !gltf_skins.is_empty() {
        root_fields.push(("skins", JsonVal::Arr(gltf_skins)));
    }
    if !gltf_animations.is_empty() {
        root_fields.push(("animations", JsonVal::Arr(gltf_animations)));
    }
    let root = obj(root_fields);

    let stats = ExportStats {
        draw_calls: total_draw_calls,
        has_translucent,
        total_tris,
        total_verts: total_tris * 3,
    };
    Ok((root, bin, stats))
}
