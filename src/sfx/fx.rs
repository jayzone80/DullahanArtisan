use std::f64::consts::TAU;

use crate::sfx::dsp::{FilterMode, TptSvf};
use crate::sfx::wav::AudioBuffer;

#[derive(Debug, Clone)]
pub enum FxSpec {
    Filter {
        mode: FilterMode,
        cutoff: f64,
        q: f64,
        gain_db: f64,
        mix: f64,
    },
    Drive {
        amount: f64,
        asym: f64,
        mix: f64,
    },
    Bitcrush {
        bits: f64,
        rate_hz: f64,
        mix: f64,
    },
    Chorus {
        rate: f64,
        depth_ms: f64,
        delay_ms: f64,
        mix: f64,
    },
    Delay {
        time_sec: f64,
        beats: Option<f64>,
        feedback: f64,
        damp_hz: f64,
        ping_pong: bool,
        mix: f64,
    },
    Reverb {
        room: f64,
        damp: f64,
        pre_delay_ms: f64,
        mix: f64,
    },
    Compressor {
        threshold_db: f64,
        ratio: f64,
        attack: f64,
        release: f64,
        makeup_db: f64,
    },
    Width {
        amount: f64, // 0 = mono, 1 = normal, 2 = wide
    },
}

impl FxSpec {
    /// Conservative tail duration estimate for sizing per-voice local FX buffers cleanly.
    pub fn tail_seconds(&self, bpm: f64) -> f64 {
        match self {
            FxSpec::Reverb { room, pre_delay_ms, .. } => {
                pre_delay_ms * 0.001 + 0.4 + room.clamp(0.0, 0.98) * 2.4
            }
            FxSpec::Delay { time_sec, beats, feedback, .. } => {
                let dt = beats
                    .map(|b| b * 60.0 / bpm.max(20.0))
                    .unwrap_or(*time_sec)
                    .max(0.01);
                let fb = feedback.clamp(0.0, 0.92);
                if fb < 0.01 {
                    dt
                } else {
                    (dt * (-60.0 / (20.0 * fb.log10())).clamp(1.0, 12.0)).min(4.0)
                }
            }
            FxSpec::Chorus { delay_ms, depth_ms, .. } => (delay_ms + depth_ms) * 0.002,
            _ => 0.02,
        }
    }
}

pub fn apply_fx_chain(buf: &mut AudioBuffer, chain: &[FxSpec], bpm: f64, cutoff_jitter: f64) {
    for fx in chain {
        apply_fx(buf, fx, bpm, cutoff_jitter);
    }
}

