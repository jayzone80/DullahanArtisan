use std::f64::consts::{PI, TAU};

use crate::common::font::Canvas;
use crate::common::png::encode_rgba;
use crate::sfx::wav::AudioBuffer;

#[derive(Debug, Clone)]
pub struct AudioMetrics {
    pub duration_sec: f64,
    pub sample_rate: u32,
    pub channels: u16,
    pub peak_db: f64,
    pub rms_db: f64,
    pub dc_offset: f64,
    pub clipped_samples: usize,
    pub loop_seam_delta: f64,
    pub spectral_centroid_hz: f64,
    /// 6-band relative energy in dB: Sub(<60), Bass(60-250), LowMid(250-800), Mid(800-2.5k), HighMid(2.5k-6k), Air(>6k)
    pub bands_db: [f64; 6],
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl AudioMetrics {
    pub fn is_ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// Analyze audio buffer for loudness, DC bias, clipping, loop seam continuity, and 6-band spectrum.
pub fn analyze_wav(buf: &AudioBuffer, expect_loop: bool) -> AudioMetrics {
    let n = buf.len();
    let mut errors = Vec::new();
    let mut warnings = Vec::new();

    if n == 0 {
        errors.push("empty audio buffer".into());
        return AudioMetrics {
            duration_sec: 0.0,
            sample_rate: buf.sample_rate,
            channels: buf.channels,
            peak_db: -120.0,
            rms_db: -120.0,
            dc_offset: 0.0,
            clipped_samples: 0,
            loop_seam_delta: 0.0,
            spectral_centroid_hz: 0.0,
            bands_db: [-60.0; 6],
            errors,
            warnings,
        };
    }

    let mut peak = 0.0f64;
    let mut sq_sum = 0.0f64;
    let mut sum_l = 0.0f64;
    let mut sum_r = 0.0f64;
    let mut clipped = 0usize;
    let mut non_finite = false;

    for i in 0..n {
        let l = buf.left[i];
        let r = buf.right[i];
        if !l.is_finite() || !r.is_finite() {
            non_finite = true;
            continue;
        }
        peak = peak.max(l.abs()).max(r.abs());
        sq_sum += 0.5 * (l * l + r * r);
        sum_l += l;
        sum_r += r;
        if l.abs() >= 0.9995 || r.abs() >= 0.9995 {
            clipped += 1;
        }
    }

    if non_finite {
        errors.push("buffer contains NaN/Inf samples".into());
    }
    let peak_db = 20.0 * peak.max(1e-7).log10();
    let rms_db = 20.0 * (sq_sum / n as f64).sqrt().max(1e-7).log10();
    let dc_offset = (sum_l / n as f64).abs().max((sum_r / n as f64).abs());
    let channel_seam = |ch: &[f64]| -> f64 {
        let len = ch.len();
        if len < 3 {
            return if len == 2 { (ch[0] - ch[1]).abs() } else { 0.0 };
        }
        let d_seam = ch[0] - ch[len - 1];
        let raw = d_seam.abs();
        let d_next = ch[1] - ch[0];
        let d_prev = ch[len - 1] - ch[len - 2];
        // Slope-aware continuity: eliminates false positives on high-frequency cycle-locked waves
        // while catching real amplitude steps, DC pops, and phase discontinuities.
        raw.min((d_seam - d_next).abs()).min((d_seam - d_prev).abs())
    };
    let loop_seam_delta = if buf.channels == 1 {
        channel_seam(&buf.left)
    } else {
        channel_seam(&buf.left).max(channel_seam(&buf.right))
    };

    if peak_db < -45.0 {
        errors.push(format!("audio is effectively silent (peak {peak_db:.1} dBFS)"));
    }
    if dc_offset > 0.02 {
        warnings.push(format!("DC offset {dc_offset:.4} > 0.02"));
    }
    if clipped > 0 {
        warnings.push(format!("{clipped} clipped sample(s)"));
    }
    if expect_loop && loop_seam_delta > 0.08 {
        warnings.push(format!("loop seam discontinuity {loop_seam_delta:.4} > 0.08"));
    }

    // Average power spectrum over overlapping 1024-pt Hann windows
    const FFT_N: usize = 1024;
    let hop = FFT_N / 2;
    let mut mag_acc = vec![0.0f64; FFT_N / 2];
    let mut frames = 0usize;
    let sr = buf.sample_rate as f64;
    let mut start = 0usize;
    loop {
        let mut re = vec![0.0; FFT_N];
        let mut im = vec![0.0; FFT_N];
        for k in 0..FFT_N {
            let idx = start + k;
            let s = if idx < n {
                0.5 * (buf.left[idx] + buf.right[idx])
            } else {
                0.0
            };
            let win = 0.5 * (1.0 - (TAU * k as f64 / (FFT_N - 1) as f64).cos());
            re[k] = s * win;
        }
        fft_radix2(&mut re, &mut im);
        for k in 0..(FFT_N / 2) {
            mag_acc[k] += (re[k] * re[k] + im[k] * im[k]).sqrt();
        }
        frames += 1;
        if start + FFT_N >= n {
            break;
        }
        start += hop;
    }

    let bin_hz = sr / FFT_N as f64;
    let mut num = 0.0;
    let mut den = 0.0;
    let mut band_energy = [0.0f64; 6];
    let inv_f = 1.0 / frames.max(1) as f64;

    for (k, &m_sum) in mag_acc.iter().enumerate().skip(1) {
        let mag = m_sum * inv_f;
        let f = k as f64 * bin_hz;
        num += f * mag;
        den += mag;
        let e = mag * mag;
        let bi = if f < 60.0 {
            0
        } else if f < 250.0 {
            1
        } else if f < 800.0 {
            2
        } else if f < 2500.0 {
            3
        } else if f < 6000.0 {
            4
        } else {
            5
        };
        band_energy[bi] += e;
    }
    let centroid = if den > 1e-12 { num / den } else { 0.0 };
    let total_e: f64 = band_energy.iter().sum::<f64>().max(1e-18);
    let mut bands_db = [-60.0; 6];
    for i in 0..6 {
        bands_db[i] = (10.0 * (band_energy[i] / total_e).max(1e-6).log10()).clamp(-60.0, 0.0);
    }

    AudioMetrics {
        duration_sec: buf.duration_sec(),
        sample_rate: buf.sample_rate,
        channels: buf.channels,
        peak_db,
        rms_db,
        dc_offset,
        clipped_samples: clipped,
        loop_seam_delta,
        spectral_centroid_hz: centroid,
        bands_db,
        errors,
        warnings,
    }
}

/// Self-contained 5-panel diagnostic PNG (waveform+RMS, Magma log-spectrogram, 6-band balance, splice/attack zoom).
pub fn render_diagnostic_png(
    buf: &AudioBuffer,
    metrics: &AudioMetrics,
    title: &str,
    is_loop: bool,
    width: u32,
    height: u32,
) -> Vec<u8> {
    let w = width.max(640) as usize;
    let h = height.max(420) as usize;
    let mut c = Canvas::new(w, h, [15, 17, 22, 255]);

    // 1. Header telemetry bar
    c.fill_rect(0, 0, w as i32, 26, [24, 27, 35, 255]);
    c.fill_rect(0, 25, w as i32, 1, [50, 55, 68, 255]);
    let hdr = format!(
        "{} | {:.2}S {}HZ {}CH | PK:{:.1}DB RMS:{:.1}DB DC:{:.3} SEAM:{:.3}",
        title.to_ascii_uppercase(),
        metrics.duration_sec,
        metrics.sample_rate,
        metrics.channels,
        metrics.peak_db,
        metrics.rms_db,
        metrics.dc_offset,
        metrics.loop_seam_delta
    );
    c.draw_text(8, 9, &hdr, [232, 238, 248, 255], 1);

    let pad = 10i32;
    let pw = w as i32 - pad * 2;
    let wf_y = 34i32;
    let wf_h = ((h as i32 - 58) * 26) / 100;
    let sp_y = wf_y + wf_h + 10;
    let sp_h = ((h as i32 - 58) * 38) / 100;
    let bot_y = sp_y + sp_h + 10;
    let bot_h = (h as i32 - bot_y - 8).max(70);

    // 2. Stereo waveform + RMS envelope + red clip ticks
    c.fill_rect(pad, wf_y, pw, wf_h, [20, 23, 30, 255]);
    c.stroke_rect(pad, wf_y, pw, wf_h, [48, 54, 68, 255]);
    let mid_y = wf_y + wf_h / 2;
    c.fill_rect(pad + 1, mid_y, pw - 2, 1, [45, 50, 62, 255]);
    c.draw_text(pad + 4, wf_y + 4, "WAVEFORM L(CYAN) R(AMBER) + RMS", [165, 175, 195, 255], 1);

    let n = buf.len().max(1);
    let inner_w = (pw - 2).max(1) as usize;
    let half_h = (wf_h / 2 - 4).max(1) as f64;
    for col in 0..inner_w {
        let s0 = col * n / inner_w;
        let s1 = (((col + 1) * n) / inner_w).max(s0 + 1).min(n);
        let mut lmin = 1.0f64;
        let mut lmax = -1.0f64;
        let mut rmin = 1.0f64;
        let mut rmax = -1.0f64;
        let mut sq = 0.0f64;
        let mut clip = false;
        for i in s0..s1 {
            let l = buf.left[i];
            let r = buf.right[i];
            lmin = lmin.min(l);
            lmax = lmax.max(l);
            rmin = rmin.min(r);
            rmax = rmax.max(r);
            sq += 0.5 * (l * l + r * r);
            if l.abs() >= 0.999 || r.abs() >= 0.999 {
                clip = true;
            }
        }
        let x = pad + 1 + col as i32;
        let ly0 = mid_y - (lmax * half_h) as i32;
        let ly1 = mid_y - (lmin * half_h) as i32;
        c.fill_rect(x, ly0.min(ly1), 1, (ly1 - ly0).abs().max(1), [70, 200, 235, 175]);
        let ry0 = mid_y - (rmax * half_h) as i32;
        let ry1 = mid_y - (rmin * half_h) as i32;
        c.fill_rect(x, ry0.min(ry1), 1, (ry1 - ry0).abs().max(1), [240, 170, 65, 145]);
        let rms = (sq / (s1 - s0).max(1) as f64).sqrt();
        let ry = (rms * half_h) as i32;
        c.set_px(x, mid_y - ry, [245, 250, 255, 220]);
        c.set_px(x, mid_y + ry, [245, 250, 255, 220]);
        if clip {
            c.fill_rect(x, wf_y + 1, 1, 5, [255, 50, 50, 255]);
        }
    }

    // 3. Log-frequency Spectrogram (40 Hz - 16 kHz) with Magma colormap
    c.fill_rect(pad, sp_y, pw, sp_h, [10, 10, 16, 255]);
    c.stroke_rect(pad, sp_y, pw, sp_h, [48, 54, 68, 255]);
    const FFT_N: usize = 1024;
    let sr = buf.sample_rate as f64;
    let f_min = 40.0f64.ln();
    let f_max = (16000.0f64.min(sr * 0.48)).ln();
    let sp_cols = (pw - 2).max(1) as usize;
    let sp_rows = (sp_h - 2).max(1) as usize;
    let col_step = (sp_cols / 220).max(1);

    let mut x_col = 0usize;
    while x_col < sp_cols {
        let center = (x_col * n) / sp_cols;
        let start = center.saturating_sub(FFT_N / 2);
        let mut re = vec![0.0; FFT_N];
        let mut im = vec![0.0; FFT_N];
        for k in 0..FFT_N {
            let idx = start + k;
            let s = if idx < n {
                0.5 * (buf.left[idx] + buf.right[idx])
            } else {
                0.0
            };
            let win = 0.5 * (1.0 - (TAU * k as f64 / (FFT_N - 1) as f64).cos());
            re[k] = s * win;
        }
        fft_radix2(&mut re, &mut im);
        let cw = col_step.min(sp_cols - x_col) as i32;
        for row in 0..sp_rows {
            let frac = 1.0 - row as f64 / (sp_rows - 1).max(1) as f64;
            let freq = (f_min + frac * (f_max - f_min)).exp();
            let bin = ((freq * FFT_N as f64 / sr).round() as usize).clamp(1, FFT_N / 2 - 1);
            let mag = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt() / (FFT_N as f64 * 0.25);
            let db = 20.0 * mag.max(1e-6).log10();
            let norm = ((db + 78.0) / 72.0).clamp(0.0, 1.0);
            let col = magma(norm);
            c.fill_rect(pad + 1 + x_col as i32, sp_y + 1 + row as i32, cw, 1, col);
        }
        x_col += col_step;
    }
    // Frequency grid labels
    for &(f_hz, lbl) in &[(100.0f64, "100"), (500.0, "500"), (2000.0, "2K"), (8000.0, "8K")] {
        let frac = (f_hz.ln() - f_min) / (f_max - f_min);
        if (0.02..=0.98).contains(&frac) {
            let y = sp_y + 1 + ((1.0 - frac) * (sp_rows - 1) as f64) as i32;
            c.fill_rect(pad + 1, y, pw - 2, 1, [255, 255, 255, 38]);
            c.draw_text(pad + 4, y - 8, lbl, [240, 240, 248, 220], 1);
        }
    }

    // 4. Bottom-left: 6-band frequency balance bar chart
    let left_w = pw / 2 - 5;
    let right_x = pad + left_w + 10;
    let right_w = pw - left_w - 10;
    c.fill_rect(pad, bot_y, left_w, bot_h, [20, 23, 30, 255]);
    c.stroke_rect(pad, bot_y, left_w, bot_h, [48, 54, 68, 255]);
    let b_hdr = format!("6-BAND BALANCE | CENT:{:.0}HZ", metrics.spectral_centroid_hz);
    c.draw_text(pad + 5, bot_y + 4, &b_hdr, [180, 190, 210, 255], 1);

    let labels = ["SUB", "BAS", "LMD", "MID", "HMD", "AIR"];
    let bar_colors = [
        [100, 110, 240, 255],
        [75, 165, 245, 255],
        [65, 215, 190, 255],
        [135, 225, 105, 255],
        [240, 185, 70, 255],
        [240, 105, 95, 255],
    ];
    let slot_w = (left_w - 16) / 6;
    let bar_max_h = (bot_h - 32).max(16);
    for i in 0..6 {
        let bx = pad + 8 + i as i32 * slot_w;
        let norm = ((metrics.bands_db[i] + 48.0) / 48.0).clamp(0.02, 1.0);
        let bh = (norm * bar_max_h as f64) as i32;
        let by = bot_y + bot_h - 14 - bh;
        c.fill_rect(bx + 4, by, (slot_w - 10).max(6), bh, bar_colors[i]);
        c.draw_text(bx + 4, bot_y + bot_h - 11, labels[i], [185, 192, 208, 255], 1);
    }

    // 5. Bottom-right: ±25ms Loop-Seam Splice Zoom OR 50ms Attack Transient Zoom
    c.fill_rect(right_x, bot_y, right_w, bot_h, [20, 23, 30, 255]);
    c.stroke_rect(right_x, bot_y, right_w, bot_h, [48, 54, 68, 255]);
    let zm_mid = bot_y + bot_h / 2 + 4;
    let zm_half = (bot_h / 2 - 14).max(8) as f64;
    c.fill_rect(right_x + 1, zm_mid, right_w - 2, 1, [45, 50, 62, 255]);

    let zm_w = (right_w - 4).max(4) as usize;
    if is_loop {
        c.draw_text(right_x + 5, bot_y + 4, "LOOP SEAM ZOOM (-25MS | +25MS)", [180, 190, 210, 255], 1);
        let half_samps = ((0.025 * sr).round() as usize).min(n / 2).max(1);
        let total_zm = half_samps * 2;
        let mut prev_y = zm_mid;
        for col in 0..zm_w {
            let k = col * total_zm / zm_w;
            let idx = if k < half_samps {
                n - half_samps + k
            } else {
                k - half_samps
            };
            let v = 0.5 * (buf.left[idx] + buf.right[idx]);
            let y = zm_mid - (v * zm_half) as i32;
            let x = right_x + 2 + col as i32;
            if col > 0 {
                c.draw_line(x - 1, prev_y, x, y, [85, 220, 240, 255]);
            }
            prev_y = y;
        }
        // Center seam marker
        let seam_x = right_x + 2 + (zm_w / 2) as i32;
        let seam_col = if metrics.loop_seam_delta <= 0.03 {
            [80, 230, 120, 230]
        } else if metrics.loop_seam_delta <= 0.08 {
            [245, 195, 65, 230]
        } else {
            [250, 75, 75, 240]
        };
        c.fill_rect(seam_x, bot_y + 16, 1, bot_h - 18, seam_col);
    } else {
        c.draw_text(right_x + 5, bot_y + 4, "ATTACK TRANSIENT ZOOM (0..50MS)", [180, 190, 210, 255], 1);
        let span = ((0.050 * sr).round() as usize).min(n).max(1);
        let mut prev_y = zm_mid;
        for col in 0..zm_w {
            let idx = (col * span / zm_w).min(n - 1);
            let v = 0.5 * (buf.left[idx] + buf.right[idx]);
            let y = zm_mid - (v * zm_half) as i32;
            let x = right_x + 2 + col as i32;
            if col > 0 {
                c.draw_line(x - 1, prev_y, x, y, [245, 180, 75, 255]);
            }
            prev_y = y;
        }
    }

    encode_rgba(w as u32, h as u32, &c.rgba)
}

/// In-place Radix-2 Cooley-Tukey FFT (`re.len()` must be a power of 2).
fn fft_radix2(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2usize;
    while len <= n {
        let ang = -2.0 * PI / len as f64;
        let (wlen_i, wlen_r) = ang.sin_cos();
        let half = len >> 1;
        for i in (0..n).step_by(len) {
            let mut wr = 1.0;
            let mut wi = 0.0;
            for k in 0..half {
                let u_r = re[i + k];
                let u_i = im[i + k];
                let v_r = re[i + k + half] * wr - im[i + k + half] * wi;
                let v_i = re[i + k + half] * wi + im[i + k + half] * wr;
                re[i + k] = u_r + v_r;
                im[i + k] = u_i + v_i;
                re[i + k + half] = u_r - v_r;
                im[i + k + half] = u_i - v_i;
                let nwr = wr * wlen_r - wi * wlen_i;
                wi = wr * wlen_i + wi * wlen_r;
                wr = nwr;
            }
        }
        len <<= 1;
    }
}

/// Perceptual Magma colormap approximation for spectrograms.
fn magma(t: f64) -> [u8; 4] {
    const STOPS: [[f64; 3]; 6] = [
        [4.0, 4.0, 12.0],
        [45.0, 17.0, 95.0],
        [124.0, 35.0, 130.0],
        [198.0, 62.0, 108.0],
        [247.0, 132.0, 62.0],
        [252.0, 248.0, 185.0],
    ];
    let x = t.clamp(0.0, 1.0) * (STOPS.len() - 1) as f64;
    let i = (x.floor() as usize).min(STOPS.len() - 2);
    let f = x - i as f64;
    let r = STOPS[i][0] + (STOPS[i + 1][0] - STOPS[i][0]) * f;
    let g = STOPS[i][1] + (STOPS[i + 1][1] - STOPS[i][1]) * f;
    let b = STOPS[i][2] + (STOPS[i + 1][2] - STOPS[i][2]) * f;
    [r as u8, g as u8, b as u8, 255]
}
