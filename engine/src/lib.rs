//! CHIMERA's voice for the MPC: the engine with the settings that fit a 32-bit MPC, behind a
//! C interface the plugin's C++ calls (see `chimera_mpc.h`).
//!
//! Threads: `cm_new` and `cm_free` allocate and free, so the plugin calls them outside the audio
//! callback (`prepare`, the destructor). Everything else runs on the audio thread and allocates
//! nothing: the three preset body voices are built in `cm_new` and changing the body swaps one
//! in; the engine's own thread (`worker`) loads the dictionary, plans the lyrics and the spoken
//! lines, builds a blended body's voice, and frees what the audio thread hands back. The audio
//! thread only `try_lock`s what it shares with that thread.
//!
//! Every entry point catches a panic. After one the engine is marked failed and renders silence
//! from then on, so a bug can never take the MPC app down.
//!
//! Modes, as CHIMERA's own plugin has them:
//! - Sing: each note sings the next syllable of the chosen lyrics (CHIMERA's Singer).
//! - Play: notes sing one held posture, legato (the last key held sets the pitch).
//! - Speak: each note says the next line of the chosen text, its pitch moved by the key.
//! - Beatbox: the 16-step sequencer (its own tempo, or following the host's transport); notes
//!   below C3 hit drum sounds (General MIDI's map), notes from C3 up hum two octaves down.

mod close_mic;
mod worker;

use chimera::anatomy::human::{ArticKey, ArticPatch, BodyMix, NEUTRAL};
use chimera::airway::network::{APPROX_KINETIC, APPROX_WALLS};
use chimera::control::score::{Gesture, Label};
use chimera::control::singer::Singer;
use chimera::engine::{Engine, default_f0};
use chimera::math::{exp, ln, midi_to_hz, pow};
use chimera::phonetics::beatbox::{BEAT_PRESETS, BeatStep, parse_beat};
use chimera::phonetics::planner::vowel_table;
use chimera::phonetics::sing::LiveSyllable;
use chimera::phonetics::vowel_data::VOWELS;
use chimera::voice::human_voice::{HumanVoice, SourcePatch, oversample_for};
use close_mic::CloseMic;
use std::ffi::{CStr, c_char};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::Arc;
use worker::{Hub, Lyrics, Request, Speech, TRASH_ROOM, Trash};

/// The plugin's parameters, in the plugin's order (`src/CHIMERAMPC.cpp` declares the same list).
/// Values arrive in plain units.
pub mod p {
    pub const GAIN: usize = 0; // dB, applied by the C++ side
    pub const BREATH: usize = 1; // cmH2O
    pub const PRESSED: usize = 2; // 0..1
    pub const BRIGHT: usize = 3; // 0..1
    pub const GLIDE: usize = 4; // s
    pub const BREATHY: usize = 5;
    pub const ROUGH: usize = 6;
    pub const FRY: usize = 7;
    pub const RING: usize = 8;
    pub const VIBRATO: usize = 9; // cents
    pub const VIBRATO_RATE: usize = 10; // Hz
    pub const BODY: usize = 11; // 0 male, 1 female, 2 child, 3 blend
    pub const BODY_FEMALE: usize = 12; // 0..1 (blend)
    pub const BODY_CHILD: usize = 13;
    pub const JAW: usize = 14; // cm
    pub const LIP_OPEN: usize = 15;
    pub const PROTRUDE: usize = 16;
    pub const ROUNDING: usize = 17; // 1 - lip width
    pub const ROOT: usize = 18;
    pub const TIP_CURL: usize = 19;
    pub const VELUM: usize = 20; // cm²
    pub const LARYNX: usize = 21; // cm
    pub const TWANG: usize = 22; // epilarynx 0..1
    pub const FALSE_FOLDS: usize = 23; // ventricular 0..1
    pub const TONGUE_FWD: usize = 24; // tongue body x (template cm)
    pub const TONGUE_UP: usize = 25; // tongue body y
    pub const TIP_FWD: usize = 26; // tongue tip x
    pub const TIP_UP: usize = 27; // tongue tip y
    pub const VOWEL_FRONT: usize = 28; // 0..1 of the body's F2 range
    pub const VOWEL_OPEN: usize = 29; // 0..1 of the body's F1 range
    pub const WHISPER: usize = 30; // 0/1
    pub const PITCH: usize = 31; // Hz
    pub const MODE: usize = 32; // 0 Sing, 1 Play, 2 Speak, 3 Beatbox
    pub const BREATHE: usize = 33; // 0/1: Play's held breath without a key
    pub const RESTART: usize = 34; // 0/1: lyrics and lines from their start when the transport starts
    pub const MPE: usize = 35; // 0/1 (not used yet)
    pub const LYRICS: usize = 36; // slot 0..7
    pub const START_SYL: usize = 37; // syllable the lyrics start from
    pub const SPEECH: usize = 38; // slot 0..7
    pub const START_LINE: usize = 39; // line the speech starts from
    pub const SPEECH_RATE: usize = 40; // x
    pub const IPA: usize = 41; // 0/1: the speech text is IPA
    pub const SPEAK: usize = 42; // momentary: say the next line
    pub const CONSONANT: usize = 43; // (not used yet: the consonant chart)
    pub const PAD_VOWEL: usize = 44; // (not used yet)
    pub const BEAT: usize = 45; // 0..6 a preset, 7 your beat (the C++ side writes a preset's steps)
    pub const TEMPO: usize = 46; // bpm
    pub const SWING: usize = 47; // 0..0.6
    pub const MIC: usize = 48; // 0/1: the close-mic sound in Beatbox
    pub const FOLLOW: usize = 49; // 0/1: follow the host's transport
    pub const PLAY_BEAT: usize = 50; // 0/1: the sequencer runs on its own tempo
    pub const STEP_1: usize = 51; // ..66: 0 rest, 1.. STEP_IDS
    pub const PAD: usize = 67; // momentary: 0 none, 1.. STEP_IDS
    pub const COUNT: usize = 68;
}

