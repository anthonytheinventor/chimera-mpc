//! CHIMERA's voice for the MPC: the engine with the settings that fit a 32-bit MPC, behind a
//! C interface the plugin's C++ calls (see `chimera_mpc.h`).
//!
//! Threads: `cm_new` and `cm_free` allocate and free, so the plugin calls them outside the audio
//! callback (`prepare`, the destructor). Everything else runs on the audio thread and allocates
//! nothing: the three body voices are built in `cm_new`, and changing the body swaps one in.
//!
//! Every entry point catches a panic. After one the engine is marked failed and renders silence
//! from then on, so a bug can never take the MPC app down.
//!
//! This first version is Play mode: MIDI notes sing one held posture (legato, the last key held
//! wins, as CHIMERA's own Play mode does), shaped by the voice and articulator parameters.

use chimera::anatomy::human::{ArticKey, ArticPatch, BodyMix, NEUTRAL};
use chimera::airway::network::{APPROX_KINETIC, APPROX_WALLS};
use chimera::engine::{Engine, default_f0};
use chimera::math::midi_to_hz;
use chimera::voice::human_voice::{HumanVoice, SourcePatch, oversample_for};
use std::panic::{AssertUnwindSafe, catch_unwind};

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
    pub const COUNT: usize = 24;
}

/// Pitch bend range each way (semitones), as CHIMERA's plugin.
const BEND_RANGE: f64 = 2.0;
/// Most keys held at once that legato falls back through.
const MAX_KEYS: usize = 16;

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

pub struct ChimeraMpc {
    engine: Engine,
    /// the voices not playing: `spare[i]` holds the voice for BODIES[i], except the playing one's
    /// slot, which holds whatever voice it was swapped with
    spare: [Box<HumanVoice>; 3],
    /// which BODIES entry is playing
    body: usize,
    params: [f32; p::COUNT],
    /// the parameters as last applied (NaN: never)
    applied: [f32; p::COUNT],
    keys: [u8; MAX_KEYS],
    n_keys: usize,
    bend: f64,
    failed: bool,
}

impl ChimeraMpc {
    fn new(fs: f64) -> ChimeraMpc {
        let mut engine = Engine::new(fs);
        let mut spare = [mpc_voice(fs, BODIES[0]), mpc_voice(fs, BODIES[1]), mpc_voice(fs, BODIES[2])];
        // the engine's own voice (CHIMERA's default settings) goes into slot 0, and the MPC male
        // voice plays
        engine.install_voice(&mut spare[0], BODIES[0]);
        ChimeraMpc { engine, spare, body: 0, params: defaults(), applied: [f32::NAN; p::COUNT], keys: [0; MAX_KEYS], n_keys: 0, bend: 1.0, failed: false }
    }

    fn set_body(&mut self, b: usize) {
        if b == self.body || b >= BODIES.len() {
            return;
        }
        // put the playing voice back in its slot, then play slot b's voice
        let (cur, next) = (self.body, b);
        self.engine.install_voice(&mut self.spare[next], BODIES[next]);
        // spare[next] now holds the voice that was playing (body `cur`): move it to slot `cur`,
        // whose occupant (the voice swapped out when `cur` started) goes to slot `next`
        self.spare.swap(cur, next);
        self.body = next;
        // the new voice took over the source and posture; the parameters set the rest again
        self.applied = [f32::NAN; p::COUNT];
    }

