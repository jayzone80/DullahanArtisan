use std::collections::HashMap;
use std::path::Path;

use crate::common::expr::ExprEnv;
use crate::sfx::dsp::{
    EnvSpec, FilterMode, LfoSpec, LfoTarget, LfoWave, NoiseColor, SweepCurve, SweepSpec, WaveType,
};
use crate::sfx::fx::FxSpec;
use crate::sfx::theory::note_to_hz;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CrossfadeCurve {
    Linear,
    EqualPower,
}

#[derive(Debug, Clone)]
pub struct SoundDoc {
    pub name: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub duration: f64,
    pub bpm: f64,
    pub loop_mode: bool,
    pub crossfade: f64,
    pub curve: CrossfadeCurve,
    pub normalize: bool,
    pub norm_db: f64,
    pub seed: u64,
    pub defs: HashMap<String, Vec<Layer>>,
    pub layers: Vec<Layer>,
    pub master_fx: Vec<FxSpec>,
}

#[derive(Debug, Clone)]
pub struct CommonLayer {
    pub gain: f64,
    pub pan: f64,
    pub start_sec: f64,
    pub duration: Option<f64>,
    pub muted: bool,
    pub env: Option<EnvSpec>,
    pub pitch_env: Option<SweepSpec>,
    pub cutoff_env: Option<SweepSpec>,
    pub lfos: Vec<LfoSpec>,
    pub fx: Vec<FxSpec>,
}

impl Default for CommonLayer {
    fn default() -> Self {
        Self {
            gain: 1.0,
            pan: 0.0,
            start_sec: 0.0,
            duration: None,
            muted: false,
            env: None,
            pitch_env: None,
            cutoff_env: None,
            lfos: Vec::new(),
            fx: Vec::new(),
        }
    }
}

#[derive(Debug, Clone)]
pub enum LayerKind {
    Osc {
        wave: WaveType,
        freq: f64,
        pwm: f64,
        phase: f64,
        cycle_lock: bool,
    },
    Supersaw {
        wave: WaveType,
        freq: f64,
        voices: usize,
        detune_cents: f64,
        spread: f64,
    },
    Fm {
        freq: f64,
        mod_ratio: f64,
        mod_index: f64,
        mod_decay: Option<f64>,
        phase: f64,
    },
    Modal {
        freq: f64,
        partials: Vec<(f64, f64, f64)>, // (ratio_or_hz, amp, decay_sec)
        absolute: bool,
    },
    Pluck {
        freq: f64,
        damping: f64,
        brightness: f64,
    },
    Noise {
        color: NoiseColor,
        density: f64,
        stereo: bool,
    },
    Ref {
        voice: String,
        freq_override: Option<f64>,
        semitones: f64,
    },
    Group {
        children: Vec<Layer>,
    },
    Pattern {
        voice: String,
        steps: String,
        step_beats: f64,
        swing: f64,
        repeat: usize,
    },
    Sequence {
        voice: String,
        events: Vec<SeqEvent>,
        gate: f64,
        repeat: usize,
        transpose: f64,
    },
    Scatter {
        voice: String,
        count: usize,
        win_start: f64,
        win_end: f64,
        min_gap: f64,
        pitch_lo: f64,
        pitch_hi: f64,
        gain_lo: f64,
        gain_hi: f64,
        pan_lo: f64,
        pan_hi: f64,
        seed: u64,
    },
}

#[derive(Debug, Clone)]
pub struct SeqEvent {
    pub notes_hz: Vec<f64>, // empty => rest
    pub beats: f64,
    pub velocity: f64,
}

#[derive(Debug, Clone)]
pub struct Layer {
    pub common: CommonLayer,
    pub kind: LayerKind,
}

pub fn load_sound_file(path: &Path, kit_override: Option<&Path>) -> Result<SoundDoc, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read sound '{}': {e}", path.display()))?;
    let base_dir = path.parent().unwrap_or_else(|| Path::new("."));
    let default_name = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("sound");
    parse_sound_doc(&text, default_name, base_dir, kit_override)
}