const MODE_SING: i32 = 0;
const MODE_PLAY: i32 = 1;
const MODE_SPEAK: i32 = 2;
const MODE_BEATBOX: i32 = 3;

/// The steps' and pads' sounds, after "rest" / "none" (parameter value 0).
pub const STEP_IDS: [&str; 10] = ["B", "t", "Pf", "K", "^K", "!", "P!", "tss", "kch", "m"];
const STEPS: usize = 16;

/// Pitch bend range each way (semitones), as CHIMERA's plugin.
const BEND_RANGE: f64 = 2.0;
/// Most keys held at once that legato falls back through.
const MAX_KEYS: usize = 16;
/// Lyrics and speech slots.
pub const SLOTS: usize = 8;
/// What a slot sings before its lyrics are ready, or when it is empty: CHIMERA's default.
pub const DEFAULT_LYRICS: &str = "Hello there, I am a voice made of air.";
/// Speak: the key that speaks at the body's natural pitch, and how far a key moves it
/// (semitones); a line's time 0 comes this long after its note (s). As CHIMERA's plugin.
const SPEECH_ROOT: f64 = 60.0;
const SPEECH_RANGE: f64 = 24.0;
const SPEECH_LEAD: f64 = 0.03;
/// Beatbox: notes from this one up hum, this many semitones down; a hum's tag and longest length;
/// a pad hit's length (s). As CHIMERA's plugin.
const HUM_FROM: u8 = 60;
const HUM_DOWN: u8 = 24;
const HUM_TAG: f64 = -1.0;
const HUM_LEN: f64 = 3600.0;
const BEAT_HIT_LEN: f64 = 0.18;

/// A voice with the MPC settings: 1x at 44.1 and 48 kHz, lean trachea and nose sections,
/// the geometry every 4 control steps and every 4 samples, no second geometry pass after a
/// contact correction, the cheaper arithmetic and the single-precision (NEON) airway.
pub(crate) fn mpc_voice(fs: f64, body: BodyMix) -> Box<HumanVoice> {
    let os = if fs >= 40000.0 { 1 } else { oversample_for(fs) };
    let mut v = Box::new(HumanVoice::with_sections(fs, body.body(), os, 2.0, 1.4));
    v.set_anat_every(4);
    v.set_geo_every(4);
    v.set_contact_reapply(false);
    v.airway.net.approx = APPROX_WALLS | APPROX_KINETIC;
    v.set_fast_airway(true);
    v.set_source(&SourcePatch::f0(default_f0(body)));
    // a voice's first control step sizes some of its buffers: do it here, not on the audio thread
    v.render(&mut [0.0; 128]);
    v
}

pub(crate) const BODIES: [BodyMix; 3] = [BodyMix::MALE, BodyMix::FEMALE, BodyMix::CHILD];
const BODY_NAMES: [&str; 3] = ["male", "female", "child"];

// ---------------------------------------------------------------- the vowel pad

/// The articulators the vowel pad sets, in the order `cm_vowel_posture` returns them.
const PAD_KEYS: [ArticKey; 9] = [ArticKey::TbX, ArticKey::TbY, ArticKey::Jaw, ArticKey::LipAp, ArticKey::LipProt, ArticKey::LipWidth, ArticKey::Root, ArticKey::Larynx, ArticKey::TtCurl];
/// The vowels the pad leaves out (Mandarin-only, as CHIMERA's own pad).
const NOT_ON_THE_MAP: [&str; 4] = ["ɿ", "ʅ", "u̠", "ʊ̠"];

#[derive(Clone, Copy)]
struct PadVowel {
    f1: f64,
    f2: f64,
    artic: [f64; 9],
}

/// One body's pad: its calibrated vowels and the pad's F1 and F2 ranges (Hz).
struct Pad {
    vowels: Vec<PadVowel>,
    f1: (f64, f64),
    f2: (f64, f64),
}

fn pad_for(b: usize) -> Pad {
    let table = vowel_table().get(BODY_NAMES[b]);
    let mut vowels = Vec::new();
    for v in VOWELS {
        if NOT_ON_THE_MAP.contains(&v.ipa) {
            continue;
        }
        let Some(t) = table.and_then(|t| t.get(v.ipa)) else { continue };
        if t.formants.len() < 2 {
            continue;
        }
        let mut artic = [0.0; 9];
        for (i, k) in PAD_KEYS.iter().enumerate() {
            artic[i] = t.artic.get(*k).unwrap_or(NEUTRAL.get(*k));
        }
        vowels.push(PadVowel { f1: t.formants[0], f2: t.formants[1], artic });
    }
    // CHIMERA's pad ranges per body (VowelMap.tsx)
    let (f1, f2) = [((230.0, 950.0), (700.0, 2700.0)), ((280.0, 1150.0), (800.0, 3200.0)), ((300.0, 1250.0), (900.0, 3500.0))][b];
    Pad { vowels, f1, f2 }
}

