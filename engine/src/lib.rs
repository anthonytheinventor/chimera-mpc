//! CHIMERA's voice for the MPC: the engine with the settings that fit a 32-bit MPC, behind a
//! C interface the plugin's C++ calls (see `chimera_mpc.h`).
//!
//! Threads: `cm_new` and `cm_free` allocate and free, so the plugin calls them outside the audio
//! callback (`prepare`, the destructor). Everything else runs on the audio thread and allocates
//! nothing: the three body voices are built in `cm_new` and changing the body swaps one in; the
//! lyrics are turned into syllables on a thread of their own (the dictionary takes a moment to
//! load), and the audio thread takes them over by moving them.
//!
//! Every entry point catches a panic. After one the engine is marked failed and renders silence
//! from then on, so a bug can never take the MPC app down.
//!
//! Modes: Sing (each note sings the next syllable of the chosen lyrics, CHIMERA's Singer) and
//! Play (notes sing one held posture, legato: the last key held sets the pitch). Speak and
//! Beatbox are not built yet and play as Play does.

use chimera::anatomy::human::{ArticKey, ArticPatch, BodyMix, NEUTRAL};
use chimera::airway::network::{APPROX_KINETIC, APPROX_WALLS};
use chimera::control::singer::{Singer, syllables_from};
use chimera::engine::{Engine, default_f0};
use chimera::math::{exp, ln, midi_to_hz};
use chimera::phonetics::english::EnglishG2P;
use chimera::phonetics::planner::vowel_table;
use chimera::phonetics::sing::LiveSyllable;
use chimera::phonetics::syllables::lyrics_to_syllables;
use chimera::phonetics::vowel_data::VOWELS;
use chimera::voice::human_voice::{HumanVoice, SourcePatch, oversample_for};
use std::ffi::{CStr, c_char};
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

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
    pub const RESTART: usize = 34; // 0/1 (not used yet: needs the transport)
    pub const MPE: usize = 35; // 0/1 (not used yet)
    pub const LYRICS: usize = 36; // slot 0..7
    pub const START_SYL: usize = 37; // syllable the lyrics start from
    pub const COUNT: usize = 38;
}

const MODE_SING: i32 = 0;

/// Pitch bend range each way (semitones), as CHIMERA's plugin.
const BEND_RANGE: f64 = 2.0;
/// Most keys held at once that Play's legato falls back through.
const MAX_KEYS: usize = 16;
/// Lyrics slots (lines of Content/lyrics.txt).
pub const SLOTS: usize = 8;
/// What a slot sings before its lyrics are ready, or when it is empty: CHIMERA's default.
const DEFAULT_LYRICS: &str = "Hello there, I am a voice made of air.";

/// A voice with the MPC settings: 1x at 44.1 and 48 kHz, lean trachea and nose sections,
/// the geometry every 4 control steps and every 4 samples, no second geometry pass after a
/// contact correction, the cheaper arithmetic and the single-precision (NEON) airway.
fn mpc_voice(fs: f64, body: BodyMix) -> Box<HumanVoice> {
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

const BODIES: [BodyMix; 3] = [BodyMix::MALE, BodyMix::FEMALE, BodyMix::CHILD];
const BODY_NAMES: [&str; 3] = ["male", "female", "child"];

// ---------------------------------------------------------------- the vowel pad

/// The articulators the vowel pad sets, in the order `cm_vowel_posture` returns them.
const PAD_KEYS: [ArticKey; 9] = [ArticKey::TbX, ArticKey::TbY, ArticKey::Jaw, ArticKey::LipAp, ArticKey::LipProt, ArticKey::LipWidth, ArticKey::Root, ArticKey::Larynx, ArticKey::TtCurl];
/// The vowels the pad leaves out (Mandarin-only, as CHIMERA's own pad).
const NOT_ON_THE_MAP: [&str; 4] = ["ɿ", "ʅ", "u̠", "ʊ̠"];

/// A calibrated vowel of one body: its formants and the pad's articulators.
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
            // keep the four heaviest, heaviest first
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

// ---------------------------------------------------------------- lyrics, off the audio thread

/// What the lyrics thread hands the audio thread: each slot's syllables (the Singer plans them
/// for whichever body is playing).
type Prepared = Vec<Vec<LiveSyllable>>;

#[derive(Default)]
struct Shared {
    ready: Mutex<Option<Prepared>>,
}

/// The lyrics slots of `Content/lyrics.txt`: up to SLOTS non-empty lines, blank and `#` lines
/// skipped; an empty slot sings CHIMERA's default line.
fn read_slots(content: &str) -> Vec<String> {
    let text = std::fs::read_to_string(std::path::Path::new(content).join("lyrics.txt")).unwrap_or_default();
    let mut slots: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).take(SLOTS).map(String::from).collect();
    while slots.len() < SLOTS {
        slots.push(DEFAULT_LYRICS.to_string());
    }
    slots
}

