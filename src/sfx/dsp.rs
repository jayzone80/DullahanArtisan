use std::f64::consts::{PI, TAU};

use crate::common::math::Rng;
use crate::sfx::theory::snap_cycle_locked_hz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WaveType {
    Sine,
    Triangle,
    Saw,
    Square,
    Pulse,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NoiseColor {
    White,
    Pink,
    Brown,
    Crackle,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilterMode {
    LowPass,
    HighPass,
    BandPass,
    Notch,
    Bell,
}

/// PolyBLEP residual for band-limited discontinuities (dt = freq / sr, t in [0, 1)).
#[inline]
fn poly_blep(t: f64, dt: f64) -> f64 {
    if dt <= 0.0 {
        return 0.0;
    }
    if t < dt {
        let x = t / dt;
        x + x - x * x - 1.0
    } else if t > 1.0 - dt {
        let x = (t - 1.0) / dt;
        x * x + x + x + 1.0
    } else {
        0.0
    }
}

/// Anti-aliased oscillator evaluation at normalized phase `p` in `[0, 1)` and phase step `dt = f / sr`.
#[inline]
pub fn eval_wave(wave: WaveType, phase: f64, dt: f64, pwm: f64) -> f64 {
    let p = phase.rem_euclid(1.0);
    match wave {
        WaveType::Sine => (TAU * p).sin(),
        WaveType::Triangle => {
            if p < 0.25 {
                4.0 * p
            } else if p < 0.75 {
                2.0 - 4.0 * p
            } else {
                4.0 * p - 4.0
            }
        }
        WaveType::Saw => {
            let raw = 2.0 * p - 1.0;
            raw - poly_blep(p, dt)
        }
        WaveType::Square => {
            let raw = if p < 0.5 { 1.0 } else { -1.0 };
            let mut v = raw + poly_blep(p, dt);
            v -= poly_blep((p + 0.5).rem_euclid(1.0), dt);
            v
        }
        WaveType::Pulse => {
            let duty = pwm.clamp(0.05, 0.95);
            let raw = if p < duty { 1.0 } else { -1.0 };
            let mut v = raw + poly_blep(p, dt);
            v -= poly_blep((p - duty).rem_euclid(1.0), dt);
            // DC-compensate pulse width
            v - (2.0 * duty - 1.0)
        }
    }
}

/// 4-color noise generator (white, Paul Kellet 7-tap pink, leaky brown, sparse crackle).
pub struct NoiseGen {
    color: NoiseColor,
    rng: Rng,
    b0: f64,
    b1: f64,
    b2: f64,
    b3: f64,
    b4: f64,
    b5: f64,
    b6: f64,
    brown: f64,
    density_prob: f64,
}

impl NoiseGen {
    pub fn new(color: NoiseColor, seed: u64, density_hz: f64, sr: f64) -> Self {
        Self {
            color,
            rng: Rng::new(seed),
            b0: 0.0,
            b1: 0.0,
            b2: 0.0,
            b3: 0.0,
            b4: 0.0,
            b5: 0.0,
            b6: 0.0,
            brown: 0.0,
            density_prob: (density_hz / sr.max(1.0)).clamp(0.0, 0.5),
        }
    }

    #[inline]
    pub fn next(&mut self) -> f64 {
        let w = self.rng.f64() * 2.0 - 1.0;
        match self.color {
            NoiseColor::White => w,
            NoiseColor::Pink => {
                // Paul Kellet refined 7-tap -3 dB/oct pink filter
                self.b0 = 0.99886 * self.b0 + w * 0.0555179;
                self.b1 = 0.99332 * self.b1 + w * 0.0750759;
                self.b2 = 0.96900 * self.b2 + w * 0.1538520;
                self.b3 = 0.86650 * self.b3 + w * 0.3104856;
                self.b4 = 0.55000 * self.b4 + w * 0.5329522;
                self.b5 = -0.7616 * self.b5 - w * 0.0168980;
                let out = (self.b0 + self.b1 + self.b2 + self.b3 + self.b4 + self.b5 + self.b6 + w * 0.5362) * 0.11;
                self.b6 = w * 0.115926;
                out.clamp(-1.0, 1.0)
            }
            NoiseColor::Brown => {
                self.brown = (self.brown + 0.028 * w) * 0.996;
                (self.brown * 3.2).clamp(-1.0, 1.0)
            }
            NoiseColor::Crackle => {
                if self.rng.f64() < self.density_prob {
                    w
                } else {
                    0.0
                }
            }
        }
    }
}

/// Cytomic / Andrew Simper TPT State-Variable Filter — unconditionally stable under audio-rate cutoff modulation.
#[derive(Debug, Clone)]
pub struct TptSvf {
    ic1eq: f64,
    ic2eq: f64,
}

impl TptSvf {
    pub fn new() -> Self {
        Self {
            ic1eq: 0.0,
            ic2eq: 0.0,
        }
    }

    #[inline]
    pub fn process(
        &mut self,
        v0: f64,
        mode: FilterMode,
        cutoff_hz: f64,
        q: f64,
        gain_db: f64,
        sr: f64,
    ) -> f64 {
        let fc = cutoff_hz.clamp(12.0, sr * 0.47);
        let q = q.clamp(0.15, 24.0);
        let g = (PI * fc / sr).tan();

        match mode {
            FilterMode::Bell => {
                let a = 10.0f64.powf(gain_db / 40.0);
                let k = 1.0 / (q * a);
                let a1 = 1.0 / (1.0 + g * (g + k));
                let a2 = g * a1;
                let a3 = g * a2;
                let v3 = v0 - self.ic2eq;
                let v1 = a1 * self.ic1eq + a2 * v3;
                let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
                self.ic1eq = 2.0 * v1 - self.ic1eq;
                self.ic2eq = 2.0 * v2 - self.ic2eq;
                v0 + (a * a - 1.0) * k * v1
            }
            _ => {
                let k = 1.0 / q;
                let a1 = 1.0 / (1.0 + g * (g + k));
                let a2 = g * a1;
                let a3 = g * a2;
                let v3 = v0 - self.ic2eq;
                let v1 = a1 * self.ic1eq + a2 * v3;
                let v2 = self.ic2eq + a2 * self.ic1eq + a3 * v3;
                self.ic1eq = 2.0 * v1 - self.ic1eq;
                self.ic2eq = 2.0 * v2 - self.ic2eq;
                match mode {
                    FilterMode::LowPass => v2,
                    FilterMode::HighPass => v0 - k * v1 - v2,
                    FilterMode::BandPass => k * v1, // unity-peak scaled
                    FilterMode::Notch => v0 - k * v1,
                    FilterMode::Bell => unreachable!(),
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
pub enum EnvSpec {
    Adsr {
        attack: f64,
        decay: f64,
        sustain: f64,
        release: f64,
    },
    Points(Vec<(f64, f64)>),
}

impl EnvSpec {
    pub fn eval(&self, t: f64, gate_dur: f64) -> f64 {
        match self {
            EnvSpec::Adsr {
                attack,
                decay,
                sustain,
                release,
            } => {
                let a = attack.max(0.0005);
                let d = decay.max(0.0005);
                let r = release.max(0.0005);
                let s = sustain.clamp(0.0, 1.0);
                let hold_end = (gate_dur - r).max(a);

                if t < 0.0 {
                    0.0
                } else if t < a {
                    t / a
                } else if t < hold_end {
                    let dt = t - a;
                    if dt < d {
                        // Exponential-shaped decay toward sustain
                        let u = dt / d;
                        s + (1.0 - s) * (1.0 - u) * (1.0 - u)
                    } else {
                        s
                    }
                } else {
                    let level_at_rel = if hold_end - a < d {
                        let u = ((hold_end - a) / d).clamp(0.0, 1.0);
                        s + (1.0 - s) * (1.0 - u) * (1.0 - u)
                    } else {
                        s
                    };
                    let rt = (t - hold_end) / r;
                    if rt >= 1.0 {
                        0.0
                    } else {
                        level_at_rel * (1.0 - rt) * (1.0 - rt)
                    }
                }
            }
            EnvSpec::Points(pts) => eval_piecewise(pts, t),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SweepCurve {
    Exp,
    Lin,
    Log,
    Smooth,
}

/// Pitch or cutoff sweep envelope (`start>end:dur[:curve]` or piecewise `t0:v0,t1:v1,...[:curve]`).
#[derive(Debug, Clone)]
pub struct SweepSpec {
    pub points: Vec<(f64, f64)>,
    pub curve: SweepCurve,
}

impl SweepSpec {
    pub fn eval(&self, t: f64) -> f64 {
        if self.points.is_empty() {
            return 0.0;
        }
        if t <= self.points[0].0 {
            return self.points[0].1;
        }
        for w in self.points.windows(2) {
            let (t0, v0) = w[0];
            let (t1, v1) = w[1];
            if t <= t1 {
                let u = ((t - t0) / (t1 - t0).max(1e-9)).clamp(0.0, 1.0);
                return match self.curve {
                    SweepCurve::Exp if v0 > 0.0 && v1 > 0.0 => v0 * (v1 / v0).powf(u),
                    SweepCurve::Lin | SweepCurve::Exp => v0 + (v1 - v0) * u,
                    SweepCurve::Log => {
                        let ul = (1.0 + u * 9.0).log10(); // fast initial rise, slow settle
                        v0 + (v1 - v0) * ul
                    }
                    SweepCurve::Smooth => {
                        let us = u * u * (3.0 - 2.0 * u);
                        if v0 > 0.0 && v1 > 0.0 {
                            v0 * (v1 / v0).powf(us)
                        } else {
                            v0 + (v1 - v0) * us
                        }
                    }
                };
            }
        }
        self.points.last().unwrap().1
    }
}

fn eval_piecewise(pts: &[(f64, f64)], t: f64) -> f64 {
    if pts.is_empty() {
        return 1.0;
    }
    if t <= pts[0].0 {
        return pts[0].1;
    }
    for w in pts.windows(2) {
        if t <= w[1].0 {
            let u = ((t - w[0].0) / (w[1].0 - w[0].0).max(1e-9)).clamp(0.0, 1.0);
            return w[0].1 + (w[1].1 - w[0].1) * u;
        }
    }
    pts.last().unwrap().1
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoTarget {
    Gain,
    Pitch,  // semitones
    Cutoff, // Hz offset
    Pan,
    Pwm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LfoWave {
    Sine,
    Triangle,
    Saw,
    Square,
    Snh,
    Random,
}

#[derive(Debug, Clone)]
pub struct LfoSpec {
    pub target: LfoTarget,
    pub rate: f64,
    pub depth: f64,
    pub wave: LfoWave,
    pub phase: f64, // 0..1
    pub cycle_lock: bool,
}

impl LfoSpec {
    pub fn eval(&self, t: f64, loop_dur: Option<f64>, seed: u64) -> f64 {
        let rate = match (self.cycle_lock, loop_dur) {
            (true, Some(ld)) => snap_cycle_locked_hz(self.rate, ld),
            _ => self.rate,
        };
        let cyc = t * rate + self.phase;
        let p = cyc.rem_euclid(1.0);
        let raw = match self.wave {
            LfoWave::Sine => (TAU * p).sin(),
            LfoWave::Triangle => {
                if p < 0.25 {
                    4.0 * p
                } else if p < 0.75 {
                    2.0 - 4.0 * p
                } else {
                    4.0 * p - 4.0
                }
            }
            LfoWave::Saw => 2.0 * p - 1.0,
            LfoWave::Square => {
                if p < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            }
            LfoWave::Snh => {
                let idx = cyc.floor() as i64;
                hash_signed(idx, seed)
            }
            LfoWave::Random => {
                let idx = cyc.floor() as i64;
                let a = hash_signed(idx, seed);
                let b = hash_signed(idx + 1, seed);
                let s = p * p * (3.0 - 2.0 * p);
                a + (b - a) * s
            }
        };
        raw * self.depth
    }
}

fn hash_signed(idx: i64, seed: u64) -> f64 {
    let mut x = (idx as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed;
    x = (x ^ (x >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x = (x ^ (x >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    ((x >> 11) as f64 / 9007199254740992.0) * 2.0 - 1.0
}

/// Karplus-Strong plucked string synthesizer with fractional delay and one-pole damping.
pub fn synth_pluck(
    samples: usize,
    sr: f64,
    freq: f64,
    damping: f64,
    brightness: f64,
    seed: u64,
) -> Vec<f64> {
    let mut out = vec![0.0; samples];
    let delay_f = (sr / freq.clamp(20.0, sr * 0.45)).max(2.0);
    let delay_i = delay_f.floor() as usize;
    let frac = delay_f - delay_i as f64;
    let ring_len = delay_i + 2;
    let mut ring = vec![0.0; ring_len];

    // Fill initial burst with brightness-filtered, DC-removed white noise
    let mut rng = Rng::new(seed);
    let lp_a = brightness.clamp(0.05, 0.98);
    let mut lp = 0.0;
    let mut sum = 0.0;
    for r in ring.iter_mut().take(delay_i + 1) {
        let w = rng.f64() * 2.0 - 1.0;
        lp += lp_a * (w - lp);
        *r = lp;
        sum += lp;
    }
    let mean = sum / (delay_i + 1) as f64;
    for r in ring.iter_mut().take(delay_i + 1) {
        *r -= mean;
    }

    let damp = damping.clamp(0.85, 0.9995);
    let mut write_idx = 0usize;
    let mut prev_s = 0.0;

    for s in out.iter_mut() {
        let r0 = (write_idx + ring_len - delay_i) % ring_len;
        let r1 = (r0 + ring_len - 1) % ring_len;
        let val = ring[r0] * (1.0 - frac) + ring[r1] * frac;
        *s = val;
        let filtered = 0.5 * (val + prev_s) * damp;
        prev_s = val;
        ring[write_idx] = filtered;
        write_idx = (write_idx + 1) % ring_len;
    }
    out
}
