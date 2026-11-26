# CHIMERA MPC

CHIMERA's physical-model voice on Akai MPC standalone hardware, by anthonytheinventor.

[CHIMERA](https://github.com/jamesondh/chimera-vocal-synth) is a voice synthesizer by Jameson Hodge.
Every sound it makes comes from a simulated airway (lungs, vocal folds, throat, mouth and nose), not
from samples. This plugin runs CHIMERA's own engine, with settings that let it fit a 32-bit MPC.
CHIMERA MPC is an independent project, not made or endorsed by Jameson Hodge.

## What it does

This version is **Play mode**: play a note and the voice sings one held vowel at that pitch. Play
legato: the last key held sets the pitch, and the voice breathes while any key is down. The pitch
wheel bends ±2 semitones.

Sing (lyrics, one syllable per note), Speak, Beatbox, the tongue handles, the vowel pad and the
animated head are planned next.

## Parameters

| Page | Parameters |
|---|---|
| PLAY | Breath, Pressed, Vibrato, Glide; Bright, Breathy, Rough, Fry; Rounding, Lips Open, Protrude, Jaw; Soft Palate, Larynx, Twang, False Folds |
| SHAPE | Body (Male, Female, Child, Blend), Female and Child (Blend), Gain; Tongue Root, Tip Curl, Ring, Vib Rate |

- **Breath** is the lung pressure (cmH2O). **Pressed** is how tightly the vocal folds close.
- **Bright**, **Breathy**, **Rough** and **Fry** shape the sound of the vocal folds.
- **Ring** and **Twang** both narrow the tube just above the vocal folds; the larger of the two is used.
- **Blend** plays the nearest of the three bodies for now. Mixing the bodies continuously comes later.

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
- `Content/CHIMERA-LICENSE.txt`: CHIMERA's MIT license, shipped with the plugin.

## License

CHIMERA is MIT licensed, copyright Jameson Hodge; its license is in `Content/CHIMERA-LICENSE.txt`.