fn load_g2p(content: &str) -> EnglishG2P {
    let mut g = EnglishG2P::new();
    if let Ok(gz) = std::fs::read(std::path::Path::new(content).join("cmudict.txt.gz")) {
        let mut text = String::new();
        if flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut text).is_ok() {
            g.load_dict(&text);
        }
    }
    // without the dictionary, letter-to-sound rules
    g
}

fn prepare_lyrics(content: String, shared: Arc<Shared>) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let g2p = load_g2p(&content);
        let slots = read_slots(&content);
        let prepared: Prepared = slots
            .iter()
            .map(|text| {
                let syl = lyrics_to_syllables(text, &g2p);
                syllables_from(&syl)
            })
            .collect();
        if let Ok(mut r) = shared.ready.lock() {
            *r = Some(prepared);
        }
    }));
}

// ---------------------------------------------------------------- the engine

pub struct ChimeraMpc {
    engine: Engine,
    /// the voices not playing: `spare[i]` holds the voice for BODIES[i], except the playing one's
    /// slot, which holds whatever voice it was swapped with
    spare: [Box<HumanVoice>; 3],
    /// which BODIES entry is playing
    body: usize,
    pads: [Pad; 3],
    singer: Singer,
    /// each slot's syllables once the lyrics thread is done (moved in, never freed here)
    slots: Prepared,
    /// the slot the singer holds (its Vec is in the singer, the slot's entry is empty), or none
    sung_slot: Option<usize>,
    /// the Singer's own first lyrics ("ah"), kept here once replaced: dropping them would free
    /// on the audio thread
    first_lyrics: Vec<LiveSyllable>,
    shared: Arc<Shared>,
    params: [f32; p::COUNT],
    /// the parameters as last applied (NaN: never)
    applied: [f32; p::COUNT],
    mode: i32,
    keys: [u8; MAX_KEYS],
    n_keys: usize,
    bend: f64,
    failed: bool,
}

impl ChimeraMpc {
    fn new(fs: f64, content: Option<String>) -> ChimeraMpc {
        let mut engine = Engine::new(fs);
        let mut spare = [mpc_voice(fs, BODIES[0]), mpc_voice(fs, BODIES[1]), mpc_voice(fs, BODIES[2])];
        // the engine's own voice (CHIMERA's default settings) goes into slot 0, and the MPC male
        // voice plays
        engine.install_voice(&mut spare[0], BODIES[0]);
        let shared = Arc::new(Shared::default());
        if let Some(dir) = content {
            let s = shared.clone();
            let _ = std::thread::Builder::new().name("chimera-lyrics".into()).spawn(move || prepare_lyrics(dir, s));
        }
        ChimeraMpc {
            engine,
            spare,
            body: 0,
            pads: [pad_for(0), pad_for(1), pad_for(2)],
            singer: Singer::new(),
            slots: Vec::new(),
            sung_slot: None,
            first_lyrics: Vec::new(),
            shared,
            params: defaults(),
            applied: [f32::NAN; p::COUNT],
            mode: MODE_SING,
            keys: [0; MAX_KEYS],
            n_keys: 0,
            bend: 1.0,
            failed: false,
        }
    }

