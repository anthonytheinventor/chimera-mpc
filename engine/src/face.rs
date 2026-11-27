//! The face on the screen: which frame of each layer's film strip shows the voice now.
//!
//! The skin draws the head as static art plus independent layers, each a film strip on its own
//! read-only parameter (tools/make_skin.py; build-tmp/chimera-mpc/ui-v2/design.md B.6):
//!
//! | Layer       | Frames                         | Picked by                                        |
//! |-------------|--------------------------------|--------------------------------------------------|
//! | Jaw         | `JAW_FRAMES`, jaw 0..2.4 cm    | the jaw opening                                  |
//! | Tongue      | `TONGUE_FRAMES` real outlines  | the nearest of them (`face_tables.rs`)           |
//! | Upper lip   | `UPPER_HEIGHTS` x `LIP_FRONTS` | its edge's height and how far forward the lips are |
//! | Lower lip   | `LOWER_OPENINGS` x `LIP_FRONTS`| the lips' opening and how far forward; one strip per pair of jaw frames (`LOWER_GATES`), drawn at the pair's middle |
//! | Larynx      | `LARYNX_FRAMES`, -1.5..1.5 cm  | the larynx height                                |
//! | Soft palate | `VELUM_FRAMES`, shut then open | the velum port                                   |
//! | Sound       | `SOUND_FRAMES`, dark to bright | the voice's output level, smoothed               |
//!
//! `examples/dump_face.rs` draws every frame from these same functions, so a frame index means
//! the same posture on both sides. The tongue frames are real outlines clustered offline from
//! speech, singing, beats, the vowel pad and the Play knobs (`examples/fit_face.rs`, which writes
//! `face_tables.rs`); at run time the posture's outline is predicted from the articulators
//! through a fitted linear map and the nearest frame is taken, so nothing here allocates or
//! computes geometry: a few hundred multiply-adds a block.

use chimera::anatomy::human::{HumanArtic, NEUTRAL};

#[path = "face_tables.rs"]
mod tables;

pub const JAW_FRAMES: usize = 10;
pub const JAW_MAX: f64 = 2.4;
pub const LARYNX_FRAMES: usize = 7;
pub const LARYNX_RANGE: f64 = 1.5;
/// The soft palate: frame 0 shut (the port at 0 or below looks the same), then open to 1.2 cm².
pub const VELUM_FRAMES: usize = 8;
pub const VELUM_MAX: f64 = 1.2;
/// The lips (cm). How far forward they reach (protrusion, with a little from spreading: CHIMERA's
/// lip front moves 0.95 cm per cm of protrusion and 0.103 cm from rounded to spread). The upper
/// lip's edge height above CHIMERA's lip line (7.85 cm) is 0.35 x the opening - 0.3 x the jaw, so
/// its frames hold for any jaw. The lower lip hangs on the jaw: its frames are drawn per pair of
/// jaw frames, by the opening (the gap between the edges).
pub const LIP_FRONTS: [f64; 3] = [0.1, 0.6, 1.2];
pub const UPPER_HEIGHTS: [f64; 6] = [-0.75, -0.42, -0.17, 0.06, 0.3, 0.62];
pub const UPPER_FRAMES: usize = UPPER_HEIGHTS.len() * LIP_FRONTS.len();
pub const LOWER_OPENINGS: [f64; 4] = [-0.2, 0.35, 0.95, 1.7];
pub const LOWER_FRAMES: usize = LOWER_OPENINGS.len() * LIP_FRONTS.len();
pub const LOWER_GATES: usize = JAW_FRAMES / 2;
pub const TONGUE_FRAMES: usize = tables::TONGUE_REPS.len();
pub const SOUND_FRAMES: usize = 6;

/// The frame counts in the order `cm_face` reports: jaw, tongue, upper lip, lower lip, larynx, sound.
pub const FRAMES: [usize; 6] = [JAW_FRAMES, TONGUE_FRAMES, UPPER_FRAMES, LOWER_FRAMES, LARYNX_FRAMES, SOUND_FRAMES];

const SPREAD_FRONT: f64 = 0.12 * 0.86;
const PROT_FRONT: f64 = 0.95;

pub fn jaw_value(i: usize) -> f64 {
    JAW_MAX * i as f64 / (JAW_FRAMES - 1) as f64
}

pub fn larynx_value(i: usize) -> f64 {
    -LARYNX_RANGE + 2.0 * LARYNX_RANGE * i as f64 / (LARYNX_FRAMES - 1) as f64
}

pub fn velum_value(i: usize) -> f64 {
    VELUM_MAX * i as f64 / (VELUM_FRAMES - 1) as f64
}

