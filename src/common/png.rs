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
    encode_raw(width, height, rgba, 4, 6)
}

/// Encode 8-bit RGB pixels (top-down row-major) into a compressed PNG byte buffer.
pub fn encode_rgb(width: u32, height: u32, rgb: &[u8]) -> Vec<u8> {
    encode_raw(width, height, rgb, 3, 2)
}

fn encode_raw(width: u32, height: u32, pixels: &[u8], bpp: usize, color_type: u8) -> Vec<u8> {
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
    ihdr[8] = 8; // bit depth
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
    let bpp = match info.color_type {
        2 => 3usize,
        6 => 4usize,
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
    }
}