pub fn parse_sound_doc(
    text: &str,
    default_name: &str,
    base_dir: &Path,
    kit_override: Option<&Path>,
) -> Result<SoundDoc, String> {
    let mut env = ExprEnv::new();
    let mut doc = SoundDoc {
        name: default_name.to_string(),
        sample_rate: 44100,
        channels: 2,
        duration: 1.0,
        bpm: 120.0,
        loop_mode: false,
        crossfade: 0.02,
        curve: CrossfadeCurve::Linear,
        normalize: true,
        norm_db: -1.0,
        seed: 1,
        defs: HashMap::new(),
        layers: Vec::new(),
        master_fx: Vec::new(),
    };

    if let Some(kp) = kit_override {
        load_kit_into(kp, &mut doc.defs, doc.bpm)?;
    }

    let mut raw_lines: Vec<(usize, String)> = Vec::new();
    let mut saw_dur = false;
    let mut bars_opt: Option<f64> = None;
    let mut bpb = 4.0f64;

    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let clean = strip_comment(line).trim();
        if clean.is_empty() {
            continue;
        }
        let toks = tokenize(clean);
        if toks.is_empty() {
            continue;
        }
        let op = toks[0].to_ascii_lowercase();
        if op == "sfx" {
            for tok in &toks[1..] {
                if let Some((k, v)) = tok.split_once('=') {
                    match k {
                        "dur" => {
                            doc.duration = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
                            saw_dur = true;
                        }
                        "bpm" => doc.bpm = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?,
                        "bars" => bars_opt = Some(env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?),
                        "bpb" => bpb = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?,
                        "sr" => doc.sample_rate = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?.round() as u32,
                        "ch" => doc.channels = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?.round().clamp(1.0, 2.0) as u16,
                        "loop" => doc.loop_mode = v == "1" || v.eq_ignore_ascii_case("true"),
                        "xfade" => doc.crossfade = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?,
                        "curve" => {
                            doc.curve = match v.to_ascii_lowercase().as_str() {
                                "eqp" | "equalpower" | "eq" => CrossfadeCurve::EqualPower,
                                _ => CrossfadeCurve::Linear,
                            };
                        }
                        "norm" => {
                            if v == "0" || v.eq_ignore_ascii_case("false") {
                                doc.normalize = false;
                            } else {
                                doc.normalize = true;
                                doc.norm_db = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
                            }
                        }
                        "seed" => doc.seed = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?.round().max(0.0) as u64,
                        _ => {}
                    }
                } else {
                    doc.name = tok.clone();
                }
            }
            if !saw_dur {
                if let Some(bars) = bars_opt {
                    doc.duration = bars * bpb * 60.0 / doc.bpm.max(20.0);
                }
            }
            continue;
        }
        if op == "use" {
            if let Some(rel) = toks.get(1) {
                if kit_override.is_none() {
                    let kp = base_dir.join(rel);
                    load_kit_into(&kp, &mut doc.defs, doc.bpm)?;
                }
            }
            continue;
        }
        if op.starts_with('$') {
            parse_var_assign(&toks, &mut env, lineno)?;
            continue;
        }
        raw_lines.push((lineno, clean.to_string()));
    }

    // Parse blocks (def, grp, bus, layers)
    let mut i = 0usize;
    while i < raw_lines.len() {
        let (lineno, ref line) = raw_lines[i];
        let (head, fx_segs) = split_pipes(line);
        let toks = tokenize(head);
        if toks.is_empty() {
            i += 1;
            continue;
        }
        let op = toks[0].to_ascii_lowercase();
        if op == "def" {
            let vname = toks
                .get(1)
                .ok_or_else(|| format!("line {lineno}: def requires voice name"))?
                .clone();
            let (body, next_i) = collect_sfx_block(&raw_lines, i + 1, lineno, "def")?;
            let layers = parse_layer_block(&body, &env, doc.bpm, doc.duration)?;
            doc.defs.insert(vname, layers);
            i = next_i;
        } else if op == "bus" {
            for seg in fx_segs {
                doc.master_fx.push(parse_fx_segment(seg, &env, lineno)?);
            }
            i += 1;
        } else {
            let (layer, next_i) = parse_single_layer(&raw_lines, i, &env, doc.bpm, doc.duration)?;
            doc.layers.push(layer);
            i = next_i;
        }
    }

    Ok(doc)
}

fn load_kit_into(path: &Path, defs: &mut HashMap<String, Vec<Layer>>, bpm: f64) -> Result<(), String> {
    let text = std::fs::read_to_string(path)
        .map_err(|e| format!("failed to read kit '{}': {e}", path.display()))?;
    let mut env = ExprEnv::new();
    let mut raw_lines = Vec::new();
    for (idx, line) in text.lines().enumerate() {
        let lineno = idx + 1;
        let clean = strip_comment(line).trim();
        if clean.is_empty() {
            continue;
        }
        let toks = tokenize(clean);
        if toks.is_empty() {
            continue;
        }
        if toks[0].starts_with('$') {
            parse_var_assign(&toks, &mut env, lineno)?;
            continue;
        }
        raw_lines.push((lineno, clean.to_string()));
    }
    let mut i = 0usize;
    while i < raw_lines.len() {
        let (lineno, ref line) = raw_lines[i];
        let toks = tokenize(line);
        if toks[0].eq_ignore_ascii_case("def") {
            let vname = toks
                .get(1)
                .ok_or_else(|| format!("kit line {lineno}: def requires name"))?
                .clone();
            let (body, next_i) = collect_sfx_block(&raw_lines, i + 1, lineno, "def")?;
            let layers = parse_layer_block(&body, &env, bpm, 1.0)?;
            defs.insert(vname, layers);
            i = next_i;
        } else {
            i += 1;
        }
    }
    Ok(())
}