    fn set_body(&mut self, b: usize) {
        if b == self.body || b >= BODIES.len() {
            return;
        }
        let (cur, next) = (self.body, b);
        self.singer.stop_all(&mut self.engine);
        self.engine.install_voice(&mut self.spare[next], BODIES[next]);
        // spare[next] now holds the voice that was playing (body `cur`): move it to slot `cur`,
        // whose occupant (the voice swapped out when `cur` started) goes to slot `next`
        self.spare.swap(cur, next);
        self.body = next;
        self.singer.body = BODIES[next];
        // the new voice took over the source and posture; the parameters set the rest again
        // (the lyrics stay: the singer keeps its syllables)
        let lyrics = self.applied[p::LYRICS];
        self.applied = [f32::NAN; p::COUNT];
        self.applied[p::LYRICS] = lyrics;
    }

    /// Take the lyrics thread's syllables when they are ready (try_lock: never waits).
    fn intake(&mut self) {
        if !self.slots.is_empty() {
            return;
        }
        let Ok(mut r) = self.shared.ready.try_lock() else { return };
        if let Some(prepared) = r.take() {
            // a move: the Mutex is left holding None
            self.slots = prepared;
            self.sung_slot = None;
            self.applied[p::LYRICS] = f32::NAN;
        }
    }

    /// The singer sings this slot: its syllables are moved in, the previous slot's moved back.
    fn sing_slot(&mut self, slot: usize) {
        if slot >= self.slots.len() || self.sung_slot == Some(slot) {
            return;
        }
        let incoming = core::mem::take(&mut self.slots[slot]);
        if incoming.is_empty() {
            return;
        }
        let outgoing = self.singer.set_syllables(incoming);
        match self.sung_slot {
            Some(prev) => self.slots[prev] = outgoing,
            None => self.first_lyrics = outgoing,
        }
        self.sung_slot = Some(slot);
    }