fn apply_fx(buf: &mut AudioBuffer, fx: &FxSpec, bpm: f64, cutoff_jitter: f64) {
    let sr = buf.sample_rate as f64;
    let n = buf.len();
    match *fx {
        FxSpec::Filter {
            mode,
            cutoff,
            q,
            gain_db,
            mix,
        } => {
            let fc = cutoff * cutoff_jitter;
            let m = mix.clamp(0.0, 1.0);
            let mut fl = TptSvf::new();
            let mut fr = TptSvf::new();
            for i in 0..n {
                let dl = buf.left[i];
                let dr = buf.right[i];
                let wl = fl.process(dl, mode, fc, q, gain_db, sr);
                let wr = fr.process(dr, mode, fc, q, gain_db, sr);
                buf.left[i] = dl * (1.0 - m) + wl * m;
                buf.right[i] = dr * (1.0 - m) + wr * m;
            }
        }
        FxSpec::Drive { amount, asym, mix } => {
            let drv = amount.max(0.1);
            let m = mix.clamp(0.0, 1.0);
            let norm = 1.0 / drv.tanh().max(0.25);
            let bias_dc = (asym * drv).tanh();
            for i in 0..n {
                let dl = buf.left[i];
                let dr = buf.right[i];
                let wl = (((dl + asym) * drv).tanh() - bias_dc) * norm;
                let wr = (((dr + asym) * drv).tanh() - bias_dc) * norm;
                buf.left[i] = dl * (1.0 - m) + wl * m;
                buf.right[i] = dr * (1.0 - m) + wr * m;
            }
        }
        FxSpec::Bitcrush { bits, rate_hz, mix } => {
            let levels = 2.0f64.powf(bits.clamp(1.0, 16.0) - 1.0);
            let step = (sr / rate_hz.clamp(200.0, sr)).max(1.0);
            let m = mix.clamp(0.0, 1.0);
            let mut phase = 0.0;
            let mut hl = 0.0;
            let mut hr = 0.0;
            for i in 0..n {
                if phase <= 0.0 {
                    hl = (buf.left[i] * levels).round() / levels;
                    hr = (buf.right[i] * levels).round() / levels;
                    phase += step;
                }
                phase -= 1.0;
                buf.left[i] = buf.left[i] * (1.0 - m) + hl * m;
                buf.right[i] = buf.right[i] * (1.0 - m) + hr * m;
            }
        }
        FxSpec::Chorus {
            rate,
            depth_ms,
            delay_ms,
            mix,
        } => {
            let max_d = (((delay_ms + depth_ms) * 0.001 * sr).ceil() as usize + 8).max(16);
            let mut dl_buf = vec![0.0; max_d];
            let mut dr_buf = vec![0.0; max_d];
            let mut w = 0usize;
            let m = mix.clamp(0.0, 1.0);
            for i in 0..n {
                let xl = buf.left[i];
                let xr = buf.right[i];
                dl_buf[w] = xl;
                dr_buf[w] = xr;
                let t = i as f64 / sr;
                let lfo_l = (TAU * rate * t).sin();
                let lfo_r = (TAU * rate * t + std::f64::consts::FRAC_PI_2).sin();
                let d_l = ((delay_ms + depth_ms * lfo_l) * 0.001 * sr).clamp(1.0, (max_d - 2) as f64);
                let d_r = ((delay_ms + depth_ms * lfo_r) * 0.001 * sr).clamp(1.0, (max_d - 2) as f64);
                let yl = read_frac(&dl_buf, w, d_l);
                let yr = read_frac(&dr_buf, w, d_r);
                buf.left[i] = xl * (1.0 - m) + yl * m;
                buf.right[i] = xr * (1.0 - m) + yr * m;
                w = (w + 1) % max_d;
            }
        }
        FxSpec::Delay {
            time_sec,
            beats,
            feedback,
            damp_hz,
            ping_pong,
            mix,
        } => {
            let dt = beats
                .map(|b| b * 60.0 / bpm.max(20.0))
                .unwrap_or(time_sec)
                .clamp(0.005, 4.0);
            let d_samps = ((dt * sr).round() as usize).max(1);
            let mut dl_buf = vec![0.0; d_samps];
            let mut dr_buf = vec![0.0; d_samps];
            let mut idx = 0usize;
            let fb = feedback.clamp(0.0, 0.94);
            let m = mix.clamp(0.0, 1.0);
            let alpha = (-TAU * damp_hz.clamp(200.0, sr * 0.45) / sr).exp();
            let mut lp_l = 0.0;
            let mut lp_r = 0.0;

            for i in 0..n {
                let xl = buf.left[i];
                let xr = buf.right[i];
                let yl = dl_buf[idx];
                let yr = dr_buf[idx];
                lp_l = yl * (1.0 - alpha) + lp_l * alpha;
                lp_r = yr * (1.0 - alpha) + lp_r * alpha;
                if ping_pong {
                    dl_buf[idx] = xl + lp_r * fb;
                    dr_buf[idx] = xr + lp_l * fb;
                } else {
                    dl_buf[idx] = xl + lp_l * fb;
                    dr_buf[idx] = xr + lp_r * fb;
                }
                buf.left[i] = xl * (1.0 - m) + yl * m;
                buf.right[i] = xr * (1.0 - m) + yr * m;
                idx = (idx + 1) % d_samps;
            }
        }
        FxSpec::Reverb {
            room,
            damp,
            pre_delay_ms,
            mix,
        } => {
            apply_freeverb(buf, room, damp, pre_delay_ms, mix);
        }
        FxSpec::Compressor {
            threshold_db,
            ratio,
            attack,
            release,
            makeup_db,
        } => {
            let atk_c = (-1.0 / (attack.max(0.0005) * sr)).exp();
            let rel_c = (-1.0 / (release.max(0.005) * sr)).exp();
            let ratio = ratio.max(1.0);
            let makeup = 10.0f64.powf(makeup_db / 20.0);
            let mut env = 0.0f64;
            for i in 0..n {
                let peak = buf.left[i].abs().max(buf.right[i].abs());
                let c = if peak > env { atk_c } else { rel_c };
                env = c * env + (1.0 - c) * peak;
                let db = 20.0 * env.max(1e-7).log10();
                let gr_db = if db > threshold_db {
                    (threshold_db - db) * (1.0 - 1.0 / ratio)
                } else {
                    0.0
                };
                let g = 10.0f64.powf(gr_db / 20.0) * makeup;
                buf.left[i] *= g;
                buf.right[i] *= g;
            }
        }
        FxSpec::Width { amount } => {
            let w = amount.clamp(0.0, 2.0);
            for i in 0..n {
                let mid = 0.5 * (buf.left[i] + buf.right[i]);
                let side = 0.5 * (buf.left[i] - buf.right[i]) * w;
                buf.left[i] = mid + side;
                buf.right[i] = mid - side;
            }
        }
    }
}

