use crate::common::base64;
use crate::common::json::{obj, JsonVal};
use crate::common::math::{Aabb, Vec3};
use crate::mesh::geom::{Mesh, Triangle};
use crate::mesh::palette::Palette;

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
    bin: &mut Vec<u8>,
    buffer_views: &mut Vec<JsonVal>,
    accessors: &mut Vec<JsonVal>,
) -> JsonVal {
    let vert_count = tris.len() * 3;
    let mut pos_bytes = Vec::with_capacity(vert_count * 12);
    let mut norm_bytes = Vec::with_capacity(vert_count * 12);
    let mut uv_bytes = Vec::with_capacity(vert_count * 8);
    let mut bb = Aabb::empty();

    for t in tris {
        for (p, n) in [(t.a, t.na), (t.b, t.nb), (t.c, t.nc)] {
            bb.expand(p);
            pos_bytes.extend_from_slice(&p.x.to_le_bytes());
            pos_bytes.extend_from_slice(&p.y.to_le_bytes());
            pos_bytes.extend_from_slice(&p.z.to_le_bytes());
            let nn = n.normalized();
            norm_bytes.extend_from_slice(&nn.x.to_le_bytes());
            norm_bytes.extend_from_slice(&nn.y.to_le_bytes());
            norm_bytes.extend_from_slice(&nn.z.to_le_bytes());
            uv_bytes.extend_from_slice(&t.uv.x.to_le_bytes());
            uv_bytes.extend_from_slice(&t.uv.y.to_le_bytes());
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

    obj(vec![
        (
            "attributes",
            obj(vec![
                ("POSITION", JsonVal::Int(pos_acc as i64)),
                ("NORMAL", JsonVal::Int(norm_acc as i64)),
                ("TEXCOORD_0", JsonVal::Int(uv_acc as i64)),
            ]),
        ),
        ("indices", JsonVal::Int(idx_acc as i64)),
    ])
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

    let any_opaque = mesh.tris.iter().any(|t| !t.is_translucent)
        || mesh.nodes.iter().any(|n| n.tris.iter().any(|t| !t.is_translucent))
        || mesh.colliders.iter().any(|c| !c.tris.is_empty());
    let has_translucent = mesh.tris.iter().any(|t| t.is_translucent)
        || mesh.nodes.iter().any(|n| n.tris.iter().any(|t| t.is_translucent));

    let mut bin: Vec<u8> = Vec::new();
    let mut buffer_views: Vec<JsonVal> = Vec::new();
    let mut accessors: Vec<JsonVal> = Vec::new();
    let mut materials: Vec<JsonVal> = Vec::new();

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

    let opaque_mat_idx = if any_opaque {
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

    let mut build_prims = |tris: &[Triangle]| -> Vec<JsonVal> {
        let opq: Vec<&Triangle> = tris.iter().filter(|t| !t.is_translucent).collect();
        let trn: Vec<&Triangle> = tris.iter().filter(|t| t.is_translucent).collect();
        let mut prims = Vec::new();
        if !opq.is_empty() {
            let mut p = append_primitive_buffers(&opq, &mut bin, &mut buffer_views, &mut accessors);
            if let (JsonVal::Obj(ref mut fields), Some(mi)) = (&mut p, opaque_mat_idx) {
                fields.push(("material".into(), JsonVal::Int(mi as i64)));
            }
            prims.push(p);
        }
        if !trn.is_empty() {
            let mut p = append_primitive_buffers(&trn, &mut bin, &mut buffer_views, &mut accessors);
            if let (JsonVal::Obj(ref mut fields), Some(mi)) = (&mut p, trans_mat_idx) {
                fields.push(("material".into(), JsonVal::Int(mi as i64)));
            }
            prims.push(p);
        }
        prims
    };

    let mut gltf_meshes: Vec<JsonVal> = Vec::new();
    let mut gltf_nodes: Vec<JsonVal> = Vec::new();
    let mut child_indices: Vec<JsonVal> = Vec::new();
    let mut total_draw_calls = 0usize;
    let mut total_tris = 0usize;

    let root_mesh_idx = if !mesh.tris.is_empty() {
        let prims = build_prims(&mesh.tris);
        total_draw_calls += prims.len();
        total_tris += mesh.tris.len();
        let idx = gltf_meshes.len() as i64;
        gltf_meshes.push(obj(vec![
            ("name", JsonVal::Str(name.into())),
            ("primitives", JsonVal::Arr(prims)),
        ]));
        Some(idx)
    } else {
        None
    };

    // Named visual child nodes (with local pivot translation/rotation/scale)
    for cnode in &mesh.nodes {
        if cnode.tris.is_empty() {
            continue;
        }
        let prims = build_prims(&cnode.tris);
        total_draw_calls += prims.len();
        total_tris += cnode.tris.len();
        let mesh_idx = gltf_meshes.len() as i64;
        gltf_meshes.push(obj(vec![
            ("name", JsonVal::Str(cnode.name.clone())),
            ("primitives", JsonVal::Arr(prims)),
        ]));
        let node_idx = (1 + child_indices.len()) as i64;
        child_indices.push(JsonVal::Int(node_idx));
        let mut nf = vec![
            ("name", JsonVal::Str(cnode.name.clone())),
            ("mesh", JsonVal::Int(mesh_idx)),
        ];
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
        gltf_nodes.push(obj(nf));
    }

    // 3 PBR Atlas images (BaseColor, ORM, Emissive)
    let mut images = Vec::new();
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
        if is_glb {
            for (img_name, bytes) in pngs {
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
            }
        } else {
            for (img_name, bytes) in pngs {
                let uri = format!("data:image/png;base64,{}", base64::encode(&bytes));
                images.push(obj(vec![
                    ("name", JsonVal::Str(img_name.into())),
                    ("mimeType", JsonVal::Str("image/png".into())),
                    ("uri", JsonVal::Str(uri)),
                ]));
            }
        }
    }

    for col in &mesh.colliders {
        if col.tris.is_empty() {
            continue;
        }
        let col_refs: Vec<&Triangle> = col.tris.iter().collect();
        let mut col_prim = append_primitive_buffers(&col_refs, &mut bin, &mut buffer_views, &mut accessors);
        if let JsonVal::Obj(ref mut map) = col_prim {
            map.push(("material".into(), JsonVal::Int(opaque_mat_idx.unwrap_or(0) as i64)));
        }
        let mesh_idx = gltf_meshes.len() as i64;
        gltf_meshes.push(obj(vec![
            ("name", JsonVal::Str(col.name.clone())),
            ("primitives", JsonVal::Arr(vec![col_prim])),
        ]));
        let node_idx = (1 + child_indices.len()) as i64;
        child_indices.push(JsonVal::Int(node_idx));
        gltf_nodes.push(obj(vec![
            ("name", JsonVal::Str(col.name.clone())),
            ("mesh", JsonVal::Int(mesh_idx)),
        ]));
    }

    let mut root_node_fields = vec![("name", JsonVal::Str(name.into()))];
    if let Some(rmi) = root_mesh_idx {
        root_node_fields.push(("mesh", JsonVal::Int(rmi)));
    }
    if !child_indices.is_empty() {
        root_node_fields.push(("children", JsonVal::Arr(child_indices)));
    }
    let mut all_nodes = Vec::with_capacity(1 + gltf_nodes.len());
    all_nodes.push(obj(root_node_fields));
    all_nodes.extend(gltf_nodes);

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

    let root = obj(vec![
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
        (
            "samplers",
            JsonVal::Arr(vec![obj(vec![
                ("magFilter", JsonVal::Int(9728)),
                ("minFilter", JsonVal::Int(9728)),
                ("wrapS", JsonVal::Int(33071)),
                ("wrapT", JsonVal::Int(33071)),
            ])]),
        ),
        (
            "textures",
            JsonVal::Arr(vec![
                obj(vec![("sampler", JsonVal::Int(0)), ("source", JsonVal::Int(0))]),
                obj(vec![("sampler", JsonVal::Int(0)), ("source", JsonVal::Int(1))]),
                obj(vec![("sampler", JsonVal::Int(0)), ("source", JsonVal::Int(2))]),
            ]),
        ),
        ("images", JsonVal::Arr(images)),
        ("buffers", JsonVal::Arr(vec![buffer_obj])),
        ("bufferViews", JsonVal::Arr(buffer_views)),
        ("accessors", JsonVal::Arr(accessors)),
    ]);

    let stats = ExportStats {
        draw_calls: total_draw_calls,
        has_translucent,
        total_tris,
        total_verts: total_tris * 3,
    };
    Ok((root, bin, stats))
}
