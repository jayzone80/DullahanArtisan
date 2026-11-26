use std::path::Path;

use crate::common::base64;
use crate::common::json::{parse_json, JsonVal};
use crate::common::png::inspect_and_verify_png;

#[derive(Debug, Clone)]
pub struct ValidationReport {
    pub format: String,
    pub primitives: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub images_checked: usize,
    pub errors: Vec<String>,
}

impl ValidationReport {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

pub fn validate_file(path: &Path) -> Result<ValidationReport, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("failed to read '{}': {e}", path.display()))?;
    if bytes.len() >= 4 && &bytes[0..4] == b"glTF" {
        validate_glb_bytes(&bytes)
    } else {
        let s = std::str::from_utf8(&bytes).map_err(|_| "file is neither GLB nor UTF-8 glTF")?;
        validate_gltf_json_str(s)
    }
}

pub fn validate_glb_bytes(bytes: &[u8]) -> Result<ValidationReport, String> {
    if bytes.len() < 20 {
        return Err("GLB truncated (< 20 bytes)".into());
    }
    let magic = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
    let version = u32::from_le_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let total_len = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    if magic != 0x4654_6C67 {
        return Err("invalid GLB magic".into());
    }
    if version != 2 {
        return Err(format!("unsupported GLB version {version}"));
    }
    if total_len != bytes.len() {
        return Err(format!("GLB header length {total_len} != file size {}", bytes.len()));
    }

    let c0_len = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]) as usize;
    let c0_type = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]);
    if c0_type != 0x4E4F_534A || 20 + c0_len > bytes.len() {
        return Err("invalid or truncated GLB JSON chunk".into());
    }
    let json_str = std::str::from_utf8(&bytes[20..20 + c0_len])
        .map_err(|_| "invalid UTF-8 in GLB JSON chunk")?;
    let root = parse_json(json_str)?;

    let bin_start = 20 + c0_len;
    if bin_start + 8 > bytes.len() {
        return Err("missing GLB BIN chunk".into());
    }
    let c1_len = u32::from_le_bytes([
        bytes[bin_start],
        bytes[bin_start + 1],
        bytes[bin_start + 2],
        bytes[bin_start + 3],
    ]) as usize;
    let c1_type = u32::from_le_bytes([
        bytes[bin_start + 4],
        bytes[bin_start + 5],
        bytes[bin_start + 6],
        bytes[bin_start + 7],
    ]);
    if c1_type != 0x004E_4942 || bin_start + 8 + c1_len > bytes.len() {
        return Err("invalid or truncated GLB BIN chunk".into());
    }
    let bin = &bytes[bin_start + 8..bin_start + 8 + c1_len];
    validate_parsed(&root, Some(bin), "GLB")
}

pub fn validate_gltf_json_str(json_str: &str) -> Result<ValidationReport, String> {
    let root = parse_json(json_str)?;
    validate_parsed(&root, None, "glTF")
}

