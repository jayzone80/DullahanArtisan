use std::f64::consts::{FRAC_PI_2, TAU};

use crate::common::math::Rng;
use crate::sfx::dsp::{
    eval_wave, synth_pluck, FilterMode, LfoTarget, NoiseGen, TptSvf,
};
use crate::sfx::fx::{apply_fx_chain, master_limit_and_normalize, remove_dc_offset, FxSpec};
use crate::sfx::parser::{CrossfadeCurve, Layer, LayerKind, SoundDoc};
use crate::sfx::theory::{semitones_to_ratio, snap_cycle_locked_hz};
use crate::sfx::wav::AudioBuffer;

#[derive(Debug, Clone, Copy)]
struct RenderCtx {
    sr: f64,
    bpm: f64,
    loop_dur: Option<f64>,
    seed: u64,
    pitch_ratio: f64,
    cutoff_jitter: f64,
}

/// Render a `SoundDoc` into a finished stereo/mono `AudioBuffer`.
/// For `variation_index > 0`, applies deterministic seed offset + bounded pitch/cutoff micro-jitter.
pub fn render_sound(
    doc: &SoundDoc,
    variation_index: usize,
    seed_override: Option<u64>,
) -> Result<AudioBuffer, String> {
    let base_seed = seed_override
        .unwrap_or(doc.seed)
        .wrapping_add((variation_index as u64).wrapping_mul(7919));
    let mut vrng = Rng::new(base_seed ^ 0xA5A5_1234);

    let (var_pitch_ratio, var_cutoff_jitter) = if variation_index > 0 {
        let semi = vrng.range_f64(-0.9, 0.9);
        let cf = vrng.range_f64(0.90, 1.10);
        (semitones_to_ratio(semi), cf)
    } else {
        (1.0, 1.0)
    };

    let sr = doc.sample_rate.max(8000);
    let sr_f = sr as f64;
    let cycle_sec = doc.duration.max(0.02);
    let cycle_samples = (cycle_sec * sr_f).round() as usize;

    let (total_sec, xfade_samples, cycles) = if doc.loop_mode {
        let xf = doc.crossfade.clamp(0.005, cycle_sec * 0.45);
        let xf_s = (xf * sr_f).round() as usize;
        // 2 full cycles + crossfade continuation tail
        (cycle_sec * 2.0 + xf, xf_s, 2usize)
    } else {
        (cycle_sec, 0usize, 1usize)
    };

    let total_samples = (total_sec * sr_f).round().max(1.0) as usize;
    let mut mix = AudioBuffer::new(sr, doc.channels, total_samples);

    let ctx = RenderCtx {
        sr: sr_f,
        bpm: doc.bpm,
        loop_dur: if doc.loop_mode { Some(cycle_sec) } else { None },
        seed: base_seed,
        pitch_ratio: var_pitch_ratio,
        cutoff_jitter: var_cutoff_jitter,
    };

    for (li, layer) in doc.layers.iter().enumerate() {
        if layer.common.muted {
            continue;
        }
        let lctx = RenderCtx {
            seed: ctx.seed.wrapping_add((li as u64 + 1).wrapping_mul(31337)),
            ..ctx
        };
        render_layer_into(&mut mix, doc, layer, lctx, 0.0, total_sec, cycle_sec, cycles, 0)?;
    }

    // Master FX chain runs continuously across full pre-warmed buffer so delays/reverbs ring into cycle 1!
    if !doc.master_fx.is_empty() {
        apply_fx_chain(&mut mix, &doc.master_fx, doc.bpm, var_cutoff_jitter);
    }

    remove_dc_offset(&mut mix);

    let mut final_buf = if doc.loop_mode {
        // Extract Cycle 1 ([cycle_samples .. 2 * cycle_samples)) and fold continuation tail onto start
        let mut out = mix.extract_slice(cycle_samples, cycle_samples);
        let tail_start = cycle_samples * 2;
        let xf = xfade_samples.min(cycle_samples / 2).min(mix.len().saturating_sub(tail_start));
        for i in 0..xf {
            let u = i as f64 / xf.max(1) as f64;
            let (w_tail, w_head) = match doc.curve {
                CrossfadeCurve::Linear => (1.0 - u, u),
                CrossfadeCurve::EqualPower => ((u * FRAC_PI_2).cos(), (u * FRAC_PI_2).sin()),
            };
            out.left[i] = out.left[i] * w_head + mix.left[tail_start + i] * w_tail;
            out.right[i] = out.right[i] * w_head + mix.right[tail_start + i] * w_tail;
        }
        out
    } else {
        // Anti-click ~2 ms end fade on one-shots
        let fade = ((0.002 * sr_f).round() as usize).min(mix.len() / 4);
        let n = mix.len();
        for i in 0..fade {
            let g = (fade - i) as f64 / fade.max(1) as f64;
            mix.left[n - fade + i] *= g;
            mix.right[n - fade + i] *= g;
        }
        mix
    };

    master_limit_and_normalize(&mut final_buf, doc.norm_db, doc.normalize);
    Ok(final_buf)
}

