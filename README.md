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
- **Speak**: each note says the next line of the chosen text. **Speech** picks one of eight blocks
  of lines in `Content/speech.txt` (blocks separated by a blank line); **Start Line** sets where it
  starts and **Speech Rate** how fast it talks. C3 speaks at the body's natural pitch; other keys
  move the whole line up or down, up to two octaves. With **IPA** on, the text is read as IPA.
  **Speak** says the next line without a key. **Consonant** says one consonant between two
  vowels, as CHIMERA's consonant chart does; **Pad Vowel** picks the vowel around it.
- **Beatbox**: a 16-step pattern of beatbox sounds. **Beat** picks one of CHIMERA's seven patterns
  (it fills the steps, Tempo and Swing) or keeps **Your Beat**. **Play Beat** runs it on its own
  tempo; with **Follow Host** on, it plays along with the MPC's sequencer while that runs. The
  MPC's pads 1-16 (notes 36-51: set the track's Note Mapping to Chromatic C1) each play the sound
  in the BEATBOX square with the same number, and the square lights up; the beat lights its squares
  as it plays them. Other keys below C3 hit drum sounds (General MIDI's drum map), keys from C3 up
  hum two octaves lower, and **Pad** hits a sound without a key. **Mic FX** is CHIMERA's
  close-mic sound.

With **Restart** on, Sing and Speak start again from Start Syl and Start Line each time the MPC's
transport starts.

**Typing your own words**: on the WORDS page, pick **Type Into** (Speak or Sing: what Speak says or what Sing sings) and **Type Slot**
(1 to 8), then type on the keyboard. **Del** takes the last character back, **Clear** empties the slot,
and for Speak **Line** starts a new line: each note says the next line. The slot starts with the line
from the text file; what you type replaces it and is saved with the project. A moment after you stop
typing, the next note says (or sings) the new words; **Speak** says the next line without a key.
Type Slot and the slot that plays move together (Speech on the SPEAK page, or Lyrics on SING while
typing for Sing), so the words on the WORDS page are the ones you hear. The keyboard types while the
MPC's sequence is stopped, so taps recorded as automation never type again on playback.

The pitch wheel bends ±2 semitones, and channel or key pressure (aftertouch) breathes harder.
With **MPE** on, MIDI channels 2 to 16 carry one note each: its own bend (±48 semitones),
pressure, and slide (CC 74), which moves the brightness.

## The screen

CHIMERA's look (glowing outlines on black): the controls on the left, so the MPC's Q-Link window,
which slides in from the right, never covers them, and on the right the vocal tract, a full head in
profile. Its tongue, jaw, upper and lower lip, larynx and soft palate each move on their own, so
turning one knob (or dragging the vowel pad) moves just that part, and in Sing, Speak and Beatbox
they all follow the voice. The amber lines across the mouth and throat glow brighter as the voice
gets louder and go dark when it is silent. Each column of four Q-Links is one of
CHIMERA's pages of four, colored green, red, pink and white from top to bottom, matching the column
of four in the Q-Link window. Under the chin, the Jaw, Lips and Velum readouts follow their knobs;
Velum says Shut while the soft palate closes off the nose.
Buttons that are on are solid; off, they are an outline. On KIT, the pad last heard lights up,
whether you tapped it, played its drum key, or the beat played it.

| Page | Columns | Right side |
|---|---|---|
| SING | SONG (Vibrato, Vib Rate, Breathy, Ring); VOICE (Breath, Pressed, Bright, Glide); TONE (Rough, Fry, Twang, False Folds); LYRICS (Lyrics, Start Syl, Restart, Gain) | the vocal tract, moving as it sings |
| PLAY | VOICE (Pitch, Breath, Pressed, Vibrato); TONE (Bright, Breathy, Rough, Fry); MOUTH (Rounding, Lips Open, Protrude, Jaw); THROAT (Soft Palate, Larynx, Twang, False Folds) | the vocal tract |
| VOWEL (press PLAY again) | VOWEL (Front, Open, Soft Palate, Rounding); TONGUE (Tongue Fwd, Tongue Up, Tip Fwd, Tip Up); MORE (Tongue Root, Tip Curl, Glide, Vib Rate); VOICE (Pitch, Breath, Pressed, Bright) | the vowel chart: drag to set Front and Open |
| SPEAK | SPEAK (Speech Rate, Start Line, Speech, IPA); VOICE; TONE (Breathy, Rough, Fry, Ring); SHAPE (Jaw, Lips Open, Larynx, Soft Palate) | the vocal tract |
| SOUNDS (press SPEAK again) | as SPEAK | the consonant buttons (tap one to say it) and the vowel around it |
| BEATBOX | the 16 steps, laid out as the MPC's pads (1-4 on the bottom row, 13-16 on top); Q-Link n turns square n | the vocal tract, the beat's name and tempo |
| KIT (press BEATBOX again) | BEAT (Beat, Tempo, Swing, Mic FX); HUM VOICE; TRANSPORT (Follow Host, Play Beat, Gain) | the ten pads |
| WORDS (function key 5) | one row: Type Into, Type Slot, Speech Rate, Start Line | the end of the text being typed, with a full-width keyboard below |
| BODY (function key 6) | BODY (Body, Female, Child, Gain); SETUP (MPE, Restart, Whisper, Breathe) | the vocal tract and the credit; the body chips are in the top bar |

The mode chips in the top bar switch the mode from any page; the button at the top left is the
page's main action (Breathe, Speak, Play Beat).

## Parameter notes

- **Gain** turns the voice's own level up or down before its output limiter, so a boost makes it
  louder up to the limit and never clips.
- **Breath** is the lung pressure (cmH2O). **Pressed** is how tightly the vocal folds close.
- **Bright**, **Breathy**, **Rough** and **Fry** shape the sound of the vocal folds. **Whisper**
  turns the voicing off.
- **Ring** and **Twang** both narrow the tube just above the vocal folds; the larger of the two is used.
- **Tip Fwd** and **Tip Up** place the tongue tip; at their starting values it rests where the
  jaw and tongue body put it.
- **Blend** mixes the bodies with **Female** and **Child**. Building a blended voice takes a
  moment: the previous body keeps playing until it is ready.
- A step's **m** (hum) plays at a fixed pitch; the Hum Bass pattern's own hum notes are not kept.

## Presets

Eight built-in voices, in the MPC's preset list: **Init** (CHIMERA's own defaults), **Natural**,
**Breathy**, **Belt**, **Opera**, **Airy**, **Rough** and **Child**. A preset sets the voice and the
body (Gain through Child, the first 14 parameters) and leaves the mode, your words, the mouth's shape
and the beat as they are, so you can change voices in the middle of a song. Breathy and Airy use the
female body and Child the child body; the others use the male body. Your own settings, words and beat
included, are saved with MPC presets of your own (save one from the plugin's preset menu).
`engine/examples/presets.rs` renders each preset singing and holding a vowel, with its loudness.

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
- `src/CHIMERAMPC.cpp`: the plugin (parameters, MIDI, transport, output); `src/chimera_mpc.h`: the engine's C interface.
- `engine/`: CHIMERA's engine behind that interface (Rust static library); `engine/src/worker.rs` is its own
  thread (dictionary, lyrics, spoken lines, blended voices); `engine/src/close_mic.rs` is CHIMERA's close-mic sound;
  `engine/src/face.rs` picks the face's frames each block (read-only parameters Jaw Pos, Tongue Pos, Lips Pos,
  Lower Lip, Larynx Pos, Sound Lvl and Velum Pos).