fn parse_layer_block(
    lines: &[(usize, String)],
    env: &ExprEnv,
    bpm: f64,
    doc_dur: f64,
) -> Result<Vec<Layer>, String> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < lines.len() {
        let (layer, next_i) = parse_single_layer(lines, i, env, bpm, doc_dur)?;
        out.push(layer);
        i = next_i;
    }
    Ok(out)
}

fn parse_single_layer(
    lines: &[(usize, String)],
    idx: usize,
    env: &ExprEnv,
    bpm: f64,
    doc_dur: f64,
) -> Result<(Layer, usize), String> {
    let (lineno, ref line) = lines[idx];
    let (head, fx_segs) = split_pipes(line);
    let toks = tokenize(head);
    let op = toks[0].to_ascii_lowercase();

    let mut pos_args = Vec::new();
    let mut flags: HashMap<String, String> = HashMap::new();
    for tok in &toks[1..] {
        if let Some((k, v)) = tok.split_once('=') {
            if !k.is_empty() && k.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
                flags.insert(k.to_ascii_lowercase(), v.to_string());
                continue;
            }
        }
        pos_args.push(tok.clone());
    }

    let mut common = CommonLayer::default();
    for seg in fx_segs {
        common.fx.push(parse_fx_segment(seg, env, lineno)?);
    }
    if let Some(v) = flags.get("g") {
        common.gain = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
    }
    if let Some(v) = flags.get("pan") {
        if !v.contains("..") {
            common.pan = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
        }
    }
    if let Some(v) = flags.get("t") {
        common.start_sec = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
    }
    if let Some(v) = flags.get("tb") {
        let b = env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?;
        common.start_sec = b * 60.0 / bpm.max(20.0);
    }
    if let Some(v) = flags.get("dur") {
        common.duration = Some(env.eval(v).map_err(|e| format!("line {lineno}: {e}"))?);
    }
    if let Some(v) = flags.get("mute") {
        common.muted = v == "1" || v.eq_ignore_ascii_case("true");
    }
    if let Some(v) = flags.get("env") {
        common.env = Some(parse_env(v, env, lineno)?);
    }
    if let Some(v) = flags.get("penv") {
        common.pitch_env = Some(parse_sweep(v, env, true, lineno)?);
    }
    if let Some(v) = flags.get("cenv") {
        common.cutoff_env = Some(parse_sweep(v, env, true, lineno)?);
    }
    let cl_global = flags
        .get("cl")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if let Some(v) = flags.get("lfo") {
        common.lfos = parse_lfos(v, env, cl_global, lineno)?;
    }
    // Shorthand inline filter flag `flt=lp:1200:1.4`
    if let Some(v) = flags.get("flt") {
        let parts: Vec<&str> = v.split(':').collect();
        let mode = parse_filter_mode(parts.first().copied().unwrap_or("lp"), lineno)?;
        let cutoff = env.eval(parts.get(1).copied().unwrap_or("1000")).map_err(|e| format!("line {lineno}: {e}"))?;
        let q = env.eval(parts.get(2).copied().unwrap_or("0.707")).map_err(|e| format!("line {lineno}: {e}"))?;
        common.fx.insert(
            0,
            FxSpec::Filter {
                mode,
                cutoff,
                q,
                gain_db: 0.0,
                mix: 1.0,
            },
        );
    }

    if op == "grp" {
        let (body, next_i) = collect_sfx_block(lines, idx + 1, lineno, "grp")?;
        let children = parse_layer_block(&body, env, bpm, doc_dur)?;
        return Ok((Layer { common, kind: LayerKind::Group { children } }, next_i));
    }

    let kind = match op.as_str() {
        "osc" => {
            if pos_args.len() < 2 {
                return Err(format!("line {lineno}: osc requires <wave> <freq|note>"));
            }
            let wave = parse_wave(&pos_args[0], lineno)?;
            let freq = parse_freq_expr(&pos_args[1], env, lineno)?;
            let pwm = opt_eval(&flags, "pwm", 0.5, env, lineno)?;
            let phase = opt_eval(&flags, "ph", 0.0, env, lineno)?.rem_euclid(1.0);
            LayerKind::Osc {
                wave,
                freq,
                pwm,
                phase,
                cycle_lock: cl_global,
            }
        }
        "ssaw" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: ssaw requires <freq|note>"));
            }
            let freq = parse_freq_expr(&pos_args[0], env, lineno)?;
            let wave = match flags.get("w") {
                Some(w) => parse_wave(w, lineno)?,
                None => WaveType::Saw,
            };
            let voices = opt_eval(&flags, "v", 5.0, env, lineno)?.round().clamp(2.0, 9.0) as usize;
            let detune_cents = opt_eval(&flags, "det", 12.0, env, lineno)?;
            let spread = opt_eval(&flags, "spr", 0.8, env, lineno)?;
            LayerKind::Supersaw {
                wave,
                freq,
                voices,
                detune_cents,
                spread,
            }
        }
        "fm" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: fm requires <freq|note>"));
            }
            let freq = parse_freq_expr(&pos_args[0], env, lineno)?;
            let mod_ratio = opt_eval(&flags, "mr", 2.0, env, lineno)?;
            let mod_index = opt_eval(&flags, "mi", 2.5, env, lineno)?;
            let mod_decay = match flags.get("md") {
                Some(s) => Some(env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?),
                None => None,
            };
            let phase = opt_eval(&flags, "ph", 0.0, env, lineno)?.rem_euclid(1.0);
            LayerKind::Fm {
                freq,
                mod_ratio,
                mod_index,
                mod_decay,
                phase,
            }
        }
        "mod" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: mod requires <freq|note> p=r:a[:dec],..."));
            }
            let freq = parse_freq_expr(&pos_args[0], env, lineno)?;
            let p_str = flags
                .get("p")
                .ok_or_else(|| format!("line {lineno}: mod requires p=ratio:amp[:decay],..."))?;
            let absolute = flags
                .get("abs")
                .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
                .unwrap_or(false);
            let mut partials = Vec::new();
            for item in p_str.split(',') {
                let parts: Vec<&str> = item.split(':').collect();
                if parts.len() < 2 {
                    return Err(format!("line {lineno}: bad modal partial '{item}'"));
                }
                let r = parse_freq_expr(parts[0], env, lineno)?;
                let a = env.eval(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
                let d = if let Some(ds) = parts.get(2) {
                    env.eval(ds).map_err(|e| format!("line {lineno}: {e}"))?
                } else {
                    0.5
                };
                partials.push((r, a, d));
            }
            LayerKind::Modal {
                freq,
                partials,
                absolute,
            }
        }
        "plk" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: plk requires <freq|note>"));
            }
            let freq = parse_freq_expr(&pos_args[0], env, lineno)?;
            let damping = opt_eval(&flags, "damp", 0.992, env, lineno)?;
            let brightness = opt_eval(&flags, "br", 0.65, env, lineno)?;
            LayerKind::Pluck {
                freq,
                damping,
                brightness,
            }
        }
        "noi" => {
            let color = match pos_args.first().map(|s| s.to_ascii_lowercase()).as_deref() {
                Some("pink") => NoiseColor::Pink,
                Some("brown") => NoiseColor::Brown,
                Some("crackle") => NoiseColor::Crackle,
                Some("white") | None => NoiseColor::White,
                Some(other) => return Err(format!("line {lineno}: unknown noise color '{other}'")),
            };
            let density = opt_eval(&flags, "dens", 40.0, env, lineno)?;
            let stereo = flags
                .get("st")
                .map(|v| v != "0" && !v.eq_ignore_ascii_case("false"))
                .unwrap_or(true);
            LayerKind::Noise {
                color,
                density,
                stereo,
            }
        }
        "ref" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: ref requires <voice>"));
            }
            let voice = pos_args[0].clone();
            let freq_override = if let Some(n) = flags.get("note").or_else(|| flags.get("freq")) {
                Some(parse_freq_expr(n, env, lineno)?)
            } else {
                None
            };
            let semitones = opt_eval(&flags, "semi", 0.0, env, lineno)?;
            LayerKind::Ref {
                voice,
                freq_override,
                semitones,
            }
        }
        "pat" => {
            if pos_args.len() < 2 {
                return Err(format!("line {lineno}: pat requires <voice> \"<steps>\""));
            }
            let voice = pos_args[0].clone();
            let steps = pos_args[1].clone();
            let step_beats = opt_eval(&flags, "step", 0.25, env, lineno)?;
            let swing = opt_eval(&flags, "swing", 0.0, env, lineno)?;
            let repeat = opt_eval(&flags, "rep", 1.0, env, lineno)?.round().max(1.0) as usize;
            LayerKind::Pattern {
                voice,
                steps,
                step_beats,
                swing,
                repeat,
            }
        }
        "seq" => {
            if pos_args.len() < 2 {
                return Err(format!("line {lineno}: seq requires <voice> \"<events>\""));
            }
            let voice = pos_args[0].clone();
            let events = parse_seq_events(&pos_args[1], env, lineno)?;
            let gate = opt_eval(&flags, "gate", 0.9, env, lineno)?;
            let repeat = opt_eval(&flags, "rep", 1.0, env, lineno)?.round().max(1.0) as usize;
            let transpose = opt_eval(&flags, "tr", 0.0, env, lineno)?;
            LayerKind::Sequence {
                voice,
                events,
                gate,
                repeat,
                transpose,
            }
        }
        "sct" => {
            if pos_args.is_empty() {
                return Err(format!("line {lineno}: sct requires <voice>"));
            }
            let voice = pos_args[0].clone();
            let count = opt_eval(&flags, "n", 8.0, env, lineno)?.round().max(1.0) as usize;
            let (win_start, win_end) = match flags.get("win") {
                Some(s) => parse_range_f64(s, env, lineno)?,
                None => (0.0, doc_dur),
            };
            let min_gap = opt_eval(&flags, "gap", 0.01, env, lineno)?;
            let (pitch_lo, pitch_hi) = match flags.get("pr") {
                Some(s) => parse_range_f64(s, env, lineno)?,
                None => (0.0, 0.0),
            };
            let (gain_lo, gain_hi) = match flags.get("gr") {
                Some(s) => parse_range_f64(s, env, lineno)?,
                None => (0.7, 1.0),
            };
            let (pan_lo, pan_hi) = match flags.get("pan") {
                Some(s) if s.contains("..") => {
                    common.pan = 0.0;
                    parse_range_f64(s, env, lineno)?
                }
                _ => (-0.4, 0.4),
            };
            let seed = opt_eval(&flags, "seed", 1.0, env, lineno)?.round().max(0.0) as u64;
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
            }
        }
        _ => return Err(format!("line {lineno}: unknown sfx opcode '{op}'")),
    };

    Ok((Layer { common, kind }, idx + 1))
}