/// How far forward the lips reach (cm) for this protrusion and spreading.
pub fn lip_front(prot: f64, width: f64) -> f64 {
    SPREAD_FRONT * (1.0 - width.clamp(0.0, 1.0)) + PROT_FRONT * prot
}

fn prot_for(front: f64) -> f64 {
    (front - SPREAD_FRONT * (1.0 - NEUTRAL.lip_width)) / PROT_FRONT
}

/// Upper lip frame `i`: (opening, protrusion), drawn with the rest jaw and the neutral spreading.
pub fn upper_value(i: usize) -> (f64, f64) {
    let (h, f) = (i / LIP_FRONTS.len(), i % LIP_FRONTS.len());
    ((UPPER_HEIGHTS[h] + 0.3 * NEUTRAL.jaw) / 0.35, prot_for(LIP_FRONTS[f]))
}

/// Lower lip frame `i`: (opening, protrusion), drawn with the neutral spreading.
pub fn lower_value(i: usize) -> (f64, f64) {
    let (a, f) = (i / LIP_FRONTS.len(), i % LIP_FRONTS.len());
    (LOWER_OPENINGS[a], prot_for(LIP_FRONTS[f]))
}

/// The jaw the lower lip strip `g` is drawn with: the middle of its two jaw frames.
pub fn lower_gate_jaw(g: usize) -> f64 {
    0.5 * (jaw_value(2 * g) + jaw_value((2 * g + 1).min(JAW_FRAMES - 1)))
}

/// The tongue frame `i`'s posture (the articulators that shape the tongue; the rest neutral).
pub fn tongue_value(i: usize) -> HumanArtic {
    let r = &tables::TONGUE_REPS[i];
    let mut a = NEUTRAL;
    a.jaw = r[0];
    a.tb_x = r[1];
    a.tb_y = r[2];
    a.tt_x = r[3];
    a.tt_y = r[4];
    a.tt_curl = r[5];
    a.root = r[6];
    a.larynx = r[7];
    a.dorsum = r[8];
    a
}

/// The articulators the tongue frames are fitted on, in `face_tables.rs` order.
pub fn tongue_features(a: &HumanArtic) -> [f64; 9] {
    [a.jaw, a.tb_x, a.tb_y, a.tt_x, a.tt_y, a.tt_curl, a.root, a.larynx, a.dorsum]
}

fn nearest_step(x: f64, lo: f64, hi: f64, n: usize) -> usize {
    (((x - lo) / (hi - lo)) * (n - 1) as f64).round().clamp(0.0, (n - 1) as f64) as usize
}

pub fn jaw_frame(jaw: f64) -> usize {
    nearest_step(jaw, 0.0, JAW_MAX, JAW_FRAMES)
}

pub fn larynx_frame(larynx: f64) -> usize {
    nearest_step(larynx, -LARYNX_RANGE, LARYNX_RANGE, LARYNX_FRAMES)
}

/// Frame 0 only when the port is shut, so the screen can say "Shut".
pub fn velum_frame(velum: f64) -> usize {
    if velum <= 0.0 { 0 } else { nearest_step(velum, 0.0, VELUM_MAX, VELUM_FRAMES).max(1) }
}

/// The lips' edges as CHIMERA's HumanVocalTract::compute places them (male body, scale 1):
/// (upper x, upper y, lower x, lower y), x from the lips' front at zero protrusion and spreading,
/// y from their middle height; with the lower lip tucked under the upper teeth (/f v/) they move
/// toward the incisor (`incisor`: the upper incisor's edge in the male body's cm).
fn lip_edges(a: &HumanArtic, incisor: (f64, f64)) -> (f64, f64, f64, f64) {
    let lab = a.labiodental.clamp(0.0, 1.0);
    let front = lip_front(a.lip_prot.clamp(-0.2, 1.6), a.lip_width);
    let (mut ux, mut uy) = (front - 0.15, 0.35 * a.lip_ap + 0.35 * lab);
    let (mut lx, mut ly) = (front - 0.25, -0.65 * a.lip_ap);
    if lab > 0.0 {
        let x0 = 0.5 + (8.15 - 0.5) * 0.86;
        let y_mid = 7.85 - 0.3 * a.jaw.clamp(-0.2, 3.0);
        let (ix, iy) = (incisor.0 - x0, incisor.1 - y_mid);
        lx += (ix + 0.05 - lx) * lab;
        ly += (iy - a.lip_ap.max(-0.1) - ly) * lab;
        ux += (ix + 0.55 - ux) * lab * 0.7;
        uy += (iy + 0.45 - uy) * lab * 0.7;
    }
    (ux, uy, lx, ly)
}