#[allow(clippy::too_many_arguments)]
fn render_layer_into(
    dest: &mut AudioBuffer,
    doc: &SoundDoc,
    layer: &Layer,
    ctx: RenderCtx,
    base_offset_sec: f64,
    parent_dur: f64,
    cycle_sec: f64,
    cycles: usize,
    depth: usize,
) -> Result<(), String> {
    if depth > 12 || layer.common.muted {
        return Ok(());
    }

    let c = &layer.common;
    match &layer.kind {
        // Continuous/leaf generators: when depth == 0 and looping, span the full pre-warmed buffer if no explicit dur
        LayerKind::Osc { .. }
        | LayerKind::Supersaw { .. }
        | LayerKind::Fm { .. }
        | LayerKind::Modal { .. }
        | LayerKind::Pluck { .. }
        | LayerKind::Noise { .. } => {
            let is_repeating_oneshot = depth == 0 && cycles > 1 && c.duration.is_some();
            let rep_count = if is_repeating_oneshot { cycles + 1 } else { 1 };
            for cyc in 0..rep_count {
                let start_sec = base_offset_sec + c.start_sec + cyc as f64 * cycle_sec;
                let dur = c.duration.unwrap_or_else(|| (parent_dur - c.start_sec).max(0.01));
                let tail = max_fx_tail(&c.fx, ctx.bpm);
                let total_local = dur + if depth > 0 { tail } else { 0.0 };
                let local_n = (total_local * ctx.sr).round().max(1.0) as usize;
                let mut local = AudioBuffer::new(dest.sample_rate, dest.channels, local_n);
                let leaf_n = (dur * ctx.sr).round().max(1.0) as usize;
                let consumed_first_flt = synth_leaf(&mut local, leaf_n, dur, layer, ctx);
                let fx_slice = if consumed_first_flt && !c.fx.is_empty() {
                    &c.fx[1..]
                } else {
                    &c.fx[..]
                };
                if !fx_slice.is_empty() {
                    apply_fx_chain(&mut local, fx_slice, ctx.bpm, ctx.cutoff_jitter);
                }
                let off_s = (start_sec.max(0.0) * ctx.sr).round() as usize;
                dest.add_scaled(&local, c.gain, c.pan, off_s);
            }
        }
        LayerKind::Group { children } => {
            let dur = c.duration.unwrap_or(parent_dur);
            let tail = if depth > 0 { max_fx_tail(&c.fx, ctx.bpm) } else { 0.0 };
            let local_n = ((dur + tail) * ctx.sr).round().max(1.0) as usize;
            let mut local = AudioBuffer::new(dest.sample_rate, dest.channels, local_n);
            for (ci, child) in children.iter().enumerate() {
                let cctx = RenderCtx {
                    seed: ctx.seed.wrapping_add((ci as u64 + 1) * 4099),
                    ..ctx
                };
                render_layer_into(&mut local, doc, child, cctx, 0.0, dur, cycle_sec, 1, depth + 1)?;
            }
            if !c.fx.is_empty() {
                apply_fx_chain(&mut local, &c.fx, ctx.bpm, ctx.cutoff_jitter);
            }
            let off_s = ((base_offset_sec + c.start_sec).max(0.0) * ctx.sr).round() as usize;
            dest.add_scaled(&local, c.gain, c.pan, off_s);
        }
        LayerKind::Ref {
            voice,
            freq_override,
            semitones,
        } => {
            let rep_count = if depth == 0 && cycles > 1 { cycles + 1 } else { 1 };
            for cyc in 0..rep_count {
                let start_sec = base_offset_sec + c.start_sec + cyc as f64 * cycle_sec;
                let dur = c.duration.unwrap_or_else(|| estimate_voice_dur(doc, voice).unwrap_or(0.35));
                let mut vctx = ctx;
                vctx.pitch_ratio *= semitones_to_ratio(*semitones);
                trigger_voice(
                    dest,
                    doc,
                    voice,
                    vctx,
                    start_sec,
                    dur,
                    c.gain,
                    c.pan,
                    *freq_override,
                    &c.fx,
                    depth + 1,
                )?;
            }
        }
        LayerKind::Pattern {
            voice,
            steps,
            step_beats,
            swing,
            repeat,
        } => {
            // Render track into a dedicated track buffer so shared track FX (reverb/delay) run ONCE!
            let mut track = AudioBuffer::new(dest.sample_rate, dest.channels, dest.len());
            let beat_sec = 60.0 / ctx.bpm.max(20.0);
            let step_sec = step_beats * beat_sec;
            let chars: Vec<char> = steps.chars().filter(|ch| !ch.is_whitespace() && *ch != '|').collect();
            let rep_cycles = if depth == 0 && cycles > 1 { cycles + 1 } else { 1 };

            for cyc in 0..rep_cycles {
                let cyc_off = base_offset_sec + c.start_sec + cyc as f64 * cycle_sec;
                let mut step_idx = 0usize;
                for _r in 0..(*repeat) {
                    for &ch in &chars {
                        let vel = match ch {
                            'X' => 1.0,
                            'x' => 0.72,
                            'o' => 0.52,
                            'g' => 0.28,
                            _ => 0.0,
                        };
                        if vel > 0.0 {
                            let swing_off = if step_idx % 2 == 1 { swing * step_sec } else { 0.0 };
                            let t = cyc_off + step_idx as f64 * step_sec + swing_off;
                            if t < track.duration_sec() {
                                let vdur = c
                                    .duration
                                    .unwrap_or_else(|| estimate_voice_dur(doc, voice).unwrap_or(step_sec * 1.2));
                                let vctx = RenderCtx {
                                    seed: ctx.seed.wrapping_add((step_idx as u64 + 1) * 131 + cyc as u64 * 997),
                                    ..ctx
                                };
                                trigger_voice(&mut track, doc, voice, vctx, t, vdur, vel, 0.0, None, &[], depth + 1)?;
                            }
                        }
                        step_idx += 1;
                    }
                }
            }
            if !c.fx.is_empty() {
                apply_fx_chain(&mut track, &c.fx, ctx.bpm, ctx.cutoff_jitter);
            }
            dest.add_scaled(&track, c.gain, c.pan, 0);
        }
        LayerKind::Sequence {
            voice,
            events,
            gate,
            repeat,
            transpose,
        } => {
            let mut track = AudioBuffer::new(dest.sample_rate, dest.channels, dest.len());
            let beat_sec = 60.0 / ctx.bpm.max(20.0);
            let tr_ratio = semitones_to_ratio(*transpose);
            let rep_cycles = if depth == 0 && cycles > 1 { cycles + 1 } else { 1 };

            for cyc in 0..rep_cycles {
                let mut cur_t = base_offset_sec + c.start_sec + cyc as f64 * cycle_sec;
                let mut ev_idx = 0usize;
                for _r in 0..(*repeat) {
                    for ev in events {
                        let dur_s = ev.beats * beat_sec;
                        let note_dur = (dur_s * gate.clamp(0.05, 1.5)).max(0.015);
                        if cur_t < track.duration_sec() {
                            let poly_scale = if ev.notes_hz.len() > 1 {
                                1.0 / (ev.notes_hz.len() as f64).sqrt()
                            } else {
                                1.0
                            };
                            for (ni, &hz) in ev.notes_hz.iter().enumerate() {
                                let vctx = RenderCtx {
                                    seed: ctx.seed.wrapping_add((ev_idx as u64 + 1) * 521 + ni as u64 * 73),
                                    ..ctx
                                };
                                trigger_voice(
                                    &mut track,
                                    doc,
                                    voice,
                                    vctx,
                                    cur_t,
                                    note_dur,
                                    ev.velocity * poly_scale,
                                    0.0,
                                    Some(hz * tr_ratio),
                                    &[],
                                    depth + 1,
                                )?;
                            }
                        }
                        cur_t += dur_s;
                        ev_idx += 1;
                    }
                }
            }
            if !c.fx.is_empty() {
                apply_fx_chain(&mut track, &c.fx, ctx.bpm, ctx.cutoff_jitter);
            }
            dest.add_scaled(&track, c.gain, c.pan, 0);
        }
        LayerKind::Scatter {
            voice,
            count,
            win_start,
            win_end,
            min_gap,
            pitch_lo,
            pitch_hi,
            gain_lo,
            gain_hi,
            pan_lo,
            pan_hi,
            seed,
        } => {
            let mut track = AudioBuffer::new(dest.sample_rate, dest.channels, dest.len());
            let mut rng = Rng::new(ctx.seed ^ seed.wrapping_mul(65537));
            let w0 = win_start.max(0.0);
            let w1 = win_end.max(w0 + 0.01);

            // Pick event times respecting min_gap
            let mut times = Vec::with_capacity(*count);
            for _ in 0..(*count * 12) {
                if times.len() >= *count {
                    break;
                }
                let t = rng.range_f64(w0, w1);
                if times.iter().all(|&prev: &f64| (prev - t).abs() >= *min_gap) {
                    times.push(t);
                }
            }
            times.sort_by(|a, b| a.total_cmp(b));

            let mut events = Vec::with_capacity(times.len());
            for t in times {
                events.push((
                    t,
                    rng.range_f64(*pitch_lo, *pitch_hi),
                    rng.range_f64(*gain_lo, *gain_hi),
                    rng.range_f64(*pan_lo, *pan_hi),
                    rng.next_u64(),
                ));
            }

            let rep_cycles = if depth == 0 && cycles > 1 { cycles + 1 } else { 1 };
            for cyc in 0..rep_cycles {
                let cyc_off = base_offset_sec + cyc as f64 * cycle_sec;
                for &(t, semi, g, pan, eseed) in &events {
                    let abs_t = cyc_off + t;
                    if abs_t < track.duration_sec() {
                        let vdur = c
                            .duration
                            .unwrap_or_else(|| estimate_voice_dur(doc, voice).unwrap_or(0.08));
                        let mut vctx = ctx;
                        vctx.seed = eseed;
                        vctx.pitch_ratio *= semitones_to_ratio(semi);
                        trigger_voice(&mut track, doc, voice, vctx, abs_t, vdur, g, pan, None, &[], depth + 1)?;
                    }
                }
            }
            if !c.fx.is_empty() {
                apply_fx_chain(&mut track, &c.fx, ctx.bpm, ctx.cutoff_jitter);
            }
            dest.add_scaled(&track, c.gain, c.pan, 0);
        }
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
fn trigger_voice(
    dest: &mut AudioBuffer,
    doc: &SoundDoc,
    voice_name: &str,
    ctx: RenderCtx,
    start_sec: f64,
    dur: f64,
    gain: f64,
    pan: f64,
    freq_override: Option<f64>,
    extra_fx: &[FxSpec],
    depth: usize,
) -> Result<(), String> {
    let layers = doc
        .defs
        .get(voice_name)
        .ok_or_else(|| format!("unknown voice '{voice_name}'"))?;
    let tail = max_fx_tail(extra_fx, ctx.bpm)
        .max(layers.iter().map(|l| max_fx_tail(&l.common.fx, ctx.bpm)).fold(0.0, f64::max));
    let buf_dur = (dur + tail).min(dest.duration_sec());
    let local_n = (buf_dur * ctx.sr).round().max(1.0) as usize;
    let mut vbuf = AudioBuffer::new(dest.sample_rate, dest.channels, local_n);

    for (i, l) in layers.iter().enumerate() {
        let mut cloned = l.clone();
        if let Some(f) = freq_override {
            override_layer_freq(&mut cloned, f);
        }
        if cloned.common.duration.is_none() {
            cloned.common.duration = Some(dur);
        }
        let lctx = RenderCtx {
            seed: ctx.seed.wrapping_add((i as u64 + 1) * 773),
            ..ctx
        };
        render_layer_into(&mut vbuf, doc, &cloned, lctx, 0.0, dur, dur, 1, depth + 1)?;
    }

    if !extra_fx.is_empty() {
        apply_fx_chain(&mut vbuf, extra_fx, ctx.bpm, ctx.cutoff_jitter);
    }
    let off = (start_sec.max(0.0) * ctx.sr).round() as usize;
    dest.add_scaled(&vbuf, gain, pan, off);
    Ok(())
}

fn override_layer_freq(layer: &mut Layer, target_hz: f64) {
    match &mut layer.kind {
        LayerKind::Osc { freq, .. }
        | LayerKind::Supersaw { freq, .. }
        | LayerKind::Fm { freq, .. }
        | LayerKind::Modal { freq, .. }
        | LayerKind::Pluck { freq, .. } => {
            *freq = target_hz;
        }
        LayerKind::Group { children } => {
            for ch in children {
                override_layer_freq(ch, target_hz);
            }
        }
        _ => {}
    }
}

fn estimate_voice_dur(doc: &SoundDoc, voice: &str) -> Option<f64> {
    let layers = doc.defs.get(voice)?;
    let mut best = 0.0f64;
    for l in layers {
        if let Some(d) = l.common.duration {
            best = best.max(l.common.start_sec + d);
        }
    }
    if best > 0.0 { Some(best) } else { None }
}

fn max_fx_tail(fx: &[FxSpec], bpm: f64) -> f64 {
    fx.iter().map(|f| f.tail_seconds(bpm)).fold(0.0, f64::max)
}

#[derive(Debug, Clone, Copy)]
struct InlineFilter {
    mode: FilterMode,
    base_cutoff: f64,
    q: f64,
    gain_db: f64,
    mix: f64,
}

fn synth_leaf(buf: &mut AudioBuffer, n: usize, dur: f64, layer: &Layer, ctx: RenderCtx) -> bool {
    let c = &layer.common;
    let n = n.min(buf.len());
    let sr = ctx.sr;

    // If cutoff_env or cutoff LFO is active, modulate the first pipe filter inline (or default LP 1400 Hz)
    let has_cmod = c.cutoff_env.is_some()
        || c.lfos.iter().any(|l| l.target == LfoTarget::Cutoff);
    let (inline_flt, consumed_first) = if has_cmod {
        if let Some(FxSpec::Filter { mode, cutoff, q, gain_db, mix }) = c.fx.first() {
            (
                Some(InlineFilter {
                    mode: *mode,
                    base_cutoff: *cutoff,
                    q: *q,
                    gain_db: *gain_db,
                    mix: *mix,
                }),
                true,
            )
        } else {
            (
                Some(InlineFilter {
                    mode: FilterMode::LowPass,
                    base_cutoff: 1400.0,
                    q: 1.1,
                    gain_db: 0.0,
                    mix: 1.0,
                }),
                false,
            )
        }
    } else {
        (None, false)
    };
    let mut c_flt_l = TptSvf::new();
    let mut c_flt_r = TptSvf::new();

    // Helper: evaluate all LFOs at time `t`
    let eval_lfos = |t: f64| -> (f64, f64, f64, f64, f64) {
        let mut g_mod = 1.0;
        let mut p_semi = 0.0;
        let mut cf_off = 0.0;
        let mut pan_off = 0.0;
        let mut pwm_off = 0.0;
        for (idx, lfo) in c.lfos.iter().enumerate() {
            let v = lfo.eval(t, ctx.loop_dur, ctx.seed.wrapping_add(idx as u64 * 97));
            match lfo.target {
                LfoTarget::Gain => g_mod *= (1.0 + v).max(0.0),
                LfoTarget::Pitch => p_semi += v,
                LfoTarget::Cutoff => cf_off += v,
                LfoTarget::Pan => pan_off += v,
                LfoTarget::Pwm => pwm_off += v,
            }
        }
        (g_mod, p_semi, cf_off, pan_off, pwm_off)
    };

    match &layer.kind {
        LayerKind::Osc {
            wave,
            freq,
            pwm,
            cycle_lock,
        } => {
            let mut phase = 0.0;
            let base_f = if *cycle_lock {
                if let Some(ld) = ctx.loop_dur {
                    snap_cycle_locked_hz(*freq * ctx.pitch_ratio, ld)
                } else {
                    *freq * ctx.pitch_ratio
                }
            } else {
                *freq * ctx.pitch_ratio
            };
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, ps, cfo, pano, pwmo) = eval_lfos(t);
                let f = c
                    .pitch_env
                    .as_ref()
                    .map(|pe| pe.eval(t) * ctx.pitch_ratio)
                    .unwrap_or(base_f)
                    * semitones_to_ratio(ps);
                let dt = (f / sr).clamp(0.0, 0.49);
                let s = eval_wave(*wave, phase, dt, pwm + pwmo);
                phase = (phase + dt).rem_euclid(1.0);
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm;
                write_sample(buf, i, s * env, s * env, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        LayerKind::Supersaw {
            wave,
            freq,
            voices,
            detune_cents,
            spread,
        } => {
            let nv = (*voices).max(2);
            let mut rng = Rng::new(ctx.seed ^ 0x55AA);
            let mut phases: Vec<f64> = (0..nv).map(|_| rng.f64()).collect();
            let dets: Vec<f64> = (0..nv)
                .map(|k| {
                    let frac = (k as f64 / (nv - 1) as f64) * 2.0 - 1.0;
                    semitones_to_ratio(frac * detune_cents / 100.0)
                })
                .collect();
            let pans: Vec<(f64, f64)> = (0..nv)
                .map(|k| {
                    let frac = ((k as f64 / (nv - 1) as f64) * 2.0 - 1.0) * spread.clamp(0.0, 1.0);
                    let p = (frac + 1.0) * 0.5;
                    ((p * FRAC_PI_2).cos(), (p * FRAC_PI_2).sin())
                })
                .collect();
            let norm = 1.0 / (nv as f64).sqrt();
            let base_f = *freq * ctx.pitch_ratio;
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, ps, cfo, pano, _) = eval_lfos(t);
                let f = c
                    .pitch_env
                    .as_ref()
                    .map(|pe| pe.eval(t) * ctx.pitch_ratio)
                    .unwrap_or(base_f)
                    * semitones_to_ratio(ps);
                let mut sl = 0.0;
                let mut sr_s = 0.0;
                for k in 0..nv {
                    let dt = (f * dets[k] / sr).clamp(0.0, 0.49);
                    let v = eval_wave(*wave, phases[k], dt, 0.5) * norm;
                    phases[k] = (phases[k] + dt).rem_euclid(1.0);
                    sl += v * pans[k].0;
                    sr_s += v * pans[k].1;
                }
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm;
                write_sample(buf, i, sl * env, sr_s * env, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        LayerKind::Fm {
            freq,
            mod_ratio,
            mod_index,
            mod_decay,
        } => {
            let mut c_ph = 0.0;
            let mut m_ph = 0.0;
            let base_f = *freq * ctx.pitch_ratio;
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, ps, cfo, pano, _) = eval_lfos(t);
                let f = c
                    .pitch_env
                    .as_ref()
                    .map(|pe| pe.eval(t) * ctx.pitch_ratio)
                    .unwrap_or(base_f)
                    * semitones_to_ratio(ps);
                let mi = match mod_decay {
                    Some(md) if *md > 1e-5 => mod_index * (-t / md).exp(),
                    _ => *mod_index,
                };
                let s = (TAU * c_ph + mi * (TAU * m_ph).sin()).sin();
                c_ph = (c_ph + f / sr).rem_euclid(1.0);
                m_ph = (m_ph + f * mod_ratio / sr).rem_euclid(1.0);
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm;
                write_sample(buf, i, s * env, s * env, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        LayerKind::Modal {
            freq,
            partials,
            absolute,
        } => {
            let base_f = *freq * ctx.pitch_ratio;
            let mut phases = vec![0.0; partials.len()];
            let norm = 1.0 / partials.iter().map(|p| p.1.abs()).sum::<f64>().max(1.0);
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, ps, cfo, pano, _) = eval_lfos(t);
                let p_mul = semitones_to_ratio(ps);
                let mut sum = 0.0;
                for (k, &(r, amp, dec)) in partials.iter().enumerate() {
                    let f = if *absolute { r * ctx.pitch_ratio } else { base_f * r } * p_mul;
                    let d = if dec > 1e-5 { (-t * 4.6 / dec).exp() } else { 1.0 };
                    sum += (TAU * phases[k]).sin() * amp * d;
                    phases[k] = (phases[k] + f / sr).rem_euclid(1.0);
                }
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm * norm;
                let s = sum * env;
                write_sample(buf, i, s, s, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        LayerKind::Pluck {
            freq,
            damping,
            brightness,
        } => {
            let raw = synth_pluck(n, sr, *freq * ctx.pitch_ratio, *damping, *brightness, ctx.seed);
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, _, cfo, pano, _) = eval_lfos(t);
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm;
                let s = raw[i] * env;
                write_sample(buf, i, s, s, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        LayerKind::Noise {
            color,
            density,
            stereo,
        } => {
            let mut ng_l = NoiseGen::new(*color, ctx.seed, *density, sr);
            let mut ng_r = NoiseGen::new(*color, ctx.seed ^ 0xDEAD_BEEF, *density, sr);
            for i in 0..n {
                let t = i as f64 / sr;
                let (gm, _, cfo, pano, _) = eval_lfos(t);
                let sl = ng_l.next();
                let sr_s = if *stereo { ng_r.next() } else { sl };
                let env = c.env.as_ref().map(|e| e.eval(t, dur)).unwrap_or(1.0) * gm;
                write_sample(buf, i, sl * env, sr_s * env, pano, c, inline_flt, &mut c_flt_l, &mut c_flt_r, t, cfo, ctx);
            }
        }
        _ => {}
    }
    consumed_first
}

#[allow(clippy::too_many_arguments)]
#[inline]
fn write_sample(
    buf: &mut AudioBuffer,
    i: usize,
    mut sl: f64,
    mut sr_s: f64,
    pan_off: f64,
    c: &crate::sfx::parser::CommonLayer,
    inline_flt: Option<InlineFilter>,
    flt_l: &mut TptSvf,
    flt_r: &mut TptSvf,
    t: f64,
    cf_off: f64,
    ctx: RenderCtx,
) {
    if let Some(iflt) = inline_flt {
        let base_cf = c
            .cutoff_env
            .as_ref()
            .map(|ce| ce.eval(t))
            .unwrap_or(iflt.base_cutoff);
        let fc = (base_cf + cf_off).clamp(40.0, ctx.sr * 0.45) * ctx.cutoff_jitter;
        let wl = flt_l.process(sl, iflt.mode, fc, iflt.q, iflt.gain_db, ctx.sr);
        let wr = flt_r.process(sr_s, iflt.mode, fc, iflt.q, iflt.gain_db, ctx.sr);
        let m = iflt.mix.clamp(0.0, 1.0);
        sl = sl * (1.0 - m) + wl * m;
        sr_s = sr_s * (1.0 - m) + wr * m;
    }
    if pan_off.abs() > 1e-4 {
        let p = (pan_off.clamp(-1.0, 1.0) + 1.0) * 0.5;
        sl *= (p * FRAC_PI_2).cos() * std::f64::consts::SQRT_2;
        sr_s *= (p * FRAC_PI_2).sin() * std::f64::consts::SQRT_2;
    }
    buf.left[i] = sl;
    buf.right[i] = sr_s;
}
