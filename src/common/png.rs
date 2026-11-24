use crate::common::deflate::{zlib_compress, zlib_decompress};

const PNG_SIG: [u8; 8] = [137, 80, 78, 71, 13, 10, 26, 10];

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for &b in data {
        crc ^= b as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn write_chunk(out: &mut Vec<u8>, tag: &[u8; 4], payload: &[u8]) {
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    let crc_start = out.len();
    out.extend_from_slice(tag);
    out.extend_from_slice(payload);
    let c = crc32(&out[crc_start..]);
    out.extend_from_slice(&c.to_be_bytes());
}

/// Encode 8-bit RGBA pixels (top-down row-major) into a compressed PNG byte buffer.
pub fn encode_rgba(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    encode_raw(width, height, rgba, 4, 6, 8)
}

/// Encode 8-bit RGB pixels (top-down row-major) into a compressed PNG byte buffer.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    encode_raw(width, height, rgb, 3, 2, 8)
}

/// Encode 16-bit grayscale pixels (top-down row-major) into a lossless 16-bit PNG (big-endian).
pub fn encode_gray16(width: u32, height: u32, gray: &[u16]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(gray.len() * 2);
    for &v in gray {
        bytes.extend_from_slice(&v.to_be_bytes());
    }
    encode_raw(width, height, &bytes, 2, 0, 16)
}

fn encode_raw(
    width: u32,
    height: u32,
    pixels: &[u8],
    bpp: usize,
    color_type: u8,
    bit_depth: u8,
) -> Vec<u8> {
    let stride = width as usize * bpp;
    assert_eq!(pixels.len(), stride * height as usize);

    // Apply PNG Sub filter (type 1) per scanline — dramatically shrinks flat-color atlases and UI plots.
    let mut raw = Vec::with_capacity((stride + 1) * height as usize);
    for y in 0..height as usize {
        raw.push(1u8); // Filter type 1 = Sub
        let row = &pixels[y * stride..(y + 1) * stride];
        for x in 0..stride {
            let left = if x >= bpp { row[x - bpp] } else { 0 };
            raw.push(row[x].wrapping_sub(left));
        }
    }

    let compressed = zlib_compress(&raw);

    let mut out = Vec::with_capacity(compressed.len() + 64);
    out.extend_from_slice(&PNG_SIG);

    let mut ihdr = [0u8; 13];
    ihdr[0..4].copy_from_slice(&width.to_be_bytes());
    ihdr[4..8].copy_from_slice(&height.to_be_bytes());
    ihdr[8] = bit_depth;
    ihdr[9] = color_type;
    ihdr[10] = 0; // deflate
    ihdr[11] = 0; // adaptive filter
    ihdr[12] = 0; // non-interlaced
    write_chunk(&mut out, b"IHDR", &ihdr);
    write_chunk(&mut out, b"IDAT", &compressed);
    write_chunk(&mut out, b"IEND", &[]);
    out
}

#[derive(Debug, Clone)]
pub struct DecodedPng {
    pub width: u32,
    pub height: u32,
    /// Normalized 0..=1 grayscale luminance (preserves full 16-bit precision for 16-bit gray PNGs).
    pub gray_f32: Vec<f32>,
    /// Straight 8-bit RGBA pixels (`width * height * 4`).
    pub rgba: Vec<u8>,
}

fn paeth(a: u8, b: u8, c: u8) -> u8 {
    let a = a as i32;
    let b = b as i32;
    let c = c as i32;
    let p = a + b - c;
    let pa = (p - a).abs();
    let pb = (p - b).abs();
    let pc = (p - c).abs();
    if pa <= pb && pa <= pc {
        a as u8
    } else if pb <= pc {
        b as u8
    } else {
        c as u8
    }
}