- `engine/examples/fit_face.rs`: renders speech, singing, consonants, beats, the vowel pad and the Play knobs,
  clusters the tongue outlines it sees and writes `engine/src/face_tables.rs` (the tongue frames and the map
  the engine picks them with). Run it again after changing the engine's articulation.
- `build_chimera.py`: the build.
- `tools/make_skin.py`: draws the screen (`skins/`, previews in `build/preview/`, including `face_*.png`
  check postures with the true outlines drawn over the layers) from the build's `build/params.json` and the
  vocal tract's outlines (`cargo run --release --example dump_face` in `engine/`, written to
  `build/face.json`); `ui.json` is no longer used once `skins/` exists.
- `Content/lyrics.txt`: the eight lyrics lines; `Content/speech.txt`: the eight blocks of spoken lines;
  `Content/cmudict.txt.gz`: the CMU pronouncing dictionary.
- `Content/CHIMERA-LICENSE.txt`, `Content/CMUDICT-LICENSE.txt`: the licenses, shipped with the plugin.
- `engine/tests/api.rs`: the engine through its C interface (`cargo test --release` in `engine/`), with WAVs in `build/listen/`.

## License

CHIMERA is MIT licensed, copyright Jameson Hodge; its license is in `Content/CHIMERA-LICENSE.txt`.
The CMU pronouncing dictionary is copyright Carnegie Mellon University, under the license in
`Content/CMUDICT-LICENSE.txt`.