fn validate_parsed(
    root: &JsonVal,
    glb_bin: Option<&[u8]>,
    format: &str,
) -> Result<ValidationReport, String> {
    let mut rep = ValidationReport {
        format: format.to_string(),
        primitives: 0,
        vertices: 0,
        triangles: 0,
        images_checked: 0,
        errors: Vec::new(),
    };

    let version = root
        .get("asset")
        .and_then(|a| a.get("version"))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if version != "2.0" {
        rep.errors.push(format!("asset.version '{version}' != '2.0'"));
    }

    // Decode primary binary buffer
    let buffers = root
        .get("buffers")
        .and_then(|b| b.as_arr())
        .ok_or_else(|| "missing buffers array".to_string())?;
    if buffers.is_empty() {
        return Err("empty buffers array".into());
    }
    let owned_bin: Vec<u8>;
    let bin: &[u8] = if let Some(b) = glb_bin {
        b
    } else {
        let uri = buffers[0]
            .get("uri")
            .and_then(|u| u.as_str())
            .ok_or_else(|| "glTF buffer[0] missing uri".to_string())?;
        let b64 = uri
            .strip_prefix("data:application/octet-stream;base64,")
            .ok_or_else(|| "buffer[0].uri is not base64 octet-stream".to_string())?;
        owned_bin = base64::decode(b64)?;
        &owned_bin
    };

    let declared_len = buffers[0]
        .get("byteLength")
        .and_then(|v| v.as_usize())
        .unwrap_or(0);
    if declared_len > bin.len() {
        rep.errors.push(format!(
            "buffer byteLength {declared_len} exceeds actual buffer size {}",
            bin.len()
        ));
    }

    // Check bufferViews alignment & bounds
    let bvs = root
        .get("bufferViews")
        .and_then(|v| v.as_arr())
        .ok_or_else(|| "missing bufferViews".to_string())?;
    for (i, bv) in bvs.iter().enumerate() {
        let off = bv.get("byteOffset").and_then(|v| v.as_usize()).unwrap_or(0);
        let len = bv.get("byteLength").and_then(|v| v.as_usize()).unwrap_or(0);
        if off % 4 != 0 {
            rep.errors.push(format!("bufferView[{i}] byteOffset {off} not 4-byte aligned"));
        }
        if off + len > bin.len() {
            rep.errors.push(format!("bufferView[{i}] range {off}..{} exceeds buffer {}", off + len, bin.len()));
        }
    }

    // Track which materials are clamped palette atlases vs repeating PBR texture sets
    let mut mat_is_palette: Vec<bool> = Vec::new();
    if let Some(mats) = root.get("materials").and_then(|v| v.as_arr()) {
        for (i, m) in mats.iter().enumerate() {
            let is_pal = m
                .get("name")
                .and_then(|v| v.as_str())
                .map(|s| s.starts_with("palette_"))
                .unwrap_or(true);
            mat_is_palette.push(is_pal);
            let pbr = m.get("pbrMetallicRoughness");
            if pbr.and_then(|p| p.get("baseColorTexture")).is_none() {
                rep.errors.push(format!("material[{i}] missing baseColorTexture"));
            }
            if is_pal {
                if pbr.and_then(|p| p.get("metallicRoughnessTexture")).is_none() {
                    rep.errors.push(format!("material[{i}] missing metallicRoughnessTexture"));
                }
                if m.get("occlusionTexture").is_none() {
                    rep.errors.push(format!("material[{i}] missing occlusionTexture"));
                }
                if m.get("emissiveTexture").is_none() {
                    rep.errors.push(format!("material[{i}] missing emissiveTexture"));
                }
            }
        }
    }

    // Check embedded PNG images (base64 data URI or GLB bufferView) and matching dimensions on palette trio
    let mut pal_dims: Option<(u32, u32)> = None;
    if let Some(imgs) = root.get("images").and_then(|v| v.as_arr()) {
        for (i, img) in imgs.iter().enumerate() {
            let png_bytes: Option<Vec<u8>> = if let Some(uri) = img.get("uri").and_then(|u| u.as_str()) {
                if let Some(b64) = uri.strip_prefix("data:image/png;base64,") {
                    Some(base64::decode(b64)?)
                } else {
                    None // external URI
                }
            } else if let Some(bv_i) = img.get("bufferView").and_then(|v| v.as_usize()) {
                let bv = &bvs[bv_i];
                let off = bv.get("byteOffset").and_then(|v| v.as_usize()).unwrap_or(0);
                let len = bv.get("byteLength").and_then(|v| v.as_usize()).unwrap_or(0);
                if off + len <= bin.len() {
                    Some(bin[off..off + len].to_vec())
                } else {
                    rep.errors.push(format!("image[{i}] bufferView out of range"));
                    None
                }
            } else {
                None
            };

            if let Some(bytes) = png_bytes {
                match inspect_and_verify_png(&bytes) {
                    Ok(info) => {
                        rep.images_checked += 1;
                        if i < 3 {
                            if let Some((ew, eh)) = pal_dims {
                                if (info.width, info.height) != (ew, eh) {
                                    rep.errors.push(format!(
                                        "image[{i}] dims {}x{} != {}x{}",
                                        info.width, info.height, ew, eh
                                    ));
                                }
                            } else {
                                pal_dims = Some((info.width, info.height));
                            }
                        }
                    }
                    Err(e) => rep.errors.push(format!("image[{i}] invalid PNG: {e}")),
                }
            }
        }
    }

    // Check mesh primitives: POSITION min/max & finite, unit NORMAL, UV in [0,1], indices in range
    let accessors = root
        .get("accessors")
        .and_then(|v| v.as_arr())
        .ok_or_else(|| "missing accessors".to_string())?;
    let meshes = root
        .get("meshes")
        .and_then(|v| v.as_arr())
        .ok_or_else(|| "missing meshes".to_string())?;

    let slice_bv = |acc_i: usize| -> Result<(&JsonVal, &[u8]), String> {
        let acc = accessors
            .get(acc_i)
            .ok_or_else(|| format!("accessor[{acc_i}] out of bounds"))?;
        let bv_i = acc
            .get("bufferView")
            .and_then(|v| v.as_usize())
            .ok_or_else(|| format!("accessor[{acc_i}] missing bufferView"))?;
        let bv = bvs.get(bv_i).ok_or_else(|| format!("bufferView[{bv_i}] out of bounds"))?;
        let off = bv.get("byteOffset").and_then(|v| v.as_usize()).unwrap_or(0);
        let len = bv.get("byteLength").and_then(|v| v.as_usize()).unwrap_or(0);
        if off + len > bin.len() {
            return Err(format!("accessor[{acc_i}] buffer slice exceeds buffer"));
        }
        Ok((acc, &bin[off..off + len]))
    };

    for mesh in meshes {
        if let Some(prims) = mesh.get("primitives").and_then(|v| v.as_arr()) {
            for (pi, prim) in prims.iter().enumerate() {
                rep.primitives += 1;
                let attrs = prim
                    .get("attributes")
                    .ok_or_else(|| format!("primitive[{pi}] missing attributes"))?;
                let pos_i = attrs.get("POSITION").and_then(|v| v.as_usize()).ok_or("missing POSITION")?;
                let (pos_acc, pos_data) = slice_bv(pos_i)?;
                let vcount = pos_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                rep.vertices += vcount;

                let mut actual_min = [f32::INFINITY; 3];
                let mut actual_max = [f32::NEG_INFINITY; 3];
                for k in 0..vcount {
                    let b = &pos_data[k * 12..(k + 1) * 12];
                    let x = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    let y = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                    let z = f32::from_le_bytes([b[8], b[9], b[10], b[11]]);
                    if !x.is_finite() || !y.is_finite() || !z.is_finite() {
                        rep.errors.push(format!("primitive[{pi}] non-finite vertex at {k}"));
                        break;
                    }
                    let arr = [x, y, z];
                    for a in 0..3 {
                        actual_min[a] = actual_min[a].min(arr[a]);
                        actual_max[a] = actual_max[a].max(arr[a]);
                    }
                }
                if let (Some(dmin), Some(dmax)) = (
                    pos_acc.get("min").and_then(|v| v.as_arr()),
                    pos_acc.get("max").and_then(|v| v.as_arr()),
                ) {
                    for a in 0..3 {
                        let mn = dmin.get(a).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let mx = dmax.get(a).and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        if (mn - actual_min[a]).abs() > 1e-3 || (mx - actual_max[a]).abs() > 1e-3 {
                            rep.errors.push(format!(
                                "primitive[{pi}] POSITION bounds mismatch on axis {a}"
                            ));
                        }
                    }
                } else {
                    rep.errors.push(format!("primitive[{pi}] POSITION accessor missing min/max"));
                }

                // Unit normals AND alignment with triangle CCW geometric normal
                if let Some(norm_i) = attrs.get("NORMAL").and_then(|v| v.as_usize()) {
                    let (_, norm_data) = slice_bv(norm_i)?;
                    let mut normals_ok = true;
                    for k in 0..vcount {
                        let b = &norm_data[k * 12..(k + 1) * 12];
                        let nx = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                        let ny = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                        let nz = f32::from_le_bytes([b[8], b[9], b[10], b[11]]);
                        let l = (nx * nx + ny * ny + nz * nz).sqrt();
                        if (l - 1.0).abs() > 0.02 {
                            rep.errors.push(format!("primitive[{pi}] non-unit normal len={l:.3} at {k}"));
                            normals_ok = false;
                            break;
                        }
                    }
                    if normals_ok {
                        let read_v3 = |buf: &[u8], vi: usize| -> [f32; 3] {
                            let b = &buf[vi * 12..(vi + 1) * 12];
                            [
                                f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                                f32::from_le_bytes([b[4], b[5], b[6], b[7]]),
                                f32::from_le_bytes([b[8], b[9], b[10], b[11]]),
                            ]
                        };
                        let mut inverted_tris = 0usize;
                        for t in 0..(vcount / 3) {
                            let p0 = read_v3(pos_data, t * 3);
                            let p1 = read_v3(pos_data, t * 3 + 1);
                            let p2 = read_v3(pos_data, t * 3 + 2);
                            let e1 = [p1[0] - p0[0], p1[1] - p0[1], p1[2] - p0[2]];
                            let e2 = [p2[0] - p0[0], p2[1] - p0[1], p2[2] - p0[2]];
                            let gn = [
                                e1[1] * e2[2] - e1[2] * e2[1],
                                e1[2] * e2[0] - e1[0] * e2[2],
                                e1[0] * e2[1] - e1[1] * e2[0],
                            ];
                            let n0 = read_v3(norm_data, t * 3);
                            let n1 = read_v3(norm_data, t * 3 + 1);
                            let n2 = read_v3(norm_data, t * 3 + 2);
                            let an = [n0[0] + n1[0] + n2[0], n0[1] + n1[1] + n2[1], n0[2] + n1[2] + n2[2]];
                            if gn[0] * an[0] + gn[1] * an[1] + gn[2] * an[2] < -1e-7 {
                                inverted_tris += 1;
                            }
                        }
                        if inverted_tris > 0 {
                            rep.errors.push(format!(
                                "primitive[{pi}] has {inverted_tris} triangle(s) with inverted vertex normals"
                            ));
                        }
                    }
                }

                // UVs: finite always, and strictly in [0,1] for clamped palette atlas primitives
                let prim_is_pal = prim
                    .get("material")
                    .and_then(|v| v.as_usize())
                    .and_then(|mi| mat_is_palette.get(mi).copied())
                    .unwrap_or(true);
                if let Some(uv_i) = attrs.get("TEXCOORD_0").and_then(|v| v.as_usize()) {
                    let (_, uv_data) = slice_bv(uv_i)?;
                    for k in 0..vcount {
                        let b = &uv_data[k * 8..(k + 1) * 8];
                        let u = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                        let v = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                        if !u.is_finite() || !v.is_finite() {
                            rep.errors.push(format!("primitive[{pi}] non-finite UV at {k}"));
                            break;
                        }
                        if prim_is_pal && (!(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v)) {
                            rep.errors.push(format!("primitive[{pi}] UV ({u},{v}) out of [0,1]"));
                            break;
                        }
                    }
                }

                // Optional TANGENT attribute (VEC4: unit xyz + ±1 handedness)
                if let Some(tan_i) = attrs.get("TANGENT").and_then(|v| v.as_usize()) {
                    let (tan_acc, tan_data) = slice_bv(tan_i)?;
                    let tcnt = tan_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                    if tcnt != vcount {
                        rep.errors.push(format!("primitive[{pi}] TANGENT count {tcnt} != {vcount}"));
                    } else {
                        for k in 0..vcount {
                            let b = &tan_data[k * 16..(k + 1) * 16];
                            let tx = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                            let ty = f32::from_le_bytes([b[4], b[5], b[6], b[7]]);
                            let tz = f32::from_le_bytes([b[8], b[9], b[10], b[11]]);
                            let tw = f32::from_le_bytes([b[12], b[13], b[14], b[15]]);
                            let l = (tx * tx + ty * ty + tz * tz).sqrt();
                            if !l.is_finite() || (l - 1.0).abs() > 0.03 || (tw.abs() - 1.0).abs() > 0.01 {
                                rep.errors.push(format!("primitive[{pi}] invalid TANGENT at {k}"));
                                break;
                            }
                        }
                    }
                }

                // Optional morph targets (`primitive.targets`): verify POSITION delta bounds & count
                if let Some(targets) = prim.get("targets").and_then(|v| v.as_arr()) {
                    for (ti, tgt) in targets.iter().enumerate() {
                        if let Some(dp_i) = tgt.get("POSITION").and_then(|v| v.as_usize()) {
                            let (dp_acc, dp_data) = slice_bv(dp_i)?;
                            let dcnt = dp_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                            if dcnt != vcount {
                                rep.errors.push(format!(
                                    "primitive[{pi}].target[{ti}] POSITION count {dcnt} != {vcount}"
                                ));
                            }
                            if dp_acc.get("min").is_none() || dp_acc.get("max").is_none() {
                                rep.errors.push(format!(
                                    "primitive[{pi}].target[{ti}] POSITION missing min/max"
                                ));
                            }
                            for chunk in dp_data.chunks_exact(4) {
                                let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                                if !v.is_finite() {
                                    rep.errors.push(format!(
                                        "primitive[{pi}].target[{ti}] non-finite POSITION delta"
                                    ));
                                    break;
                                }
                            }
                        }
                    }
                }

                // Indices strictly in [0, vcount)
                if let Some(idx_i) = prim.get("indices").and_then(|v| v.as_usize()) {
                    let (idx_acc, idx_data) = slice_bv(idx_i)?;
                    let icount = idx_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                    let ctype = idx_acc.get("componentType").and_then(|v| v.as_usize()).unwrap_or(0);
                    rep.triangles += icount / 3;
                    for k in 0..icount {
                        let val = match ctype {
                            5123 => u16::from_le_bytes([idx_data[k * 2], idx_data[k * 2 + 1]]) as usize,
                            5125 => u32::from_le_bytes([
                                idx_data[k * 4],
                                idx_data[k * 4 + 1],
                                idx_data[k * 4 + 2],
                                idx_data[k * 4 + 3],
                            ]) as usize,
                            _ => {
                                rep.errors.push(format!("unsupported index componentType {ctype}"));
                                break;
                            }
                        };
                        if val >= vcount {
                            rep.errors.push(format!("primitive[{pi}] index {val} >= vertexCount {vcount}"));
                            break;
                        }
                    }
                }
            }
        }
    }

    // Validate optional glTF 2.0 `skins` (valid joint indices + inverseBindMatrices count & finite values)
    let node_count = root
        .get("nodes")
        .and_then(|v| v.as_arr())
        .map(|a| a.len())
        .unwrap_or(0);
    if let Some(skins) = root.get("skins").and_then(|v| v.as_arr()) {
        for (si, skin) in skins.iter().enumerate() {
            let joints = skin.get("joints").and_then(|v| v.as_arr()).unwrap_or(&[]);
            if joints.is_empty() {
                rep.errors.push(format!("skin[{si}] has no joints"));
            }
            for (ji, jv) in joints.iter().enumerate() {
                let n_i = jv.as_usize().unwrap_or(usize::MAX);
                if n_i >= node_count {
                    rep.errors.push(format!("skin[{si}].joints[{ji}] invalid node {n_i}"));
                }
            }
            if let Some(ibm_i) = skin.get("inverseBindMatrices").and_then(|v| v.as_usize()) {
                let (ibm_acc, ibm_bytes) = slice_bv(ibm_i)?;
                let cnt = ibm_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                if cnt != joints.len() {
                    rep.errors.push(format!(
                        "skin[{si}] inverseBindMatrices count {cnt} != joints {}",
                        joints.len()
                    ));
                }
                for chunk in ibm_bytes.chunks_exact(4) {
                    let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if !v.is_finite() {
                        rep.errors.push(format!("skin[{si}] non-finite inverseBindMatrix entry"));
                        break;
                    }
                }
            }
        }
    }

    // Validate optional glTF 2.0 `animations` (samplers, strictly non-decreasing timestamps, valid target nodes)
    let mesh_morph_counts: Vec<usize> = meshes
        .iter()
        .map(|m| m.get("weights").and_then(|w| w.as_arr()).map(|a| a.len()).unwrap_or(0))
        .collect();
    let nodes_arr = root.get("nodes").and_then(|v| v.as_arr()).unwrap_or(&[]);
    if let Some(anims) = root.get("animations").and_then(|v| v.as_arr()) {
        for (ai, anim) in anims.iter().enumerate() {
            let samplers = anim
                .get("samplers")
                .and_then(|v| v.as_arr())
                .unwrap_or(&[]);
            let chans = anim
                .get("channels")
                .and_then(|v| v.as_arr())
                .unwrap_or(&[]);
            for (si, samp) in samplers.iter().enumerate() {
                let Some(in_i) = samp.get("input").and_then(|v| v.as_usize()) else {
                    rep.errors.push(format!("animation[{ai}].sampler[{si}] missing input"));
                    continue;
                };
                let Some(out_i) = samp.get("output").and_then(|v| v.as_usize()) else {
                    rep.errors.push(format!("animation[{ai}].sampler[{si}] missing output"));
                    continue;
                };
                let (in_acc, in_bytes) = slice_bv(in_i)?;
                let (out_acc, out_bytes) = slice_bv(out_i)?;
                let in_cnt = in_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                let out_cnt = out_acc.get("count").and_then(|v| v.as_usize()).unwrap_or(0);
                let interp = samp
                    .get("interpolation")
                    .and_then(|v| v.as_str())
                    .unwrap_or("LINEAR");
                // For `weights` channels, output count is in_cnt * num_morph_targets (times 3 if CUBICSPLINE)
                let mut morph_mul = 1usize;
                for ch in chans {
                    if ch.get("sampler").and_then(|v| v.as_usize()) == Some(si) {
                        if let Some(tgt) = ch.get("target") {
                            if tgt.get("path").and_then(|v| v.as_str()) == Some("weights") {
                                if let Some(ni) = tgt.get("node").and_then(|v| v.as_usize()) {
                                    if let Some(mi) = nodes_arr.get(ni).and_then(|n| n.get("mesh")).and_then(|v| v.as_usize()) {
                                        morph_mul = mesh_morph_counts.get(mi).copied().unwrap_or(1).max(1);
                                    }
                                }
                            }
                        }
                    }
                }
                let expected_out = if interp == "CUBICSPLINE" {
                    in_cnt * 3 * morph_mul
                } else {
                    in_cnt * morph_mul
                };
                if out_cnt != expected_out {
                    rep.errors.push(format!(
                        "animation[{ai}].sampler[{si}] output count {out_cnt} != expected {expected_out}"
                    ));
                }
                let mut prev_t = f32::NEG_INFINITY;
                for k in 0..in_cnt {
                    let b = &in_bytes[k * 4..(k + 1) * 4];
                    let t = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                    if !t.is_finite() || t < prev_t {
                        rep.errors.push(format!(
                            "animation[{ai}].sampler[{si}] invalid timestamp {t} at {k}"
                        ));
                        break;
                    }
                    prev_t = t;
                }
                for chunk in out_bytes.chunks_exact(4) {
                    let v = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
                    if !v.is_finite() {
                        rep.errors.push(format!("animation[{ai}].sampler[{si}] non-finite output value"));
                        break;
                    }
                }
            }
            if let Some(chans) = anim.get("channels").and_then(|v| v.as_arr()) {
                for (ci, ch) in chans.iter().enumerate() {
                    let s_i = ch.get("sampler").and_then(|v| v.as_usize()).unwrap_or(usize::MAX);
                    if s_i >= samplers.len() {
                        rep.errors.push(format!("animation[{ai}].channel[{ci}] invalid sampler {s_i}"));
                    }
                    if let Some(tgt) = ch.get("target") {
                        let n_i = tgt.get("node").and_then(|v| v.as_usize()).unwrap_or(usize::MAX);
                        if n_i >= node_count {
                            rep.errors.push(format!("animation[{ai}].channel[{ci}] invalid node {n_i}"));
                        }
                    }
                }
            }
        }
    }

    Ok(rep)
}