/// The upper lip frame nearest its edge.
pub fn upper_frame(a: &HumanArtic, incisor: (f64, f64)) -> usize {
    let (ux, uy, _, _) = lip_edges(a, incisor);
    let h = uy - 0.3 * a.jaw.clamp(-0.2, 3.0);
    let mut best = (f64::INFINITY, 0);
    for (hi, &hc) in UPPER_HEIGHTS.iter().enumerate() {
        for (fi, &fc) in LIP_FRONTS.iter().enumerate() {
            let d = (fc - 0.15 - ux).powi(2) + (hc - h).powi(2);
            if d < best.0 {
                best = (d, hi * LIP_FRONTS.len() + fi);
            }
        }
    }
    best.1
}

/// The lower lip frame nearest its edge (relative to the jaw: the strip shown is the jaw's).
pub fn lower_frame(a: &HumanArtic, incisor: (f64, f64)) -> usize {
    let (_, _, lx, ly) = lip_edges(a, incisor);
    let mut best = (f64::INFINITY, 0);
    for (ai, &ap) in LOWER_OPENINGS.iter().enumerate() {
        for (fi, &fc) in LIP_FRONTS.iter().enumerate() {
            let d = (fc - 0.25 - lx).powi(2) + (-0.65 * ap - ly).powi(2);
            if d < best.0 {
                best = (d, ai * LIP_FRONTS.len() + fi);
            }
        }
    }
    best.1
}

/// The tongue frame nearest this posture: its outline predicted from the articulators, then the
/// nearest frame's outline (both in the fitted principal components). `current` keeps its frame
/// unless another is clearly nearer, so the screen does not flicker between two.
pub fn tongue_frame(a: &HumanArtic, current: usize) -> usize {
    let f = tongue_features(a);
    let mut c = [0.0f32; tables::PCS];
    for (k, row) in tables::TONGUE_MAP.iter().enumerate() {
        let mut s = row[f.len()];
        for (w, x) in row.iter().zip(f.iter()) {
            s += w * *x as f32;
        }
        c[k] = s;
    }
    let dist = |i: usize| -> f32 { tables::TONGUE_CENTERS[i].iter().zip(c.iter()).map(|(a, b)| (a - b) * (a - b)).sum() };
    let mut best = (f32::INFINITY, 0);
    for i in 0..TONGUE_FRAMES {
        let d = dist(i);
        if d < best.0 {
            best = (d, i);
        }
    }
    if current < TONGUE_FRAMES && best.1 != current {
        // the old frame stays while it is within 15 % (in distance) of the best
        let dc = dist(current);
        if dc <= best.0 * 1.3225 + 1e-4 {
            return current;
        }
    }
    best.1
}

/// The sound layer's level steps (dBFS of the smoothed output level): below the first, dark. Play at
/// the defaults is about -18 dB (step 4), a full breath -10 (5), a whisper -28 (2).
const SOUND_STEPS_DB: [f64; SOUND_FRAMES - 1] = [-50.0, -36.0, -27.0, -20.0, -13.0];
const SOUND_HYSTERESIS_DB: f64 = 1.5;
/// The level (power) follows the output with these time constants (s) rising and falling: falling,
/// about 13 dB in 0.25 s, so the fan dims with the voice instead of hanging on.
const SOUND_ATTACK: f64 = 0.02;
const SOUND_RELEASE: f64 = 0.08;

/// The smoothed output level and the frames last reported.
pub struct FaceState {
    pub level: f64,
    pub sound: usize,
    pub tongue: usize,
    /// the output's energy and sample count since the last look
    pub energy: f64,
    pub samples: usize,
}

impl FaceState {
    pub fn new() -> FaceState {
        FaceState { level: 0.0, sound: 0, tongue: usize::MAX, energy: 0.0, samples: 0 }
    }

    /// The sound frame after the output since the last call (`fs` the sample rate).
    pub fn sound_frame(&mut self, fs: f64) -> usize {
        if self.samples > 0 {
            let ms = self.energy / self.samples as f64;
            let t = if ms > self.level { SOUND_ATTACK } else { SOUND_RELEASE };
            let k = 1.0 - (-(self.samples as f64) / (t * fs)).exp();
            self.level += (ms - self.level) * k;
            if !self.level.is_finite() {
                self.level = 0.0;
            }
        }
        self.energy = 0.0;
        self.samples = 0;
        let db = 10.0 * (self.level + 1e-20).log10();
        // the step this level is in, moving off the current one only past the hysteresis
        let mut s = SOUND_STEPS_DB.iter().filter(|&&t| db >= t).count();
        if s > self.sound && db < SOUND_STEPS_DB[s - 1] + SOUND_HYSTERESIS_DB && s == self.sound + 1 {
            s = self.sound;
        } else if s < self.sound && db > SOUND_STEPS_DB[self.sound - 1] - SOUND_HYSTERESIS_DB && s + 1 == self.sound {
            s = self.sound;
        }
        self.sound = s;
        s
    }
}
