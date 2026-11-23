use std::f64::consts::FRAC_PI_2;
use std::path::Path;

use crate::common::math::Rng;

#[derive(Debug, Clone)]
pub struct AudioBuffer {
    pub sample_rate: u32,
    pub channels: u16, // 1 or 2 (internal storage always planar stereo; mono mixes L+R on write)
    pub left: Vec<f64>,
    pub right: Vec<f64>,
}

impl AudioBuffer {
    pub fn new(sample_rate: u32, channels: u16, sample_count: usize) -> Self {
        Self {
            sample_rate,
            channels: channels.clamp(1, 2),
            left: vec![0.0; sample_count],
            right: vec![0.0; sample_count],
        }
    }

    pub fn len(&self) -> usize {
        self.left.len()
    }

    pub fn duration_sec(&self) -> f64 {
        self.left.len() as f64 / self.sample_rate.max(1) as f64
    }

    /// Mix `other` into `self` at `offset_sample` with gain and constant-power stereo pan (`-1..+1`).
    pub fn add_scaled(&mut self, other: &AudioBuffer, gain: f64, pan: f64, offset_sample: usize) {
        let p = (pan.clamp(-1.0, 1.0) + 1.0) * 0.5;
        let gl = (p * FRAC_PI_2).cos() * std::f64::consts::SQRT_2 * gain;
        let gr = (p * FRAC_PI_2).sin() * std::f64::consts::SQRT_2 * gain;
        let n = other.len().min(self.len().saturating_sub(offset_sample));
        for i in 0..n {
            let d = offset_sample + i;
            self.left[d] += other.left[i] * gl;
            self.right[d] += other.right[i] * gr;
        }
    }

    pub fn extract_slice(&self, start: usize, len: usize) -> Self {
        let mut out = Self::new(self.sample_rate, self.channels, len);
        let avail = self.len().saturating_sub(start).min(len);
        out.left[..avail].copy_from_slice(&self.left[start..start + avail]);
        out.right[..avail].copy_from_slice(&self.right[start..start + avail]);
        out
    }
}

/// Write standards-compliant RIFF/WAVE 16-bit PCM with Triangular-PDF (TPDF) dither.
pub fn write_pcm16(path: &Path, buf: &AudioBuffer) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create dir '{}': {e}", parent.display()))?;
        }
    }
    let bytes = encode_pcm16_bytes(buf);
    std::fs::write(path, bytes)
        .map_err(|e| format!("failed to write wav '{}': {e}", path.display()))
}

pub fn encode_pcm16_bytes(buf: &AudioBuffer) -> Vec<u8> {
    let ch = buf.channels.clamp(1, 2) as usize;
    let n = buf.len();
    let data_bytes = n * ch * 2;
    let mut out = Vec::with_capacity(44 + data_bytes);

    // RIFF header
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&((36 + data_bytes) as u32).to_le_bytes());
    out.extend_from_slice(b"WAVE");

    // fmt chunk (PCM = 1, 16-bit)
    out.extend_from_slice(b"fmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&(ch as u16).to_le_bytes());
    out.extend_from_slice(&buf.sample_rate.to_le_bytes());
    let byte_rate = buf.sample_rate * ch as u32 * 2;
    let block_align = (ch * 2) as u16;
    out.extend_from_slice(&byte_rate.to_le_bytes());
    out.extend_from_slice(&block_align.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());

    // data chunk
    out.extend_from_slice(b"data");
    out.extend_from_slice(&(data_bytes as u32).to_le_bytes());

    let mut rng = Rng::new(0x5A17_D17B);
    let mut quant = |s: f64| -> i16 {
        let dither = (rng.f64() - rng.f64()) / 32768.0;
        let v = (s + dither).clamp(-1.0, 1.0);
        (v * 32767.0).round().clamp(-32768.0, 32767.0) as i16
    };

    if ch == 1 {
        for i in 0..n {
            let mono = 0.5 * (buf.left[i] + buf.right[i]);
            out.extend_from_slice(&quant(mono).to_le_bytes());
        }
    } else {
        for i in 0..n {
            out.extend_from_slice(&quant(buf.left[i]).to_le_bytes());
            out.extend_from_slice(&quant(buf.right[i]).to_le_bytes());
        }
    }
    out
}

/// Decode RIFF/WAVE 16-bit PCM file into an `AudioBuffer`.
pub fn read_wav(path: &Path) -> Result<AudioBuffer, String> {
    let bytes = std::fs::read(path)
        .map_err(|e| format!("failed to read wav '{}': {e}", path.display()))?;
    decode_pcm16_bytes(&bytes)
}

pub fn decode_pcm16_bytes(bytes: &[u8]) -> Result<AudioBuffer, String> {
    if bytes.len() < 44 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err("invalid RIFF/WAVE header".into());
    }
    let mut pos = 12usize;
    let mut sample_rate = 0u32;
    let mut channels = 0u16;
    let mut bits = 0u16;
    let mut data_slice: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let tag = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]])
            as usize;
        let start = pos + 8;
        let end = (start + len).min(bytes.len());
        match tag {
            b"fmt " if end - start >= 16 => {
                let fmt = u16::from_le_bytes([bytes[start], bytes[start + 1]]);
                if fmt != 1 {
                    return Err(format!("unsupported WAV format tag {fmt} (expected PCM=1)"));
                }
                channels = u16::from_le_bytes([bytes[start + 2], bytes[start + 3]]);
                sample_rate = u32::from_le_bytes([
                    bytes[start + 4],
                    bytes[start + 5],
                    bytes[start + 6],
                    bytes[start + 7],
                ]);
                bits = u16::from_le_bytes([bytes[start + 14], bytes[start + 15]]);
            }
            b"data" => {
                data_slice = Some(&bytes[start..end]);
            }
            _ => {}
        }
        pos = start + len + (len & 1);
    }

    if sample_rate == 0 || (channels != 1 && channels != 2) || bits != 16 {
        return Err(format!(
            "unsupported WAV params (sr={sample_rate}, ch={channels}, bits={bits})"
        ));
    }
    let data = data_slice.ok_or_else(|| "WAV missing data chunk".to_string())?;
    let frame_bytes = channels as usize * 2;
    let n = data.len() / frame_bytes;
    let mut buf = AudioBuffer::new(sample_rate, channels, n);
    let inv = 1.0 / 32768.0;
    if channels == 1 {
        for i in 0..n {
            let s = i16::from_le_bytes([data[i * 2], data[i * 2 + 1]]) as f64 * inv;
            buf.left[i] = s;
            buf.right[i] = s;
        }
    } else {
        for i in 0..n {
            let l = i16::from_le_bytes([data[i * 4], data[i * 4 + 1]]) as f64 * inv;
            let r = i16::from_le_bytes([data[i * 4 + 2], data[i * 4 + 3]]) as f64 * inv;
            buf.left[i] = l;
            buf.right[i] = r;
        }
    }
    Ok(buf)
}