    fn apply_params(&mut self) {
        let q = self.params;
        let was = self.applied;
        let changed = |i: usize| q[i].to_bits() != was[i].to_bits();
        if changed(p::BODY) || changed(p::BODY_FEMALE) || changed(p::BODY_CHILD) {
            let b = match q[p::BODY].round() as i32 {
                1 => 1,
                2 => 2,
                3 => {
                    let mix = BodyMix::new(q[p::BODY_FEMALE] as f64, q[p::BODY_CHILD] as f64);
                    BODIES.iter().position(|x| x.preset() == Some(mix.nearest())).unwrap_or(0)
                }
                _ => 0,
            };
            if b != self.body {
                self.set_body(b);
                if self.n_keys > 0 && self.mode != MODE_SING {
                    self.engine.set_source(&SourcePatch::f0(midi_to_hz(self.keys[self.n_keys - 1] as f64)));
                }
            }
        }
        let was = self.applied;
        let changed = |i: usize| q[i].to_bits() != was[i].to_bits();
        if changed(p::MODE) {
            let m = q[p::MODE].round() as i32;
            if m != self.mode {
                // a mode change ends whatever was sounding
                self.singer.stop_all(&mut self.engine);
                self.n_keys = 0;
                self.mode = m;
            }
        }
        let source = [p::BREATH, p::PRESSED, p::BRIGHT, p::GLIDE, p::BREATHY, p::ROUGH, p::FRY, p::VIBRATO, p::VIBRATO_RATE, p::WHISPER, p::MODE];
        if source.iter().any(|&i| changed(i)) {
            // while singing, the notes' own gestures carry the vibrato (as CHIMERA's plugin)
            let depth = if self.mode == MODE_SING { 0.0 } else { q[p::VIBRATO] as f64 };
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
        if changed(p::PITCH) && !was[p::PITCH].is_nan() && self.mode != MODE_SING {
            // the Pitch control sets the pitch, as a key does (the last one moved wins)
            self.engine.set_source(&SourcePatch::f0(q[p::PITCH] as f64));
        }
        if changed(p::LYRICS) {
            self.sing_slot((q[p::LYRICS].round().max(0.0) as usize).min(SLOTS - 1));
        }
        if changed(p::START_SYL) || changed(p::LYRICS) {
            self.singer.set_cursor(q[p::START_SYL].round().max(0.0) as usize);
        }
        if changed(p::BREATHE) || changed(p::MODE) {
            self.update_gate();
        }
        self.applied = q;
    }

    fn update_gate(&mut self) {
        if self.mode == MODE_SING {
            return; // the singer breathes with its notes
        }
        let on = self.n_keys > 0 || self.params[p::BREATHE] >= 0.5;
        self.engine.set_gate(if on { 1.0 } else { 0.0 });
    }

    fn note_on(&mut self, note: u8, velocity: u8) {
        self.apply_params();
        if self.mode == MODE_SING {
            self.singer.note_on(&mut self.engine, note, velocity as f64);
            return;
        }
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

    fn note_off(&mut self, note: u8) {
        if self.mode == MODE_SING {
            self.singer.note_off(&mut self.engine, note);
            return;
        }
        self.remove_key(note);
        if self.n_keys > 0 {
            // legato: back to the most recent key still held
            self.engine.set_source(&SourcePatch::f0(midi_to_hz(self.keys[self.n_keys - 1] as f64)));
        }
        self.update_gate();
    }

    fn remove_key(&mut self, note: u8) {
        if let Some(i) = self.keys[..self.n_keys].iter().position(|&k| k == note) {
            self.keys.copy_within(i + 1..self.n_keys, i);
            self.n_keys -= 1;
        }
    }

    fn all_off(&mut self) {
        self.singer.stop_all(&mut self.engine);
        self.n_keys = 0;
        self.update_gate();
    }

    fn render(&mut self, out: &mut [f32]) {
        self.intake();
        self.apply_params();
        self.engine.set_bend(self.bend);
        self.engine.render(out);
        for x in out.iter_mut() {
            if !x.is_finite() {
                *x = 0.0;
            }
        }
    }
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
/// folder, with `lyrics.txt` and `cmudict.txt.gz`. Allocates and starts the lyrics thread: call it
/// outside the audio callback.
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

/// The vowel pad's posture at (front, open), 0..1 each, for the body playing: 9 values into
/// `out`, in the order tongue fwd, tongue up, jaw, lips open, protrude, lip width (1 - rounding),
/// tongue root, larynx, tip curl.
#[unsafe(no_mangle)]
pub extern "C" fn cm_vowel_posture(h: *mut ChimeraMpc, front: f32, open: f32, out: *mut f32) {
    if out.is_null() {
        return;
    }
    guarded(h, |e| {
        let a = e.pads[e.body].posture(front as f64, open as f64);
        // SAFETY: the caller gives room for 9 values
        let o = unsafe { std::slice::from_raw_parts_mut(out, 9) };
        for (o, x) in o.iter_mut().zip(a.iter()) {
            *o = *x as f32;
        }
    });
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
    guarded(h, |e| e.all_off());
}

/// Pitch bend, -1..1 (the wheel's range is ±2 semitones).
#[unsafe(no_mangle)]
pub extern "C" fn cm_pitch_bend(h: *mut ChimeraMpc, bend: f32) {
    let b = if bend.is_finite() { bend.clamp(-1.0, 1.0) as f64 } else { 0.0 };
    guarded(h, |e| e.bend = chimera::math::pow(2.0, b * BEND_RANGE / 12.0));
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

/// 1 once the lyrics are ready (Sing sings CHIMERA's default "ah" until then).
#[unsafe(no_mangle)]
pub extern "C" fn cm_lyrics_ready(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(0, |e| (!e.slots.is_empty()) as i32)
}

/// 1 once a panic has silenced the engine.
#[unsafe(no_mangle)]
pub extern "C" fn cm_failed(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(1, |e| e.failed as i32)
}