#[inline]
fn read_frac(buf: &[f64], write_idx: usize, delay: f64) -> f64 {
    let n = buf.len();
    let d0 = delay.floor() as usize;
    let f = delay - d0 as f64;
    let i0 = (write_idx + n - (d0 % n)) % n;
    let i1 = (i0 + n - 1) % n;
    buf[i0] * (1.0 - f) + buf[i1] * f
}

/// Algorithmic stereo Freeverb: 8 parallel damped comb filters + 4 series allpass filters per channel.
fn apply_freeverb(buf: &mut AudioBuffer, room: f64, damp: f64, pre_delay_ms: f64, mix: f64) {
    let sr = buf.sample_rate as f64;
    let scale = sr / 44100.0;
    const COMB_TUNING: [usize; 8] = [1116, 1188, 1277, 1356, 1422, 1491, 1557, 1617];
    const AP_TUNING: [usize; 4] = [556, 441, 341, 225];
    const STEREO_SPREAD: usize = 23;

    let fb = (0.70 + room.clamp(0.0, 1.0) * 0.27).min(0.97);
    let damp1 = damp.clamp(0.0, 0.95) * 0.55;
    let damp2 = 1.0 - damp1;
    let m = mix.clamp(0.0, 1.0);

    struct Comb {
        buf: Vec<f64>,
        idx: usize,
        store: f64,
    }
    impl Comb {
        fn new(len: usize) -> Self {
            Self {
                buf: vec![0.0; len.max(1)],
                idx: 0,
                store: 0.0,
            }
        }
        #[inline]
        fn tick(&mut self, input: f64, fb: f64, damp1: f64, damp2: f64) -> f64 {
            let out = self.buf[self.idx];
            self.store = out * damp2 + self.store * damp1;
            self.buf[self.idx] = input + self.store * fb;
            self.idx += 1;
            if self.idx >= self.buf.len() {
                self.idx = 0;
            }
            out
        }
    }

    struct Allpass {
        buf: Vec<f64>,
        idx: usize,
    }
    impl Allpass {
        fn new(len: usize) -> Self {
            Self {
                buf: vec![0.0; len.max(1)],
                idx: 0,
            }
        }
        #[inline]
        fn tick(&mut self, input: f64) -> f64 {
            let bo = self.buf[self.idx];
            let out = -input + bo;
            self.buf[self.idx] = input + bo * 0.5;
            self.idx += 1;
            if self.idx >= self.buf.len() {
                self.idx = 0;
            }
            out
        }
    }

    let mut combs_l: Vec<Comb> = COMB_TUNING
        .iter()
        .map(|&l| Comb::new((l as f64 * scale).round() as usize))
        .collect();
    let mut combs_r: Vec<Comb> = COMB_TUNING
        .iter()
        .map(|&l| Comb::new(((l + STEREO_SPREAD) as f64 * scale).round() as usize))
        .collect();
    let mut aps_l: Vec<Allpass> = AP_TUNING
        .iter()
        .map(|&l| Allpass::new((l as f64 * scale).round() as usize))
        .collect();
    let mut aps_r: Vec<Allpass> = AP_TUNING
        .iter()
        .map(|&l| Allpass::new(((l + STEREO_SPREAD) as f64 * scale).round() as usize))
        .collect();

    let pre_samps = ((pre_delay_ms.max(0.0) * 0.001 * sr).round() as usize).max(1);
    let mut pre_buf = vec![0.0; pre_samps];
    let mut pre_idx = 0usize;

    for i in 0..buf.len() {
        let xl = buf.left[i];
        let xr = buf.right[i];
        let mono_in = (xl + xr) * 0.025;
        let fed = if pre_samps > 1 {
            let v = pre_buf[pre_idx];
            pre_buf[pre_idx] = mono_in;
            pre_idx = (pre_idx + 1) % pre_samps;
            v
        } else {
            mono_in
        };

        let mut ol = 0.0;
        let mut or = 0.0;
        for c in &mut combs_l {
            ol += c.tick(fed, fb, damp1, damp2);
        }
        for c in &mut combs_r {
            or += c.tick(fed, fb, damp1, damp2);
        }
        for ap in &mut aps_l {
            ol = ap.tick(ol);
        }
        for ap in &mut aps_r {
            or = ap.tick(or);
        }
        buf.left[i] = xl * (1.0 - m) + ol * m;
        buf.right[i] = xr * (1.0 - m) + or * m;
    }
}