impl Pad {
    /// The pad's posture at (front, open), 0..1 each: CHIMERA's own blend of the four nearest
    /// calibrated vowels (inverse distance cubed in log formants, F1 counted 2.2 times).
    fn posture(&self, front: f64, open: f64) -> [f64; 9] {
        let lerp_log = |r: (f64, f64), u: f64| exp(ln(r.0) + u.clamp(0.0, 1.0) * (ln(r.1) - ln(r.0)));
        let f2 = lerp_log(self.f2, front);
        let f1 = lerp_log(self.f1, open);
        let mut best = [(0.0f64, 0usize); 4];
        let mut n = 0;
        for (i, v) in self.vowels.iter().enumerate() {
            let d = ((ln(f1 / v.f1) * 2.2).powi(2) + ln(f2 / v.f2).powi(2)).sqrt();
            let w = 1.0 / (d + 0.02).powi(3);
            let mut j = n.min(3);
            if n < 4 {
                n += 1;
            } else if w <= best[3].0 {
                continue;
            }
            while j > 0 && best[j - 1].0 < w {
                best[j] = best[j - 1];
                j -= 1;
            }
            best[j] = (w, i);
        }
        let mut out = [0.0; 9];
        if n == 0 {
            for (i, k) in PAD_KEYS.iter().enumerate() {
                out[i] = NEUTRAL.get(*k);
            }
            return out;
        }
        let sum: f64 = best[..n].iter().map(|b| b.0).sum();
        for (w, i) in &best[..n] {
            for (o, a) in out.iter_mut().zip(self.vowels[*i].artic.iter()) {
                *o += a * w / sum;
            }
        }
        out
    }
}

// ---------------------------------------------------------------- the engine

/// Which voice the engine is playing.
#[derive(Clone, Copy, PartialEq)]
enum Playing {
    Preset(usize),
    Blend((f32, f32)),
}

/// The host's transport this block.
#[derive(Clone, Copy, Default)]
struct Transport {
    playing: bool,
    bpm: Option<f64>,
    beats: Option<f64>,
}

pub struct ChimeraMpc {
    engine: Engine,
    fs: f64,
    /// the preset voices not playing (the playing one's entry is None)
    voices: [Option<Box<HumanVoice>>; 3],
    playing: Playing,
    pads: [Pad; 3],
    singer: Singer,
    hub: Arc<Hub>,
    /// what goes back to the worker to be freed, while the trash is locked (room reserved)
    to_trash: Vec<Trash>,
    /// each slot's syllables once ready (moved in, never freed here)
    lyrics: Lyrics,
    /// the slot the singer holds (its Vec is in the singer, the slot's entry is empty)
    sung_slot: Option<usize>,
    /// the Singer's own first lyrics ("ah"), parked: dropping them would free on the audio thread
    first_lyrics: Vec<LiveSyllable>,
    speech: Option<Speech>,
    speech_cursor: usize,
    /// the request last sent to the worker
    sent: Option<Request>,
    params: [f32; p::COUNT],
    /// the parameters as last applied (NaN: never)
    applied: [f32; p::COUNT],
    mode: i32,
    keys: [u8; MAX_KEYS],
    n_keys: usize,
    /// Beatbox: the keys humming (newest last)
    hums: [u8; MAX_KEYS],
    n_hums: usize,
    wheel: f64,
    key_bend: f64,
    transport: Transport,
    following: bool,
    close_mic: CloseMic,
    failed: bool,
}

impl ChimeraMpc {
    fn new(fs: f64, content: Option<String>) -> ChimeraMpc {
        let mut engine = Engine::new(fs);
        let mut male = mpc_voice(fs, BODIES[0]);
        // the engine's own first voice (CHIMERA's default settings) is not used: freed here
        engine.install_voice(&mut male, BODIES[0]);
        drop(male);
        let hub = Hub::new();
        worker::start(hub.clone(), fs, content);
        let mut e = ChimeraMpc {
            engine,
            fs,
            voices: [None, Some(mpc_voice(fs, BODIES[1])), Some(mpc_voice(fs, BODIES[2]))],
            playing: Playing::Preset(0),
            pads: [pad_for(0), pad_for(1), pad_for(2)],
            singer: Singer::new(),
            hub,
            to_trash: Vec::with_capacity(TRASH_ROOM),
            lyrics: Vec::new(),
            sung_slot: None,
            first_lyrics: Vec::new(),
            speech: None,
            speech_cursor: 0,
            sent: None,
            params: defaults(),
            applied: [f32::NAN; p::COUNT],
            mode: MODE_SING,
            keys: [0; MAX_KEYS],
            n_keys: 0,
            hums: [0; MAX_KEYS],
            n_hums: 0,
            wheel: 1.0,
            key_bend: 1.0,
            transport: Transport::default(),
            following: false,
            close_mic: CloseMic::new(fs),
            failed: false,
        };
        e.apply_params();
        e.send_request();
        e
    }

    /// The preset body nearest the voice playing (for the vowel pad and the spoken lines).
    fn preset(&self) -> usize {
        match self.playing {
            Playing::Preset(b) => b,
            Playing::Blend((f, c)) => {
                let mix = BodyMix::new(f as f64, c as f64);
                BODIES.iter().position(|x| x.preset() == Some(mix.nearest())).unwrap_or(0)
            }
        }
    }

    /// Play `voice` (built for `what`); the voice it replaces goes back to its slot or out.
    fn install(&mut self, mut voice: Box<HumanVoice>, what: Playing) {
        self.stop_notes();
        let mix = match what {
            Playing::Preset(b) => BODIES[b],
            Playing::Blend((f, c)) => BodyMix::new(f as f64, c as f64),
        };
        self.engine.install_voice(&mut voice, mix);
        match self.playing {
            Playing::Preset(b) => self.voices[b] = Some(voice),
            Playing::Blend(_) => self.trash(Trash::Voice(voice)),
        }
        self.playing = what;
        self.singer.body = mix;
        // the new voice took over the source and posture; the parameters set the rest again
        // (the lyrics stay: the singer keeps its syllables)
        let lyrics = self.applied[p::LYRICS];
        self.applied = [f32::NAN; p::COUNT];
        self.applied[p::LYRICS] = lyrics;
    }

    fn trash(&mut self, t: Trash) {
        if self.to_trash.len() < TRASH_ROOM {
            self.to_trash.push(t);
        } else {
            // the worker has not taken anything for a long time: leak rather than free here
            std::mem::forget(t);
        }
    }

