const ENC_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(((data.len() + 2) / 3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0] as u32;
        let b1 = if chunk.len() > 1 { chunk[1] as u32 } else { 0 };
        let b2 = if chunk.len() > 2 { chunk[2] as u32 } else { 0 };
        let n = (b0 << 16) | (b1 << 8) | b2;
        out.push(ENC_TABLE[((n >> 18) & 63) as usize] as char);
        out.push(ENC_TABLE[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            out.push(ENC_TABLE[((n >> 6) & 63) as usize] as char);
        } else {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(ENC_TABLE[(n & 63) as usize] as char);
        } else {
            out.push('=');
        }
    }
    out
}

pub fn decode(s: &str) -> Result<Vec<u8>, String> {
    let mut dec = [255u8; 128];
    for (i, &b) in ENC_TABLE.iter().enumerate() {
        dec[b as usize] = i as u8;
    }
    let bytes: Vec<u8> = s.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if bytes.len() % 4 != 0 {
        return Err("invalid base64 length".into());
    }
    let mut out = Vec::with_capacity((bytes.len() / 4) * 3);
    for chunk in bytes.chunks_exact(4) {
        let mut vals = [0u32; 4];
        let mut pad = 0usize;
        for i in 0..4 {
            let b = chunk[i];
            if b == b'=' {
                pad += 1;
                vals[i] = 0;
            } else if (b as usize) < 128 && dec[b as usize] != 255 {
                vals[i] = dec[b as usize] as u32;
            } else {
                return Err(format!("invalid base64 char '{}'", b as char));
            }
        }
        let n = (vals[0] << 18) | (vals[1] << 12) | (vals[2] << 6) | vals[3];
        out.push(((n >> 16) & 0xFF) as u8);
        if pad < 2 {
            out.push(((n >> 8) & 0xFF) as u8);
        }
        if pad < 1 {
            out.push((n & 0xFF) as u8);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_roundtrip() {
        let src = b"Dullahan Artisan PBR + DSP binary check 0123456789!";
        let e = encode(src);
        let d = decode(&e).unwrap();
        assert_eq!(d, src);
    }
}
