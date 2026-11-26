# CHIMERA MPC

CHIMERA's physical-model voice on Akai MPC standalone hardware, by anthonytheinventor.

[CHIMERA](https://github.com/jamesondh/chimera-vocal-synth) is a voice synthesizer by Jameson Hodge.
Every sound it makes comes from a simulated airway (lungs, vocal folds, throat, mouth and nose), not
from samples. This plugin runs CHIMERA's own engine, with settings that let it fit a 32-bit MPC.
CHIMERA MPC is an independent project, not made or endorsed by Jameson Hodge.

## What it does

**Mode** picks how notes play:

- **Sing** (the default): each note sings the next syllable of the lyrics, as CHIMERA's Sing mode
  does. **Lyrics** picks one of eight lines from `Content/lyrics.txt` in the plugin folder; edit
  that file on the drive to sing your own words. **Start Syl** sets where the lyrics start. The
  first moment after loading, while the pronunciation dictionary loads, a note sings "ah".
- **Play**: a note sings one held vowel at its pitch. Play legato: the last key held sets the
  pitch. The voice breathes while a key is down, or all the time with **Breathe** on; **Pitch**
  sets the pitch without a key. The mouth and throat controls shape the vowel, and the vowel pad
  (**Front** and **Open**) blends CHIMERA's calibrated vowels and moves those controls to match.
- **Speak** and **Beatbox** are planned and play as Play does for now.

The pitch wheel bends ±2 semitones.

## Parameters

| Page | Parameters |
|---|---|
| SING | Mode, Lyrics, Start Syl, Body; Vibrato, Vib Rate, Breathy, Ring; Breath, Pressed, Bright, Glide; Rough, Fry, Twang, False Folds |
| PLAY | Pitch, Breath, Pressed, Vibrato; Bright, Breathy, Rough, Fry; Rounding, Lips Open, Protrude, Jaw; Soft Palate, Larynx, Breathe, Whisper |
| VOWEL | Front, Open, Tongue Root, Tip Curl; Tongue Fwd, Tongue Up, Tip Fwd, Tip Up |
| BODY | Body (Male, Female, Child, Blend), Female and Child (Blend), Gain |

- **Breath** is the lung pressure (cmH2O). **Pressed** is how tightly the vocal folds close.
- **Bright**, **Breathy**, **Rough** and **Fry** shape the sound of the vocal folds. **Whisper**
  turns the voicing off.
- **Ring** and **Twang** both narrow the tube just above the vocal folds; the larger of the two is used.
- **Tip Fwd** and **Tip Up** place the tongue tip; at their starting values it rests where the
  jaw and tongue body put it.
- **Blend** plays the nearest of the three bodies for now. Mixing the bodies continuously comes later.
- **Restart** and **MPE** are reserved for the next version.

## How it fits on a 32-bit MPC

CHIMERA's engine is unchanged in what it models. To fit, the engine runs the airway at the
sample rate, without CHIMERA's usual 2x oversampling. It also uses longer sections in the trachea
and nose, updates the mouth shape every 4 control steps, and computes the airway in single precision
with the MPC's vector unit.

## Build

```
python "plugins/CHIMERA MPC/build_chimera.py"
```

This builds the engine (`engine/`, Rust) for every target, then runs the SDK's `tools/build.py`,
which links it in. It needs rustup with the stable and nightly toolchains:

```
rustup target add x86_64-pc-windows-gnu aarch64-unknown-linux-gnu
rustup +nightly target add armv7-unknown-linux-gnueabihf
```

The 32-bit build uses nightly Rust because its vector code (NEON intrinsics) is not in stable Rust
yet on 32-bit ARM. The engine is built from the `mpc-opt` branch of a CHIMERA checkout at
`../../../chimera-vocal-synth`.

## Project layout

- `plugin.json`: manifest (name, manufacturer, uid). The `uid` must match the C++ `uniqueId`.
- `src/CHIMERAMPC.cpp`: the plugin (parameters, MIDI, output); `src/chimera_mpc.h`: the engine's C interface.
- `engine/`: CHIMERA's engine behind that interface (Rust static library).
- `build_chimera.py`: the build.
- `ui.json`: the pages on the MPC's screen.
- `Content/lyrics.txt`: the eight lyrics lines; `Content/cmudict.txt.gz`: the CMU pronouncing dictionary.
- `Content/CHIMERA-LICENSE.txt`, `Content/CMUDICT-LICENSE.txt`: the licenses, shipped with the plugin.
- `engine/tests/api.rs`: the engine through its C interface (`cargo test --release` in `engine/`), with WAVs in `build/listen/`.

## License

CHIMERA is MIT licensed, copyright Jameson Hodge; its license is in `Content/CHIMERA-LICENSE.txt`.
The CMU pronouncing dictionary is copyright Carnegie Mellon University, under the license in
`Content/CMUDICT-LICENSE.txt`.