    /// Hand the trash to the worker and send it the request, if their locks are free.
    fn talk_to_worker(&mut self) {
        if !self.to_trash.is_empty()
            && let Ok(mut t) = self.hub.trash.try_lock()
        {
            while let Some(x) = self.to_trash.pop() {
                if t.len() == t.capacity() {
                    self.to_trash.push(x);
                    break;
                }
                t.push(x);
            }
        }
        self.send_request();
        // what the worker has made
        let Ok(mut o) = self.hub.out.try_lock() else { return };
        if self.lyrics.is_empty()
            && let Some(l) = o.lyrics.take()
        {
            self.lyrics = l;
            self.sung_slot = None;
            self.applied[p::LYRICS] = f32::NAN;
        }
        let speech = o.speech.take();
        let blend = o.blend.take();
        drop(o);
        if let Some(s) = speech {
            if let Some(old) = self.speech.replace(s) {
                self.trash(Trash::Speech(old));
            }
            self.speech_cursor = self.speech_cursor.min(self.lines_len().saturating_sub(1));
        }
        if let Some((mix, v)) = blend {
            if self.wanted_blend() == Some(mix) && self.playing != Playing::Blend(mix) {
                self.install(v, Playing::Blend(mix));
            } else {
                self.trash(Trash::Voice(v));
            }
        }
    }

    fn wanted_blend(&self) -> Option<(f32, f32)> {
        (self.params[p::BODY].round() as i32 == 3).then(|| (self.params[p::BODY_FEMALE], self.params[p::BODY_CHILD]))
    }

    fn send_request(&mut self) {
        let r = Request { speech_rate: self.params[p::SPEECH_RATE], ipa: self.params[p::IPA] >= 0.5, blend: self.wanted_blend() };
        if self.sent == Some(r) {
            return;
        }
        if let Ok(mut q) = self.hub.request.try_lock() {
            *q = Some(r);
            self.sent = Some(r);
        }
    }

    // --- lyrics and lines

    fn sing_slot(&mut self, slot: usize) {
        if slot >= self.lyrics.len() || self.sung_slot == Some(slot) {
            return;
        }
        let incoming = core::mem::take(&mut self.lyrics[slot]);
        if incoming.is_empty() {
            return;
        }
        let outgoing = self.singer.set_syllables(incoming);
        match self.sung_slot {
            Some(prev) => self.lyrics[prev] = outgoing,
            None => self.first_lyrics = outgoing,
        }
        self.sung_slot = Some(slot);
    }

    fn speech_slot(&self) -> usize {
        (self.params[p::SPEECH].round().max(0.0) as usize).min(SLOTS - 1)
    }

    fn lines_len(&self) -> usize {
        self.speech.as_ref().map_or(0, |s| s[self.speech_slot()][self.preset()].len())
    }

    /// Say the next line at this pitch ratio (the whole contour moves through the bend).
    fn say_next(&mut self, key_bend: f64) {
        let (slot, b) = (self.speech_slot(), self.preset());
        let n = self.speech.as_ref().map_or(0, |s| s[slot][b].len());
        if n == 0 {
            return;
        }
        let i = self.speech_cursor % n;
        self.key_bend = key_bend;
        self.update_bend();
        if let Some(s) = self.speech.as_ref() {
            let g: &[Gesture] = &s[slot][b][i];
            self.engine.schedule(g, SPEECH_LEAD, true);
        }
        self.speech_cursor = (i + 1) % n;
    }

    // --- beatbox

    fn beat_step(&self, i: usize) -> Option<BeatStep> {
        let k = self.params[p::STEP_1 + i].round() as usize;
        (1..=STEP_IDS.len()).contains(&k).then(|| BeatStep { id: Label::new(STEP_IDS[k - 1]), note: None, accent: None })
    }

    fn beat_time(&mut self, id: &str) -> f64 {
        let wait = self.engine.beat_lead(id);
        self.engine.clock() + wait
    }

    fn beat_hit(&mut self, id: &str, accent: f64) {
        let step = BeatStep { id: Label::new(id), note: None, accent: Some(accent) };
        let hit = self.beat_time(id);
        self.engine.beat_hit_at(&step, hit, BEAT_HIT_LEN, None);
    }

    fn start_hum(&mut self, midi: u8, accent: f64) {
        let hit = self.beat_time("m");
        self.engine.release_at(HUM_TAG, hit);
        let step = BeatStep { id: Label::new("m"), note: Some((midi.saturating_sub(HUM_DOWN)) as f64), accent: Some(accent) };
        self.engine.beat_hit_at(&step, hit, HUM_LEN, Some(HUM_TAG));
    }

    fn stop_hum(&mut self) {
        let t = self.beat_time("m");
        self.engine.release_at(HUM_TAG, t);
    }

    fn follow_beat(&mut self, samples: usize) {
        let t = self.transport;
        let host = (self.mode == MODE_BEATBOX && self.params[p::FOLLOW] >= 0.5 && t.playing).then_some(()).and(t.beats.zip(t.bpm)).filter(|(_, bpm)| *bpm > 0.0);
        match host {
            Some((beats, bpm)) => {
                if !self.following {
                    self.engine.set_beat_playing(false);
                    self.following = true;
                }
                let end = beats + samples as f64 / self.fs * bpm / 60.0;
                self.engine.beat_follow(beats, end, bpm, 0.0);
            }
            None if self.following => {
                self.following = false;
                self.engine.beat_stop_following();
                self.engine.set_beat_tempo(Some(self.params[p::TEMPO] as f64), None);
                self.engine.set_beat_playing(self.mode == MODE_BEATBOX && self.params[p::PLAY_BEAT] >= 0.5);
            }
            None => {}
        }
    }

    // --- parameters