/// First-order high-pass DC blocker (~15 Hz, R ≈ 0.9978 at 44.1 kHz).
pub fn remove_dc_offset(buf: &mut AudioBuffer) {
    let r = (-TAU * 15.0 / buf.sample_rate.max(1) as f64).exp();
    let mut xl1 = 0.0;
    let mut yl1 = 0.0;
    let mut xr1 = 0.0;
    let mut yr1 = 0.0;
    for i in 0..buf.len() {
        let xl = buf.left[i];
        yl1 = xl - xl1 + r * yl1;
        xl1 = xl;
        buf.left[i] = yl1;

        let xr = buf.right[i];
        yr1 = xr - xr1 + r * yr1;
        xr1 = xr;
        buf.right[i] = yr1;
    }
}

/// Soft-knee safety limiter + optional peak normalization (e.g. target_peak_db = -1.0).
pub fn master_limit_and_normalize(buf: &mut AudioBuffer, target_peak_db: f64, normalize: bool) {
    let mut peak = 0.0f64;
    for i in 0..buf.len() {
        peak = peak.max(buf.left[i].abs()).max(buf.right[i].abs());
    }
    if peak < 1e-9 {
        return;
    }
    let target = 10.0f64.powf(target_peak_db.clamp(-24.0, -0.1) / 20.0);
    if normalize {
        let g = target / peak;
        for i in 0..buf.len() {
            buf.left[i] *= g;
            buf.right[i] *= g;
        }
    }
    // Gentle soft-knee safety ceiling at 0.985 so nothing ever clips PCM16
    let ceil = 0.985;
    let knee = 0.88;
    let limit = |x: f64| -> f64 {
        let a = x.abs();
        if a <= knee {
            x
        } else {
            let over = (a - knee) / (ceil - knee);
            x.signum() * (knee + (ceil - knee) * over.tanh())
        }
    };
    for i in 0..buf.len() {
        buf.left[i] = limit(buf.left[i]);
        buf.right[i] = limit(buf.right[i]);
    }
}
