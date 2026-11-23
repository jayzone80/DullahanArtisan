/// Music theory helpers: scientific pitch parsing, semitone ratios, and loop cycle locking.

pub fn is_rest(token: &str) -> bool {
    let t = token.trim();
    t.is_empty() || t == "-" || t == "." || t.eq_ignore_ascii_case("rest") || t.eq_ignore_ascii_case("r")
}

/// Parse scientific pitch notation (`C4`, `F#3`, `Bb2`, `D#4`) or direct Hz (`440`, `55.5`) into Hz.
pub fn note_to_hz(s: &str) -> Result<f64, String> {
    let t = s.trim();
    if let Ok(hz) = t.parse::<f64>() {
        if hz > 0.0 && hz.is_finite() {
            return Ok(hz);
        }
        return Err(format!("invalid frequency '{t}'"));
    }
    let bytes = t.as_bytes();
    if bytes.len() < 2 {
        return Err(format!("invalid note '{t}'"));
    }
    let base_semi = match bytes[0].to_ascii_uppercase() {
        b'C' => 0i32,
        b'D' => 2,
        b'E' => 4,
        b'F' => 5,
        b'G' => 7,
        b'A' => 9,
        b'B' => 11,
        _ => return Err(format!("invalid note letter in '{t}'")),
    };
    let mut pos = 1usize;
    let mut acc = 0i32;
    if pos < bytes.len() {
        match bytes[pos] {
            b'#' => {
                acc = 1;
                pos += 1;
            }
            b'b' | b'B' => {
                acc = -1;
                pos += 1;
            }
            _ => {}
        }
    }
    let oct: i32 = t[pos..]
        .parse()
        .map_err(|_| format!("invalid octave in note '{t}'"))?;
    let midi = (oct + 1) * 12 + base_semi + acc;
    Ok(440.0 * 2.0f64.powf((midi - 69) as f64 / 12.0))
}

#[inline]
pub fn semitones_to_ratio(semitones: f64) -> f64 {
    2.0f64.powf(semitones / 12.0)
}

/// Snap a periodic frequency or LFO rate to complete an exact integer number of cycles in `loop_dur`.
#[inline]
pub fn snap_cycle_locked_hz(freq_hz: f64, loop_dur: f64) -> f64 {
    if loop_dur <= 1e-6 || freq_hz <= 0.0 {
        return freq_hz;
    }
    let cycles = (freq_hz * loop_dur).round().max(1.0);
    cycles / loop_dur
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pitch_parsing() {
        assert!((note_to_hz("A4").unwrap() - 440.0).abs() < 1e-6);
        assert!((note_to_hz("C4").unwrap() - 261.625565).abs() < 1e-4);
        assert!((note_to_hz("Bb2").unwrap() - note_to_hz("A#2").unwrap()).abs() < 1e-6);
        assert!((snap_cycle_locked_hz(6.03, 4.0) - 6.0).abs() < 1e-9);
    }
}