    fn apply_params(&mut self) {
        let q = self.params;
        let was = self.applied;
        let changed = |i: usize| q[i].to_bits() != was[i].to_bits();
        // body: a preset at once; a blend once the worker has built its voice
        if changed(p::BODY) || changed(p::BODY_FEMALE) || changed(p::BODY_CHILD) {
            let b = q[p::BODY].round() as i32;
            if (0..3).contains(&b) && self.playing != Playing::Preset(b as usize) {
                let b = b as usize;
                if let Some(v) = self.voices[b].take() {
                    self.install(v, Playing::Preset(b));
                }
            }
        }
        let was = self.applied;
        let changed = |i: usize| q[i].to_bits() != was[i].to_bits();
        if changed(p::MODE) {
            let m = q[p::MODE].round() as i32;
            if m != self.mode {
                // a mode change ends whatever was sounding
                self.stop_notes();
                self.mode = m;
                self.key_bend = 1.0;
                self.update_bend();
            }
        }
        let source = [p::BREATH, p::PRESSED, p::BRIGHT, p::GLIDE, p::BREATHY, p::ROUGH, p::FRY, p::VIBRATO, p::VIBRATO_RATE, p::WHISPER, p::MODE];
        if source.iter().any(|&i| changed(i)) {
            // while singing, the notes' own gestures carry the vibrato (as CHIMERA's plugin)
            let depth = if self.mode == MODE_PLAY { q[p::VIBRATO] as f64 } else { 0.0 };
            self.singer.vibrato = q[p::VIBRATO] as f64;
            self.engine.set_source(&SourcePatch {
                pressure: Some(q[p::BREATH] as f64),
                tension: Some(q[p::PRESSED] as f64),
                brightness: Some(q[p::BRIGHT] as f64),
                glide: Some(q[p::GLIDE] as f64),
                breathiness: Some(q[p::BREATHY] as f64),
                roughness: Some(q[p::ROUGH] as f64),
                fry: Some(q[p::FRY] as f64),
                vibrato_depth: Some(depth),
                vibrato_rate: Some(q[p::VIBRATO_RATE] as f64),
                voiced: Some(q[p::WHISPER] < 0.5),
                ..Default::default()
            });
        }
        if (p::JAW..=p::TIP_UP).any(changed) || changed(p::RING) {
            let x = |i: usize| q[i] as f64;
            // Ring and Twang both move the epilarynx (CHIMERA's plugin sets Ring there; Twang is
            // the articulator): the larger one wins
            let epi = x(p::RING).max(x(p::TWANG));
            // the tip rests where the jaw and tongue body put it until its own controls move
            let tip_free = q[p::TIP_FWD] == NEUTRAL.tt_x as f32 && q[p::TIP_UP] == NEUTRAL.tt_y as f32;
            let mut a = ArticPatch::default()
                .with(ArticKey::Jaw, x(p::JAW))
                .with(ArticKey::LipAp, x(p::LIP_OPEN))
                .with(ArticKey::LipProt, x(p::PROTRUDE))
                .with(ArticKey::LipWidth, 1.0 - x(p::ROUNDING))
                .with(ArticKey::Root, x(p::ROOT))
                .with(ArticKey::TtCurl, x(p::TIP_CURL))
                .with(ArticKey::Velum, x(p::VELUM))
                .with(ArticKey::Larynx, x(p::LARYNX))
                .with(ArticKey::Epilarynx, epi)
                .with(ArticKey::Ventricular, x(p::FALSE_FOLDS))
                .with(ArticKey::TbX, x(p::TONGUE_FWD))
                .with(ArticKey::TbY, x(p::TONGUE_UP));
            if !tip_free {
                a = a.with(ArticKey::TtX, x(p::TIP_FWD)).with(ArticKey::TtY, x(p::TIP_UP));
            }
            self.engine.set_artic(&a, Some(tip_free));
        }
        if changed(p::PITCH) && !was[p::PITCH].is_nan() && self.mode == MODE_PLAY {
            // the Pitch control sets the pitch, as a key does (the last one moved wins)
            self.engine.set_source(&SourcePatch::f0(q[p::PITCH] as f64));
        }
        if changed(p::LYRICS) {
            self.sing_slot((q[p::LYRICS].round().max(0.0) as usize).min(SLOTS - 1));
        }
        if changed(p::START_SYL) || changed(p::LYRICS) {
            self.singer.set_cursor(q[p::START_SYL].round().max(0.0) as usize);
        }
        if changed(p::SPEECH) || changed(p::START_LINE) {
            self.speech_cursor = q[p::START_LINE].round().max(0.0) as usize;
        }
        if changed(p::SPEAK) && q[p::SPEAK] >= 0.5 && was[p::SPEAK] < 0.5 && self.mode == MODE_SPEAK {
            self.say_next(1.0);
        }
        // the sequencer
        if (p::STEP_1..p::STEP_1 + STEPS).any(changed) {
            let mut steps = [None; STEPS];
            for (i, s) in steps.iter_mut().enumerate() {
                *s = self.beat_step(i);
            }
            self.engine.set_beat_steps(&steps);
        }
        if changed(p::TEMPO) || changed(p::SWING) {
            self.engine.set_beat_tempo(Some(q[p::TEMPO] as f64), Some(q[p::SWING] as f64));
        }
        if (changed(p::PLAY_BEAT) || changed(p::MODE)) && !self.following {
            self.engine.set_beat_playing(self.mode == MODE_BEATBOX && q[p::PLAY_BEAT] >= 0.5);
        }
        if changed(p::PAD) && q[p::PAD] >= 0.5 && self.mode == MODE_BEATBOX {
            let k = q[p::PAD].round() as usize;
            if (1..=STEP_IDS.len()).contains(&k) {
                self.beat_hit(STEP_IDS[k - 1], 1.0);
            }
        }
        if changed(p::BREATHE) || changed(p::MODE) {
            self.update_gate();
        }
        self.applied = q;
    }