fn parse_fx_segment(seg: &str, env: &ExprEnv, lineno: usize) -> Result<FxSpec, String> {
    let toks = tokenize(seg);
    if toks.is_empty() {
        return Err(format!("line {lineno}: empty FX pipe segment"));
    }
    let op = toks[0].to_ascii_lowercase();
    let mut pos = Vec::new();
    let mut flags: HashMap<String, String> = HashMap::new();
    for tok in &toks[1..] {
        if let Some((k, v)) = tok.split_once('=') {
            flags.insert(k.to_ascii_lowercase(), v.to_string());
        } else {
            pos.push(tok.clone());
        }
    }
    let mix = opt_eval(&flags, "mix", 1.0, env, lineno)?;
    match op.as_str() {
        "flt" | "eq" => {
            let mode = parse_filter_mode(pos.first().map(|s| s.as_str()).unwrap_or("lp"), lineno)?;
            let cutoff = match pos.get(1) {
                Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?,
                None => opt_eval(&flags, "fc", 1000.0, env, lineno)?,
            };
            let q = match pos.get(2) {
                Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?,
                None => opt_eval(&flags, "q", 0.707, env, lineno)?,
            };
            let gain_db = opt_eval(&flags, "db", 0.0, env, lineno)?;
            Ok(FxSpec::Filter {
                mode,
                cutoff,
                q,
                gain_db,
                mix,
            })
        }
        "drv" | "sat" => {
            let amount = match pos.first() {
                Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?,
                None => 2.0,
            };
            let asym = opt_eval(&flags, "asym", 0.0, env, lineno)?;
            Ok(FxSpec::Drive { amount, asym, mix })
        }
        "crush" => {
            let bits = match pos.first() {
                Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?,
                None => 6.0,
            };
            let rate_hz = match pos.get(1) {
                Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?,
                None => opt_eval(&flags, "rate", 11025.0, env, lineno)?,
            };
            Ok(FxSpec::Bitcrush { bits, rate_hz, mix })
        }
        "cho" => {
            let rate = pos.first().map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.5);
            let depth_ms = pos.get(1).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(3.0);
            let m = pos.get(2).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(if flags.contains_key("mix") { mix } else { 0.4 });
            let delay_ms = opt_eval(&flags, "dly", 12.0, env, lineno)?;
            Ok(FxSpec::Chorus {
                rate,
                depth_ms,
                delay_ms,
                mix: m,
            })
        }
        "dly" => {
            let mut beats = None;
            let mut time_sec = 0.25;
            if let Some(b) = flags.get("b") {
                beats = Some(env.eval(b).map_err(|e| format!("line {lineno}: {e}"))?);
            } else if let Some(s) = pos.first() {
                time_sec = env.eval(s).map_err(|e| format!("line {lineno}: {e}"))?;
            }
            let feedback = opt_eval(&flags, "fb", 0.35, env, lineno)?;
            let damp_hz = opt_eval(&flags, "damp", 4000.0, env, lineno)?;
            let ping_pong = flags.get("pp").map(|v| v == "1" || v.eq_ignore_ascii_case("true")).unwrap_or(false);
            let m = if flags.contains_key("mix") { mix } else { pos.get(1).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.3) };
            Ok(FxSpec::Delay {
                time_sec,
                beats,
                feedback,
                damp_hz,
                ping_pong,
                mix: m,
            })
        }
        "rev" => {
            let room = pos.first().map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.75);
            let m = pos.get(1).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(if flags.contains_key("mix") { mix } else { 0.3 });
            let damp = opt_eval(&flags, "damp", 0.4, env, lineno)?;
            let pre_delay_ms = opt_eval(&flags, "pre", 0.0, env, lineno)?;
            Ok(FxSpec::Reverb {
                room,
                damp,
                pre_delay_ms,
                mix: m,
            })
        }
        "cmp" => {
            let threshold_db = pos.first().map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(-12.0);
            let ratio = pos.get(1).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(3.0);
            let attack = pos.get(2).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.01);
            let release = pos.get(3).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.12);
            let makeup_db = pos.get(4).map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(0.0);
            Ok(FxSpec::Compressor {
                threshold_db,
                ratio,
                attack,
                release,
                makeup_db,
            })
        }
        "wid" => {
            let amount = pos.first().map(|s| env.eval(s)).transpose().map_err(|e| format!("line {lineno}: {e}"))?.unwrap_or(1.2);
            Ok(FxSpec::Width { amount })
        }
        _ => Err(format!("line {lineno}: unknown FX '{op}'")),
    }
}