    fn apply_params(&mut self) {
        let q = self.params;
        let was = self.applied;
        let changed = |i: usize| q[i].to_bits() != was[i].to_bits();
        // body (blend is not built yet: the nearest preset plays)
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
                // a new voice starts at its body's default pitch: a held key keeps its note
                if self.n_keys > 0 {
                    self.engine.set_source(&SourcePatch::f0(midi_to_hz(self.keys[self.n_keys - 1] as f64)));
                }
            }
        }
        let source = [p::BREATH, p::PRESSED, p::BRIGHT, p::GLIDE, p::BREATHY, p::ROUGH, p::FRY, p::VIBRATO, p::VIBRATO_RATE];
        if source.iter().any(|&i| changed(i)) {
            self.engine.set_source(&SourcePatch {
                pressure: Some(q[p::BREATH] as f64),
                tension: Some(q[p::PRESSED] as f64),
                brightness: Some(q[p::BRIGHT] as f64),
                glide: Some(q[p::GLIDE] as f64),
                breathiness: Some(q[p::BREATHY] as f64),
                roughness: Some(q[p::ROUGH] as f64),
                fry: Some(q[p::FRY] as f64),
                vibrato_depth: Some(q[p::VIBRATO] as f64),
                vibrato_rate: Some(q[p::VIBRATO_RATE] as f64),
                ..Default::default()
            });
        }
        if (p::JAW..=p::FALSE_FOLDS).any(changed) || changed(p::RING) {
            let x = |i: usize| q[i] as f64;
            // Ring and Twang both move the epilarynx (CHIMERA's plugin sets Ring there; Twang is
            // the articulator): the larger one wins
            let epi = x(p::RING).max(x(p::TWANG));
            let a = ArticPatch::default()
                .with(ArticKey::Jaw, x(p::JAW))
                .with(ArticKey::LipAp, x(p::LIP_OPEN))
                .with(ArticKey::LipProt, x(p::PROTRUDE))
                .with(ArticKey::LipWidth, 1.0 - x(p::ROUNDING))
                .with(ArticKey::Root, x(p::ROOT))
                .with(ArticKey::TtCurl, x(p::TIP_CURL))
                .with(ArticKey::Velum, x(p::VELUM))
                .with(ArticKey::Larynx, x(p::LARYNX))
                .with(ArticKey::Epilarynx, epi)
                .with(ArticKey::Ventricular, x(p::FALSE_FOLDS));
            self.engine.set_artic(&a, None);
        }
        self.applied = q;
    }

    fn note_on(&mut self, note: u8) {
        self.remove_key(note);
        if self.n_keys == MAX_KEYS {
            self.keys.copy_within(1.., 0);
            self.n_keys -= 1;
        }
        self.keys[self.n_keys] = note;
        self.n_keys += 1;
        self.engine.set_source(&SourcePatch::f0(midi_to_hz(note as f64)));
        self.engine.set_gate(1.0);
    }

    fn note_off(&mut self, note: u8) {
        self.remove_key(note);
        if self.n_keys > 0 {
            // legato: back to the most recent key still held
            self.engine.set_source(&SourcePatch::f0(midi_to_hz(self.keys[self.n_keys - 1] as f64)));
        } else {
            self.engine.set_gate(0.0);
        }
    }

    fn remove_key(&mut self, note: u8) {
        if let Some(i) = self.keys[..self.n_keys].iter().position(|&k| k == note) {
            self.keys.copy_within(i + 1..self.n_keys, i);
            self.n_keys -= 1;
        }
    }

    fn all_off(&mut self) {
        self.n_keys = 0;
        self.engine.set_gate(0.0);
    }

    fn render(&mut self, out: &mut [f32]) {
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

/// A new engine at this sample rate, or null. Allocates: call it outside the audio callback.
#[unsafe(no_mangle)]
pub extern "C" fn cm_new(sample_rate: f64) -> *mut ChimeraMpc {
    if !(sample_rate.is_finite() && sample_rate > 8000.0) {
        return std::ptr::null_mut();
    }
    catch_unwind(|| Box::into_raw(Box::new(ChimeraMpc::new(sample_rate)))).unwrap_or(std::ptr::null_mut())
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

#[unsafe(no_mangle)]
pub extern "C" fn cm_note_on(h: *mut ChimeraMpc, note: i32, velocity: i32) {
    if !(0..128).contains(&note) {
        return;
    }
    if velocity == 0 {
        return cm_note_off(h, note);
    }
    guarded(h, |e| e.note_on(note as u8));
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

/// 1 once a panic has silenced the engine.
#[unsafe(no_mangle)]
pub extern "C" fn cm_failed(h: *const ChimeraMpc) -> i32 {
    // SAFETY: as in guarded
    unsafe { h.as_ref() }.map_or(1, |e| e.failed as i32)
}