    fn update_gate(&mut self) {
        if self.mode != MODE_PLAY {
            return; // Sing, Speak and Beatbox breathe with their gestures
        }
        let on = self.n_keys > 0 || self.params[p::BREATHE] >= 0.5;
        self.engine.set_gate(if on { 1.0 } else { 0.0 });
    }

    fn update_bend(&mut self) {
        self.engine.set_bend(self.key_bend * self.wheel);
    }

    // --- notes

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.apply_params();
        match self.mode {
            MODE_SING => self.singer.note_on(&mut self.engine, note, velocity as f64),
            MODE_SPEAK => {
                let semitones = (note as f64 - SPEECH_ROOT).clamp(-SPEECH_RANGE, SPEECH_RANGE);
                self.say_next(pow(2.0, semitones / 12.0));
            }
            MODE_BEATBOX => {
                let accent = (velocity as f64 / 100.0).clamp(0.4, 1.25);
                if note >= HUM_FROM {
                    self.remove_hum(note);
                    if self.n_hums < MAX_KEYS {
                        self.hums[self.n_hums] = note;
                        self.n_hums += 1;
                    }
                    self.start_hum(note, accent);
                } else if let Some(id) = drum_pad(note) {
                    self.beat_hit(id, accent);
                }
            }
            _ => {
                self.remove_key(note);
                if self.n_keys == MAX_KEYS {
                    self.keys.copy_within(1.., 0);
                    self.n_keys -= 1;
                }
                self.keys[self.n_keys] = note;
                self.n_keys += 1;
                self.engine.set_source(&SourcePatch::f0(midi_to_hz(note as f64)));
                self.update_gate();
            }
        }
    }

    fn note_off(&mut self, note: u8) {
        match self.mode {
            MODE_SING => self.singer.note_off(&mut self.engine, note),
            // a line is a one-shot: it ends by itself, or when the next one cuts it
            MODE_SPEAK => {}
            MODE_BEATBOX => {
                if note < HUM_FROM {
                    return;
                }
                let was_top = self.n_hums > 0 && self.hums[self.n_hums - 1] == note;
                self.remove_hum(note);
                if was_top {
                    if self.n_hums > 0 {
                        self.start_hum(self.hums[self.n_hums - 1], 1.0);
                    } else {
                        self.stop_hum();
                    }
                }
            }
            _ => {
                self.remove_key(note);
                if self.n_keys > 0 {
                    // legato: back to the most recent key still held
                    self.engine.set_source(&SourcePatch::f0(midi_to_hz(self.keys[self.n_keys - 1] as f64)));
                }
                self.update_gate();
            }
        }
    }

    fn remove_key(&mut self, note: u8) {
        if let Some(i) = self.keys[..self.n_keys].iter().position(|&k| k == note) {
            self.keys.copy_within(i + 1..self.n_keys, i);
            self.n_keys -= 1;
        }
    }

    fn remove_hum(&mut self, note: u8) {
        if let Some(i) = self.hums[..self.n_hums].iter().position(|&k| k == note) {
            self.hums.copy_within(i + 1..self.n_hums, i);
            self.n_hums -= 1;
        }
    }

    /// Stop everything the keys are doing (mode change, transport stop, all notes off).
    fn stop_notes(&mut self) {
        self.singer.stop_all(&mut self.engine);
        self.n_keys = 0;
        if self.mode == MODE_SPEAK {
            self.engine.stop_speech();
            self.key_bend = 1.0;
            self.update_bend();
        }
        if self.mode == MODE_BEATBOX && self.n_hums > 0 {
            self.n_hums = 0;
            let now = self.engine.clock();
            self.engine.release_at(HUM_TAG, now);
        }
        self.update_gate();
    }

    fn set_transport(&mut self, t: Transport, samples: usize) {
        let was = self.transport;
        let started = t.playing && !was.playing;
        let jumped = t.playing && was.playing && matches!((t.beats, was.beats), (Some(now), Some(last)) if now < last - 1e-3);
        self.transport = t;
        if (started || jumped) && self.params[p::RESTART] >= 0.5 {
            self.singer.set_cursor(self.params[p::START_SYL].round().max(0.0) as usize);
            self.speech_cursor = self.params[p::START_LINE].round().max(0.0) as usize;
        }
        if was.playing && !t.playing {
            self.stop_notes();
        }
        self.follow_beat(samples);
    }

    fn render(&mut self, out: &mut [f32]) {
        self.talk_to_worker();
        self.apply_params();
        self.engine.render(out);
        if self.mode == MODE_BEATBOX && self.params[p::MIC] >= 0.5 {
            self.close_mic.process(out);
        }
        for x in out.iter_mut() {
            if !x.is_finite() {
                *x = 0.0;
            }
        }
    }
}

impl Drop for ChimeraMpc {
    fn drop(&mut self) {
        self.hub.quit.store(true, std::sync::atomic::Ordering::Relaxed);
    }
}

/// General MIDI's drum keys -> beatbox sounds, as CHIMERA's plugin maps them.
pub fn drum_pad(midi: u8) -> Option<&'static str> {
    Some(match midi {
        35 | 36 => "B",
        37 => "K",
        38 => "Pf",
        39 => "!",
        40 => "^K",
        41 | 43 | 45 | 47 | 48 | 50 => "P!",
        42 | 44 | 51 | 53 | 59 => "t",
        46 => "tss",
        49 | 52 | 55 | 57 => "kch",
        _ => return None,
    })
}

