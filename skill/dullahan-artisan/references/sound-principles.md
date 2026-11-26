# Universal Procedural Sound Design & Mixing Principles

Genre-neutral acoustic synthesis and mixing guidelines for authoring clean one-shots, variations, seamless loops, ambiences, and multi-track music in `.sfx` / `.kit`. Adapt instrumentation, mood, and density freely to the user's request.

---

## 1. Acoustic Layering Formulas
- **Impacts, Steps & Tactile Foley (compile with `--var 4`+)**:
  1. *Weight / Transient body*: short `sin` or `tri` with fast downward `penv` (e.g., `110>38 Hz` over `40–60 ms`). Keep gain restrained (`0.4–0.55`) on light foley so it does not boom like a bass drum.
  2. *Surface texture / Crunch*: `pink` noise through a sweeping `bp` or `lp` filter (`cenv`) capturing the primary strike plus an optional delayed secondary scrape/release.
  3. *Micro-detail*: `sct` firing `8–16` short (`15–30 ms`) high-bandpassed grains or `mod` metallic/wooden partials with randomized pitch (`pr=`), gain (`gr=`), and stereo spread (`pan=`).
- **Continuous Mechanical / Ambient Loops (`loop=1`)**:
  - *Periodic machinery / drones*: lock periodic oscillators and modulating LFOs to the loop duration with `cl=1`, couple filtered noise to the same modulation rate for physical air movement, and use `curve=lin` crossfading.
  - *Organic / stochastic beds*: layer low (`brown` + `lp`), mid counter-panned (`pink` + dual opposite-phase `bp` LFO sweeps on both gain and cutoff), and high air (`pink` + high `bp`), scatter occasional wide-panned foreground accents through stereo `rev`, and use `curve=eqp` (`0.4–0.8 s`) so uncorrelated textures never dip at the seam.
- **Tonal Alerts, Organic Calls & Melodic Voices**:
  - For natural whistles/calls, keep FM modulation index gentle (`mi=0.15–0.40`) with smooth multi-segment `penv` arcs and subtle pitch vibrato (`lfo=p:...`); save higher `mi` (`1.5–4.0`) for bells, metallic strikes, and bright synthetic plucks.
  - For brassy/acoustic multi-tone alerts, stack lightly detuned voices with a brief upward onset pitch scoop (`penv`) and subtle bandpassed air noise.
- **Multi-Track Musical Loops (`bpm=` + `bars=`)**:
  - Verify every `pat` and `seq` matches the exact `bars * bpb` beat length.
  - Carve frequency and stereo space: keep kick and bass centered (`pan=0`), high-pass master at `~30 Hz`, spread chords/pads wide (`ssaw` + `cho` + `rev`), pan percussion/arpeggios off-center (`±0.2–0.35`) with tempo-synced `dly b=... pp=1`, and keep the lead voice clear above the accompaniment.

## 2. Performance & Clean Signal Hygiene
- **Attach heavy FX (`rev`, `dly`) to enclosing tracks (`pat`, `seq`, `sct`, `bus`)** rather than inside per-hit `def` voices triggered dozens of times.
- **Prevent harsh digital aliasing & mud**: roll off unused extreme highs (`lp 3.5–6.5 kHz` on bright saw/FM layers unless crisp air is wanted) and always high-pass the master bus (`bus | flt hp 30`) so asymmetric `drv` and `brown` noise never leave sub-25 Hz mud or DC bias.

## 3. Diagnostic QA Checklist (Inspect Telemetry + `--plot` PNG)
1. **Loop seam continuity**: confirm `seam <= 0.03` (ideal) / `< 0.08` (max) and inspect the ±25 ms Loop Seam Zoom panel.
   - Midpoint swell on periodic loops → use `curve=lin`; midpoint dip on noise beds → use `curve=eqp`.
2. **Dynamics & cleanliness**: `clip=0`, `dc < 0.02`, crisp non-truncated transient in Attack Zoom.
3. **Spectral balance**: verify the 6-band bar chart and spectrogram match the intended acoustic role (e.g., no unintended sub rumble on high foley, no piercing upper-band sideband smear on soft whistles).

---

## 4. Character Voices & Fake Speech Synthesis (Dialogue Blips & Gibberish)

Author expressive indie-game dialogue voices (think *Animal Crossing*, *Banjo-Kazooie*, *Undertale*, *Celeste*) directly in a shared `.kit` + `.sfx` — no speech engine required:

### Synthesizing a Character Voice Kit (`.kit`)
- **Glottal source + parallel formants**: Drive a rich harmonic oscillator (`osc saw` or `osc pls pwm=.32` with gentle pitch contour `penv=` and subtle vibrato `lfo=p:5.5:10`) into **2 parallel resonant bandpass filters** (`flt bp` at `F1` and `F2` with `q=3.5..6`), plus a tiny `noi pink` burst (`8–15 ms`, high `bp`) for consonant/breath articulation.
- **Core Vowel Formants (`F1 / F2` Hz)** — shift base pitch & scale formants `×1.25–1.55` for small/cute characters, `×0.70–0.85` for deep/monstrous voices, or swap `osc saw` → `fm` / `crush` for robots and aliens:
  - `a` ("ah"): `700 / 1200 Hz` · `e` ("eh"): `550 / 1850 Hz` · `i` ("ee"): `300 / 2300 Hz` · `o` ("oh"): `480 / 900 Hz` · `u` ("oo"): `320 / 750 Hz`.
- **Two authoring workflows**:
  1. **Phoneme Bank (`.kit` → short `70–120 ms` `.wav` clips)**: Export `voice_a`, `voice_e`, `voice_i`, `voice_o`, `voice_u` (plus optional `voice_cons` soft click/breath) using `--var 3` so repeated letters never sound identical.
  2. **Pre-baked Emote Phrases (`.sfx` using `seq` / `ref`)**: Sequence 3–6 quick vowel defs with prosody arcs (`penv` rising for *question*, falling for *agree*, spiky for *surprise/angry*) for non-verbal barks and reactions.

### Mapping Text to Voice Blips (Runtime Principle)
- **Trigger on syllables / vowels (every 2–3 typed chars)** rather than every single letter to avoid machine-gun clutter:
  - Map `a / e / i / o / u` (and accented equivalents) → the matching formant clip; fall back to `e` or `o` for consonant-only chunks, and layer the soft consonant click on plosives (`p, t, k, b, d, g`).
  - **Cadence**: pause on spaces/commas (`60–140 ms`), hold slightly longer on stressed/capitalized words.
  - **Prosody from punctuation**: ramp pitch **up** `+2..+4` semitones over the final 2–3 syllables before `?`, boost pitch + slight gain on `!`, and slow cadence + drop pitch on `...`.
  - **Humanization**: apply deterministic word-hashed pitch variance (`±0.8` semitones) and a short `10–15 ms` voice-steal fade so fast dialogue flows cleanly.