fn parse_env(s: &str, env: &ExprEnv, lineno: usize) -> Result<EnvSpec, String> {
    if s.contains(':') {
        let mut pts = Vec::new();
        for pair in s.split(',') {
            let (ts, vs) = pair
                .split_once(':')
                .ok_or_else(|| format!("line {lineno}: bad env point '{pair}'"))?;
            let t = env.eval(ts).map_err(|e| format!("line {lineno}: {e}"))?;
            let v = env.eval(vs).map_err(|e| format!("line {lineno}: {e}"))?;
            pts.push((t, v));
        }
        Ok(EnvSpec::Points(pts))
    } else {
        let vals = env.eval_list_f32(s).map_err(|e| format!("line {lineno}: {e}"))?;
        if vals.len() != 4 {
            return Err(format!("line {lineno}: env expects a,d,s,r (4 values) or t0:v0,..."));
        }
        Ok(EnvSpec::Adsr {
            attack: vals[0] as f64,
            decay: vals[1] as f64,
            sustain: vals[2] as f64,
            release: vals[3] as f64,
        })
    }
}

fn parse_sweep_curve(tok: &str) -> Option<SweepCurve> {
    match tok.to_ascii_lowercase().as_str() {
        "exp" | "exponential" => Some(SweepCurve::Exp),
        "lin" | "linear" => Some(SweepCurve::Lin),
        "log" | "logarithmic" => Some(SweepCurve::Log),
        "sm" | "smooth" | "smoothstep" => Some(SweepCurve::Smooth),
        _ => None,
    }
}

