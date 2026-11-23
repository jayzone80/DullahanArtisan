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