/// Decode a non-interlaced 8-bit/16-bit PNG (Gray, RGB, GrayAlpha, RGBA) into float grayscale + RGBA8.
pub fn decode_png(bytes: &[u8]) -> Result<DecodedPng, String> {
    if bytes.len() < 33 || bytes[0..8] != PNG_SIG {
        return Err("invalid PNG signature".into());
    }
    let mut pos = 8usize;
    let mut info: Option<PngInfo> = None;
    let mut idat = Vec::new();
    let mut saw_iend = false;

    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let tag = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start + len;
        if data_end + 4 > bytes.len() {
            return Err("truncated PNG chunk".into());
        }
        let payload = &bytes[data_start..data_end];
        match tag {
            b"IHDR" => {
                if payload.len() != 13 {
                    return Err("invalid IHDR length".into());
                }
                if payload[12] != 0 {
                    return Err("interlaced PNGs are not supported".into());
                }
                info = Some(PngInfo {
                    width: u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]),
                    height: u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]),
                    bit_depth: payload[8],
                    color_type: payload[9],
                });
            }
            b"IDAT" => idat.extend_from_slice(payload),
            b"IEND" => {
                saw_iend = true;
                break;
            }
            _ => {}
        }
        pos = data_end + 4;
    }

    let info = info.ok_or_else(|| "missing PNG IHDR".to_string())?;
    if !saw_iend {
        return Err("missing PNG IEND".into());
    }
    let channels = match info.color_type {
        0 => 1usize,
        2 => 3usize,
        4 => 2usize,
        6 => 4usize,
        other => return Err(format!("unsupported PNG color type {other} (use gray, RGB, or RGBA)")),
    };
    let bytes_per_ch = match info.bit_depth {
        8 => 1usize,
        16 => 2usize,
        other => return Err(format!("unsupported PNG bit depth {other} (use 8 or 16)")),
    };
    let bpp = channels * bytes_per_ch;
    let w = info.width as usize;
    let h = info.height as usize;
    let stride = w * bpp;
    let raw = zlib_decompress(&idat)?;
    if raw.len() != (stride + 1) * h {
        return Err(format!("PNG raw size {} != expected {}", raw.len(), (stride + 1) * h));
    }

    let mut recon = vec![0u8; stride * h];
    for y in 0..h {
        let ftype = raw[y * (stride + 1)];
        let src = &raw[y * (stride + 1) + 1..y * (stride + 1) + 1 + stride];
        let row_off = y * stride;
        for x in 0..stride {
            let left = if x >= bpp { recon[row_off + x - bpp] } else { 0 };
            let up = if y > 0 { recon[row_off - stride + x] } else { 0 };
            let ul = if y > 0 && x >= bpp { recon[row_off - stride + x - bpp] } else { 0 };
            let val = match ftype {
                0 => src[x],
                1 => src[x].wrapping_add(left),
                2 => src[x].wrapping_add(up),
                3 => src[x].wrapping_add(((left as u16 + up as u16) >> 1) as u8),
                4 => src[x].wrapping_add(paeth(left, up, ul)),
                _ => return Err(format!("invalid PNG filter type {ftype}")),
            };
            recon[row_off + x] = val;
        }
    }

    let mut gray_f32 = Vec::with_capacity(w * h);
    let mut rgba = Vec::with_capacity(w * h * 4);
    for i in 0..(w * h) {
        let o = i * bpp;
        match (info.color_type, info.bit_depth) {
            (0, 8) => {
                let v = recon[o];
                gray_f32.push(v as f32 / 255.0);
                rgba.extend_from_slice(&[v, v, v, 255]);
            }
            (0, 16) => {
                let v = u16::from_be_bytes([recon[o], recon[o + 1]]);
                gray_f32.push(v as f32 / 65535.0);
                let u = (v >> 8) as u8;
                rgba.extend_from_slice(&[u, u, u, 255]);
            }
            (4, 8) => {
                let v = recon[o];
                let a = recon[o + 1];
                gray_f32.push(v as f32 / 255.0);
                rgba.extend_from_slice(&[v, v, v, a]);
            }
            (4, 16) => {
                let v = u16::from_be_bytes([recon[o], recon[o + 1]]);
                let a = recon[o + 2];
                gray_f32.push(v as f32 / 65535.0);
                let u = (v >> 8) as u8;
                rgba.extend_from_slice(&[u, u, u, a]);
            }
            (2, 8) => {
                let (r, g, b) = (recon[o], recon[o + 1], recon[o + 2]);
                gray_f32.push((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0);
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
            (2, 16) => {
                let r = u16::from_be_bytes([recon[o], recon[o + 1]]);
                let g = u16::from_be_bytes([recon[o + 2], recon[o + 3]]);
                let b = u16::from_be_bytes([recon[o + 4], recon[o + 5]]);
                gray_f32.push((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 65535.0);
                rgba.extend_from_slice(&[(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8, 255]);
            }
            (6, 8) => {
                let (r, g, b, a) = (recon[o], recon[o + 1], recon[o + 2], recon[o + 3]);
                gray_f32.push((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 255.0);
                rgba.extend_from_slice(&[r, g, b, a]);
            }
            (6, 16) => {
                let r = u16::from_be_bytes([recon[o], recon[o + 1]]);
                let g = u16::from_be_bytes([recon[o + 2], recon[o + 3]]);
                let b = u16::from_be_bytes([recon[o + 4], recon[o + 5]]);
                let a = u16::from_be_bytes([recon[o + 6], recon[o + 7]]);
                gray_f32.push((0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32) / 65535.0);
                rgba.extend_from_slice(&[(r >> 8) as u8, (g >> 8) as u8, (b >> 8) as u8, (a >> 8) as u8]);
            }
            _ => unreachable!(),
        }
    }

    Ok(DecodedPng { width: info.width, height: info.height, gray_f32, rgba })
}

#[derive(Debug, Clone)]
pub struct PngInfo {
    pub width: u32,
    pub height: u32,
    pub bit_depth: u8,
    pub color_type: u8,
}

/// Inspect and validate PNG signature, CRC integrity, IHDR, IDAT zlib decode, and IEND.
pub fn inspect_and_verify_png(bytes: &[u8]) -> Result<PngInfo, String> {
    if bytes.len() < 33 || bytes[0..8] != PNG_SIG {
        return Err("invalid PNG signature".into());
    }
    let mut pos = 8usize;
    let mut info: Option<PngInfo> = None;
    let mut idat = Vec::new();
    let mut saw_iend = false;

    while pos + 12 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
        let tag = &bytes[pos + 4..pos + 8];
        let data_start = pos + 8;
        let data_end = data_start + len;
        if data_end + 4 > bytes.len() {
            return Err("truncated PNG chunk".into());
        }
        let expected_crc = u32::from_be_bytes([
            bytes[data_end],
            bytes[data_end + 1],
            bytes[data_end + 2],
            bytes[data_end + 3],
        ]);
        let actual_crc = crc32(&bytes[pos + 4..data_end]);
        if actual_crc != expected_crc {
            return Err(format!("PNG CRC mismatch in chunk {:?}", String::from_utf8_lossy(tag)));
        }
        let payload = &bytes[data_start..data_end];
        match tag {
            b"IHDR" => {
                if payload.len() != 13 {
                    return Err("invalid IHDR length".into());
                }
                let width = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                let height = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
                info = Some(PngInfo {
                    width,
                    height,
                    bit_depth: payload[8],
                    color_type: payload[9],
                });
            }
            b"IDAT" => idat.extend_from_slice(payload),
            b"IEND" => {
                saw_iend = true;
                break;
            }
            _ => {}
        }
        pos = data_end + 4;
    }

    let info = info.ok_or_else(|| "missing PNG IHDR chunk".to_string())?;
    if !saw_iend {
        return Err("missing PNG IEND chunk".into());
    }
    if info.width == 0 || info.height == 0 {
        return Err("PNG has zero dimensions".into());
    }
    let raw = zlib_decompress(&idat)?;
    let bpp = match (info.color_type, info.bit_depth) {
        (0, 8) => 1usize,
        (0, 16) => 2usize,
        (2, 8) => 3usize,
        (6, 8) => 4usize,
        _ => return Ok(info),
    };
    let expected_raw = (info.width as usize * bpp + 1) * info.height as usize;
    if raw.len() != expected_raw {
        return Err(format!(
            "PNG decompressed byte length {} != expected {}",
            raw.len(),
            expected_raw
        ));
    }
    Ok(info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn png_encode_verify_roundtrip() {
        let mut rgba = vec![0u8; 32 * 32 * 4];
        for y in 0..32 {
            for x in 0..32 {
                let i = (y * 32 + x) * 4;
                rgba[i] = (x * 8) as u8;
                rgba[i + 1] = (y * 8) as u8;
                rgba[i + 2] = 128;
                rgba[i + 3] = 255;
            }
        }
        let png = encode_rgba(32, 32, &rgba);
        let info = inspect_and_verify_png(&png).unwrap();
        assert_eq!(info.width, 32);
        assert_eq!(info.height, 32);
        assert_eq!(info.color_type, 6);

        let dec = decode_png(&png).unwrap();
        assert_eq!(dec.rgba, rgba);

        let gray16: Vec<u16> = (0..64).map(|i| i * 1024).collect();
        let png16 = encode_gray16(8, 8, &gray16);
        let info16 = inspect_and_verify_png(&png16).unwrap();
        assert_eq!((info16.bit_depth, info16.color_type), (16, 0));
        let dec16 = decode_png(&png16).unwrap();
        assert!((dec16.gray_f32[32] - (32768.0 / 65535.0)).abs() < 1e-4);
    }
}