/// Parse `start>end:dur[:curve]`, multi-stage `v0>v1:t1>v2:t2[:curve]`,
/// or piecewise `t0:v0,t1:v1,...[:curve]` (accepting Hz or note names) into `(time, value)` points.
fn parse_sweep(s: &str, env: &ExprEnv, default_exp: bool, lineno: usize) -> Result<SweepSpec, String> {
    let mut curve = if default_exp {
        SweepCurve::Exp
    } else {
        SweepCurve::Lin
    };

    // Piecewise `t0:v0,t1:v1,...[:curve]` format (no `>`)
    if !s.contains('>') {
        let mut pts = Vec::new();
        let items: Vec<&str> = s.split(',').collect();
        for (idx, item) in items.iter().enumerate() {
            let parts: Vec<&str> = item.split(':').collect();
            if parts.len() < 2 {
                return Err(format!(
                    "line {lineno}: sweep expects start>end:dur or t0:v0,t1:v1,..., got '{s}'"
                ));
            }
            if parts.len() >= 3 && idx == items.len() - 1 {
                if let Some(c) = parse_sweep_curve(parts[2]) {
                    curve = c;
                } else {
                    return Err(format!(
                        "line {lineno}: unknown sweep curve '{}' (use exp|lin|log|sm)",
                        parts[2]
                    ));
                }
            }
            let t = env.eval(parts[0]).map_err(|e| format!("line {lineno}: {e}"))?;
            let val = parse_freq_expr(parts[1], env, lineno)?;
            pts.push((t, val));
        }
        if pts.is_empty() {
            return Err(format!("line {lineno}: empty sweep '{s}'"));
        }
        pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
        return Ok(SweepSpec { points: pts, curve });
    }

    let segs: Vec<&str> = s.split('>').collect();
    if segs.len() < 2 {
        return Err(format!("line {lineno}: sweep expects start>end:dur, got '{s}'"));
    }
    let v0 = parse_freq_expr(segs[0], env, lineno)?;
    let mut pts = vec![(0.0, v0)];
    let mut cur_t = 0.0;
    for &seg in &segs[1..] {
        let parts: Vec<&str> = seg.split(':').collect();
        if parts.len() < 2 {
            return Err(format!("line {lineno}: sweep target needs val:time in '{seg}'"));
        }
        let val = parse_freq_expr(parts[0], env, lineno)?;
        let dt = env.eval(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
        if let Some(cs) = parts.get(2) {
            if let Some(c) = parse_sweep_curve(cs) {
                curve = c;
            } else {
                return Err(format!(
                    "line {lineno}: unknown sweep curve '{cs}' (use exp|lin|log|sm)"
                ));
            }
        }
        cur_t += dt.max(0.0005);
        pts.push((cur_t, val));
    }
    Ok(SweepSpec { points: pts, curve })
}

/// Format: `target:rate:depth[:wave][:phase]` comma-separated (e.g. `g:6:.4,cf:6:300:sin:.25`)
fn parse_lfos(s: &str, env: &ExprEnv, default_cl: bool, lineno: usize) -> Result<Vec<LfoSpec>, String> {
    let mut out = Vec::new();
    for item in s.split(',') {
        let parts: Vec<&str> = item.split(':').collect();
        if parts.len() < 3 {
            return Err(format!("line {lineno}: lfo expects target:rate:depth[:wave][:phase]"));
        }
        let target = match parts[0].to_ascii_lowercase().as_str() {
            "g" | "gain" => LfoTarget::Gain,
            "p" | "pitch" => LfoTarget::Pitch,
            "cf" | "cutoff" => LfoTarget::Cutoff,
            "pan" => LfoTarget::Pan,
            "pwm" => LfoTarget::Pwm,
            other => return Err(format!("line {lineno}: unknown lfo target '{other}'")),
        };
        let rate = env.eval(parts[1]).map_err(|e| format!("line {lineno}: {e}"))?;
        let depth = env.eval(parts[2]).map_err(|e| format!("line {lineno}: {e}"))?;
        let wave = match parts.get(3).map(|w| w.to_ascii_lowercase()).as_deref() {
            Some("tri") => LfoWave::Triangle,
            Some("saw") => LfoWave::Saw,
            Some("sqr") => LfoWave::Square,
            Some("snh") => LfoWave::Snh,
            Some("rnd") => LfoWave::Random,
            _ => LfoWave::Sine,
        };
        let phase = match parts.get(4) {
            Some(ps) => env.eval(ps).map_err(|e| format!("line {lineno}: {e}"))?,
            None => 0.0,
        };
        out.push(LfoSpec {
            target,
            rate,
            depth,
            wave,
            phase,
            cycle_lock: default_cl,
        });
    }
    Ok(out)
}

fn parse_seq_events(s: &str, env: &ExprEnv, lineno: usize) -> Result<Vec<SeqEvent>, String> {
    let mut out = Vec::new();
    for tok in s.split_whitespace() {
        // Optional velocity suffix `@vel`
        let (main, vel) = if let Some((m, vs)) = tok.split_once('@') {
            (m, env.eval(vs).map_err(|e| format!("line {lineno}: {e}"))?)
        } else {
            (tok, 1.0)
        };
        let (note_part, beats) = if let Some((np, bs)) = main.split_once(':') {
            (np, env.eval(bs).map_err(|e| format!("line {lineno}: {e}"))?)
        } else {
            (main, 1.0)
        };
        let notes_hz = if crate::sfx::theory::is_rest(note_part) {
            Vec::new()
        } else if note_part.starts_with('[') && note_part.ends_with(']') {
            let inner = &note_part[1..note_part.len() - 1];
            let mut chord = Vec::new();
            for n in inner.split(',') {
                if !n.trim().is_empty() {
                    chord.push(note_to_hz(n).map_err(|e| format!("line {lineno}: {e}"))?);
                }
            }
            chord
        } else {
            vec![note_to_hz(note_part).map_err(|e| format!("line {lineno}: {e}"))?]
        };
        out.push(SeqEvent {
            notes_hz,
            beats: beats.max(0.01),
            velocity: vel.clamp(0.0, 2.0),
        });
    }
    Ok(out)
}

fn parse_range_f64(s: &str, env: &ExprEnv, lineno: usize) -> Result<(f64, f64), String> {
    let (a, b) = s
        .split_once("..")
        .ok_or_else(|| format!("line {lineno}: expected range a..b, got '{s}'"))?;
    let lo = env.eval(a).map_err(|e| format!("line {lineno}: {e}"))?;
    let hi = env.eval(b).map_err(|e| format!("line {lineno}: {e}"))?;
    Ok((lo, hi))
}

fn parse_freq_expr(s: &str, env: &ExprEnv, lineno: usize) -> Result<f64, String> {
    if let Ok(v) = env.eval(s) {
        return Ok(v);
    }
    note_to_hz(s).map_err(|e| format!("line {lineno}: {e}"))
}

fn parse_wave(s: &str, lineno: usize) -> Result<WaveType, String> {
    match s.to_ascii_lowercase().as_str() {
        "sin" | "sine" => Ok(WaveType::Sine),
        "tri" | "triangle" => Ok(WaveType::Triangle),
        "saw" => Ok(WaveType::Saw),
        "sqr" | "square" => Ok(WaveType::Square),
        "pls" | "pulse" => Ok(WaveType::Pulse),
        other => Err(format!("line {lineno}: unknown wave '{other}'")),
    }
}

fn parse_filter_mode(s: &str, lineno: usize) -> Result<FilterMode, String> {
    match s.to_ascii_lowercase().as_str() {
        "lp" | "lowpass" => Ok(FilterMode::LowPass),
        "hp" | "highpass" => Ok(FilterMode::HighPass),
        "bp" | "bandpass" => Ok(FilterMode::BandPass),
        "notch" | "nt" => Ok(FilterMode::Notch),
        "bell" | "peak" => Ok(FilterMode::Bell),
        other => Err(format!("line {lineno}: unknown filter mode '{other}'")),
    }
}

fn opt_eval(flags: &HashMap<String, String>, k: &str, def: f64, env: &ExprEnv, lineno: usize) -> Result<f64, String> {
    match flags.get(k) {
        Some(s) => env.eval(s).map_err(|e| format!("line {lineno}: {e}")),
        None => Ok(def),
    }
}

fn collect_sfx_block(
    lines: &[(usize, String)],
    start: usize,
    open_lineno: usize,
    kind: &str,
) -> Result<(Vec<(usize, String)>, usize), String> {
    let mut body = Vec::new();
    let mut depth = 1usize;
    let mut i = start;
    while i < lines.len() {
        let first = lines[i].1.split_whitespace().next().unwrap_or("").to_ascii_lowercase();
        if first == "def" || first == "grp" {
            depth += 1;
            body.push(lines[i].clone());
        } else if first == "end" {
            depth -= 1;
            if depth == 0 {
                return Ok((body, i + 1));
            }
            body.push(lines[i].clone());
        } else {
            body.push(lines[i].clone());
        }
        i += 1;
    }
    Err(format!("unclosed '{kind}' block opened at line {open_lineno}"))
}

fn parse_var_assign(tokens: &[String], env: &mut ExprEnv, lineno: usize) -> Result<(), String> {
    let mut chunks = Vec::new();
    let mut cur = String::new();
    for tok in tokens {
        if tok.starts_with('$') && tok.contains('=') && !cur.is_empty() {
            chunks.push(cur);
            cur = tok.clone();
        } else {
            if !cur.is_empty() {
                cur.push(' ');
            }
            cur.push_str(tok);
        }
    }
    if !cur.is_empty() {
        chunks.push(cur);
    }
    for c in chunks {
        let (lhs, rhs) = c
            .split_once('=')
            .ok_or_else(|| format!("line {lineno}: invalid variable assignment '{c}'"))?;
        let val = env.eval(rhs.trim()).map_err(|e| format!("line {lineno}: {e}"))?;
        env.set(lhs.trim(), val);
    }
    Ok(())
}

fn strip_comment(line: &str) -> &str {
    let mut in_quotes = false;
    for (i, ch) in line.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '#' && !in_quotes {
            // Preserve `#` inside note names like `F#3` when immediately preceded by a note letter A-G!
            if i > 0 {
                let prev = line.as_bytes()[i - 1].to_ascii_uppercase();
                if matches!(prev, b'A'..=b'G') {
                    continue;
                }
            }
            return &line[..i];
        }
    }
    line
}

fn split_pipes(line: &str) -> (&str, Vec<&str>) {
    let mut segs = Vec::new();
    let mut start = 0usize;
    let mut in_quotes = false;
    for (i, ch) in line.char_indices() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch == '|' && !in_quotes {
            segs.push(line[start..i].trim());
            start = i + 1;
        }
    }
    segs.push(line[start..].trim());
    (segs[0], segs[1..].to_vec())
}

fn tokenize(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut in_quotes = false;
    for ch in s.chars() {
        if ch == '"' {
            in_quotes = !in_quotes;
        } else if ch.is_whitespace() && !in_quotes {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
        } else {
            cur.push(ch);
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}