/// A beat preset as step parameter values (0 rest, 1.. STEP_IDS), with its tempo and swing.
pub fn preset_steps(i: usize) -> Option<([f32; STEPS], f32, f32)> {
    let pr = BEAT_PRESETS.get(i)?;
    let mut out = [0.0f32; STEPS];
    for (o, s) in out.iter_mut().zip(parse_beat(pr.text)) {
        *o = s.and_then(|s| STEP_IDS.iter().position(|id| *id == s.id.as_str())).map_or(0.0, |k| (k + 1) as f32);
    }
    Some((out, pr.bpm as f32, pr.swing as f32))
}

/// CHIMERA's own defaults (the voice source's) and the neutral posture, in parameter order.
fn defaults() -> [f32; p::COUNT] {
    let s = chimera::voice::human_voice::DEFAULT_SOURCE;
    let n = NEUTRAL;
    let mut d = [0.0f32; p::COUNT];
    d[p::GAIN] = 0.0;
    d[p::BREATH] = s.pressure as f32;
    d[p::PRESSED] = s.tension as f32;
    d[p::BRIGHT] = s.brightness as f32;
    d[p::GLIDE] = s.glide as f32;
    d[p::BREATHY] = s.breathiness as f32;
    d[p::ROUGH] = s.roughness as f32;
    d[p::FRY] = s.fry as f32;
    d[p::RING] = 0.0;
    d[p::VIBRATO] = 35.0;
    d[p::VIBRATO_RATE] = s.vibrato_rate as f32;
    d[p::BODY] = 0.0;
    d[p::BODY_FEMALE] = 0.5;
    d[p::BODY_CHILD] = 0.0;
    d[p::JAW] = n.jaw as f32;
    d[p::LIP_OPEN] = n.lip_ap as f32;
    d[p::PROTRUDE] = n.lip_prot as f32;
    d[p::ROUNDING] = (1.0 - n.lip_width) as f32;
    d[p::ROOT] = n.root as f32;
    d[p::TIP_CURL] = n.tt_curl as f32;
    d[p::VELUM] = n.velum as f32;
    d[p::LARYNX] = n.larynx as f32;
    d[p::TWANG] = n.epilarynx as f32;
    d[p::FALSE_FOLDS] = n.ventricular as f32;
    d[p::TONGUE_FWD] = n.tb_x as f32;
    d[p::TONGUE_UP] = n.tb_y as f32;
    d[p::TIP_FWD] = n.tt_x as f32;
    d[p::TIP_UP] = n.tt_y as f32;
    d[p::VOWEL_FRONT] = 0.5;
    d[p::VOWEL_OPEN] = 0.5;
    d[p::WHISPER] = 0.0;
    d[p::PITCH] = s.f0 as f32;
    d[p::MODE] = MODE_SING as f32;
    d[p::BREATHE] = 0.0;
    d[p::RESTART] = 1.0;
    d[p::MPE] = 0.0;
    d[p::LYRICS] = 0.0;
    d[p::START_SYL] = 0.0;
    d[p::SPEECH] = 0.0;
    d[p::START_LINE] = 0.0;
    d[p::SPEECH_RATE] = 1.0;
    d[p::IPA] = 0.0;
    d[p::SPEAK] = 0.0;
    d[p::CONSONANT] = 0.0;
    d[p::PAD_VOWEL] = 0.0;
    d[p::BEAT] = 0.0;
    let (steps, bpm, swing) = preset_steps(0).unwrap_or(([0.0; STEPS], 96.0, 0.0));
    d[p::TEMPO] = bpm;
    d[p::SWING] = swing;
    d[p::MIC] = 1.0;
    d[p::FOLLOW] = 1.0;
    d[p::PLAY_BEAT] = 0.0;
    d[p::STEP_1..p::STEP_1 + STEPS].copy_from_slice(&steps);
    d[p::PAD] = 0.0;
    d
}

// ---------------------------------------------------------------- the C interface

/// Runs `f` on the engine unless it has failed; a panic marks it failed.
fn guarded(h: *mut ChimeraMpc, f: impl FnOnce(&mut ChimeraMpc)) {
    // SAFETY: the plugin passes the pointer cm_new returned, and calls one function at a time
    let Some(e) = (unsafe { h.as_mut() }) else { return };
    if e.failed {
        return;
    }
    if catch_unwind(AssertUnwindSafe(|| f(&mut *e))).is_err() {
        e.failed = true;
    }
}

/// A new engine at this sample rate, or null. `content_dir` (or null) is the plugin's Content
/// folder, with `lyrics.txt`, `speech.txt` and `cmudict.txt.gz`. Allocates and starts the
/// engine's thread: call it outside the audio callback.
#[unsafe(no_mangle)]
pub extern "C" fn cm_new(sample_rate: f64, content_dir: *const c_char) -> *mut ChimeraMpc {
    if !(sample_rate.is_finite() && sample_rate > 8000.0) {
        return std::ptr::null_mut();
    }
    // SAFETY: a NUL-terminated string from the plugin, or null
    let content = (!content_dir.is_null()).then(|| unsafe { CStr::from_ptr(content_dir) }.to_string_lossy().into_owned());
    catch_unwind(AssertUnwindSafe(|| Box::into_raw(Box::new(ChimeraMpc::new(sample_rate, content))))).unwrap_or(std::ptr::null_mut())
}

/// Frees an engine from cm_new (null is fine). Call it outside the audio callback.
#[unsafe(no_mangle)]
pub extern "C" fn cm_free(h: *mut ChimeraMpc) {
    if !h.is_null() {
        // SAFETY: from cm_new, freed once
        let _ = catch_unwind(AssertUnwindSafe(|| drop(unsafe { Box::from_raw(h) })));
    }
}

/// The parameter defaults, in parameter order, into `out` (`n` values at most). Returns how
/// many parameters the engine has.
#[unsafe(no_mangle)]
pub extern "C" fn cm_param_defaults(out: *mut f32, n: i32) -> i32 {
    let d = defaults();
    if !out.is_null() && n > 0 {
        let k = (n as usize).min(p::COUNT);
        // SAFETY: the caller gives room for n values
        unsafe { std::ptr::copy_nonoverlapping(d.as_ptr(), out, k) };
    }
    p::COUNT as i32
}

/// Set every parameter at once (`n` plain values in parameter order); applied at the next render.
#[unsafe(no_mangle)]
pub extern "C" fn cm_set_params(h: *mut ChimeraMpc, values: *const f32, n: i32) {
    if values.is_null() || n <= 0 {
        return;
    }
    // SAFETY: the caller gives n values
    let v = unsafe { std::slice::from_raw_parts(values, (n as usize).min(p::COUNT)) };
    guarded(h, |e| e.params[..v.len()].copy_from_slice(v));
}

/// The host's transport for the block about to render (`samples` frames). `beats` is the song
/// position in quarter notes when `beats_valid`; `bpm` when `bpm_valid`.
#[unsafe(no_mangle)]
pub extern "C" fn cm_transport(h: *mut ChimeraMpc, playing: i32, bpm_valid: i32, bpm: f64, beats_valid: i32, beats: f64, samples: i32) {
    let t = Transport { playing: playing != 0, bpm: (bpm_valid != 0 && bpm.is_finite()).then_some(bpm), beats: (beats_valid != 0 && beats.is_finite()).then_some(beats) };
    guarded(h, |e| e.set_transport(t, samples.max(0) as usize));
}

/// The vowel pad's posture at (front, open), 0..1 each, for the body playing: 9 values into
/// `out`, in the order tongue fwd, tongue up, jaw, lips open, protrude, lip width (1 - rounding),
/// tongue root, larynx, tip curl.
#[unsafe(no_mangle)]
pub extern "C" fn cm_vowel_posture(h: *mut ChimeraMpc, front: f32, open: f32, out: *mut f32) {
    if out.is_null() {
        return;
    }
    guarded(h, |e| {
        let a = e.pads[e.preset()].posture(front as f64, open as f64);
        // SAFETY: the caller gives room for 9 values
        let o = unsafe { std::slice::from_raw_parts_mut(out, 9) };
        for (o, x) in o.iter_mut().zip(a.iter()) {
            *o = *x as f32;
        }
    });
}

/// Beat preset `index` as the 16 step parameter values, its tempo and swing. Returns 0 when there
/// is no such preset.
#[unsafe(no_mangle)]
pub extern "C" fn cm_beat_preset(index: i32, steps16: *mut f32, bpm: *mut f32, swing: *mut f32) -> i32 {
    if steps16.is_null() || bpm.is_null() || swing.is_null() || index < 0 {
        return 0;
    }
    match catch_unwind(|| preset_steps(index as usize)) {
        Ok(Some((s, b, w))) => {
            // SAFETY: the caller gives room for 16 values and one each
            unsafe {
                std::ptr::copy_nonoverlapping(s.as_ptr(), steps16, STEPS);
                *bpm = b;
                *swing = w;
            }
            1
        }
        _ => 0,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn cm_note_on(h: *mut ChimeraMpc, note: i32, velocity: i32) {
    if !(0..128).contains(&note) {
        return;
    }
    if velocity <= 0 {
        return cm_note_off(h, note);
    }
    guarded(h, |e| e.note_on(note as u8, velocity.min(127) as u8));
}

#[unsafe(no_mangle)]
pub extern "C" fn cm_note_off(h: *mut ChimeraMpc, note: i32) {
    if !(0..128).contains(&note) {
        return;
    }
    guarded(h, |e| e.note_off(note as u8));
}

#[unsafe(no_mangle)]
pub extern "C" fn cm_all_notes_off(h: *mut ChimeraMpc) {
    guarded(h, |e| e.stop_notes());
}

/// Pitch bend, -1..1 (the wheel's range is ±2 semitones).
#[unsafe(no_mangle)]
pub extern "C" fn cm_pitch_bend(h: *mut ChimeraMpc, bend: f32) {
    let b = if bend.is_finite() { bend.clamp(-1.0, 1.0) as f64 } else { 0.0 };
    guarded(h, |e| {
        e.wheel = pow(2.0, b * BEND_RANGE / 12.0);
        e.update_bend();
    });
}

/// Render `n` mono samples into `out` (silence once the engine has failed).
#[unsafe(no_mangle)]
pub extern "C" fn cm_render(h: *mut ChimeraMpc, out: *mut f32, n: i32) {
    if out.is_null() || n <= 0 {
        return;
    }
    // SAFETY: the caller gives room for n samples
    let buf = unsafe { std::slice::from_raw_parts_mut(out, n as usize) };
    buf.fill(0.0);
    guarded(h, |e| e.render(buf));
    // SAFETY: as in guarded
    if unsafe { h.as_ref() }.is_some_and(|e| e.failed) {
        buf.fill(0.0);
    }
}

/// 1 once the lyrics and spoken lines are ready (Sing sings "ah" until then, Speak is silent).
#[unsafe(no_mangle)]
pub extern "C" fn cm_lyrics_ready(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(0, |e| (!e.lyrics.is_empty() && e.speech.is_some()) as i32)
}

/// 1 when the voice playing is the one the Body parameters ask for (a blend takes a moment).
#[unsafe(no_mangle)]
pub extern "C" fn cm_body_ready(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(0, |e| match e.wanted_blend() {
        Some(mix) => (e.playing == Playing::Blend(mix)) as i32,
        None => (e.playing == Playing::Preset((e.params[p::BODY].round().max(0.0) as usize).min(2))) as i32,
    })
}

/// 1 once a panic has silenced the engine.
#[unsafe(no_mangle)]
pub extern "C" fn cm_failed(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(1, |e| e.failed as i32)
}
