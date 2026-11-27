//! Fits the screen's tongue frames: renders speech, singing, consonants, beats, the vowel pad and
//! the Play knobs through the engine, takes the voice's posture every 10 ms, draws each posture's
//! tongue outline with the male body's tract, clusters the outlines (k-means on their principal
//! components) and keeps one real outline per cluster (the one nearest its center). It then fits
//! the linear map from the articulators to the outline's components that the engine uses at run
//! time to pick the nearest frame, and writes it all to src/face_tables.rs.
//!
//!     cargo run --release --example fit_face
//!
//! Deterministic (fixed seeds). Prints how well the run-time pick matches the true nearest frame.
use chimera::anatomy::human::{BODY_MALE, HumanArtic, HumanFrame, HumanVocalTract, NEUTRAL};
use chimera_mpc_engine::face::tongue_features;
use chimera_mpc_engine::*;
use std::ffi::CString;
use std::fmt::Write as _;

const FS: f64 = 44100.0;
const BLOCK: usize = 128;
/// tongue frames
const K: usize = 28;
/// principal components kept
const PCS: usize = 10;
/// the outline's points: the tongue's drawing outline without the two floor points behind the
/// lower teeth (those belong to the jaw); the last point kept is the frenulum (jaw-carried)
const OUTLINE: usize = 51;
const PX_PER_CM: f64 = 27.0;

struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 11) as f64) / ((1u64 << 53) as f64)
    }
    fn range(&mut self, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * self.next()
    }
}

struct Rig {
    h: *mut ChimeraMpc,
    q: Vec<f32>,
    samples: Vec<HumanArtic>,
}

impl Rig {
    fn new() -> Rig {
        let dir = CString::new(format!("{}/../Content", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let h = cm_new(FS, dir.as_ptr(), std::ptr::null());
        let mut q = vec![0.0f32; p::COUNT];
        cm_param_defaults(q.as_mut_ptr(), p::COUNT as i32);
        Rig { h, q, samples: Vec::new() }
    }
    /// render `s` seconds, keeping the posture every 10 ms when `keep`
    fn run(&mut self, s: f64, keep: bool) {
        let mut buf = [0.0f32; BLOCK];
        let every = (0.01 * FS / BLOCK as f64).round().max(1.0) as usize;
        for i in 0..(s * FS / BLOCK as f64) as usize {
            cm_set_params(self.h, self.q.as_ptr(), p::COUNT as i32);
            cm_render(self.h, buf.as_mut_ptr(), BLOCK as i32);
            if keep && i % every == 0 {
                self.samples.push(live_artic(self.h).unwrap());
            }
        }
    }
    fn keep_now(&mut self) {
        self.samples.push(live_artic(self.h).unwrap());
    }
    fn pad(&mut self, front: f32, open: f32) {
        let mut a = [0.0f32; 9];
        cm_vowel_posture(self.h, front, open, a.as_mut_ptr());
        let keys = [p::TONGUE_FWD, p::TONGUE_UP, p::JAW, p::LIP_OPEN, p::PROTRUDE, p::ROUNDING, p::ROOT, p::LARYNX, p::TIP_CURL];
        for (k, x) in keys.iter().zip(a) {
            self.q[*k] = if *k == p::ROUNDING { 1.0 - x } else { x };
        }
    }
}

fn collect() -> (Vec<HumanArtic>, Vec<(&'static str, usize)>) {
    let mut r = Rig::new();
    let mut parts = Vec::new();
    let mark = |r: &Rig, name: &'static str, parts: &mut Vec<(&'static str, usize)>| parts.push((name, r.samples.len()));
    // the dictionary and lyrics load on the engine's thread
    for _ in 0..400 {
        r.run(0.025, false);
        if cm_lyrics_ready(r.h) == 1 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let d = r.q.clone();
    // singing: every lyrics slot, a phrase of notes
    r.q[p::MODE] = 0.0;
    for slot in 0..SLOTS {
        r.q[p::LYRICS] = slot as f32;
        r.run(0.05, false);
        for n in 0..14 {
            let note = 45 + (n * 5) % 19;
            cm_note_on(r.h, note as i32, 100);
            r.run(0.42, true);
            cm_note_off(r.h, note as i32);
            r.run(0.08, true);
        }
    }
    mark(&r, "singing", &mut parts);
    // speech: every block of lines
    r.q = d.clone();
    r.q[p::MODE] = 2.0;
    for slot in 0..SLOTS {
        r.q[p::SPEECH] = slot as f32;
        r.q[p::START_LINE] = 0.0;
        r.run(0.05, false);
        for _ in 0..5 {
            cm_note_on(r.h, 60, 100);
            r.run(2.6, true);
            cm_note_off(r.h, 60);
        }
    }
    mark(&r, "speech", &mut parts);
    // the consonant chart: each consonant between each pad vowel
    for v in 0..4 {
        r.q[p::PAD_VOWEL] = v as f32;
        for c in 1..=25 {
            cm_tap(r.h, p::CONSONANT as i32, c);
            r.run(0.6, true);
        }
    }
    mark(&r, "consonants", &mut parts);
    // beats: every preset pattern, and the pads
    r.q = d.clone();
    r.q[p::MODE] = 3.0;
    r.q[p::FOLLOW] = 0.0;
    for b in 0..7 {
        let (mut steps, mut bpm, mut swing) = ([0.0f32; 16], 0.0f32, 0.0f32);
        if cm_beat_preset(b, steps.as_mut_ptr(), &mut bpm, &mut swing) == 0 {
            continue;
        }
        r.q[p::STEP_1..p::STEP_1 + 16].copy_from_slice(&steps);
        r.q[p::TEMPO] = bpm;
        r.q[p::SWING] = swing;
        r.q[p::BEAT] = b as f32;
        r.q[p::PLAY_BEAT] = 1.0;
        r.run(4.0, true);
        r.q[p::PLAY_BEAT] = 0.0;
        r.run(0.3, false);
    }
    for k in 1..=10 {
        cm_tap(r.h, p::PAD as i32, k);
        r.run(0.4, true);
    }
    mark(&r, "beats", &mut parts);
    // the vowel pad, held and breathing
    r.q = d.clone();
    r.q[p::MODE] = 1.0;
    r.q[p::BREATHE] = 1.0;
    for i in 0..9 {
        for j in 0..9 {
            r.pad(i as f32 / 8.0, j as f32 / 8.0);
            r.run(0.3, false);
            r.run(0.1, true);
        }
    }
    mark(&r, "vowel pad", &mut parts);
    // the Play knobs: each tongue, jaw and larynx knob alone over its range from the defaults
    r.q = d.clone();
    r.q[p::MODE] = 1.0;
    let knobs: [(usize, f32, f32); 8] = [(p::TONGUE_FWD, 1.0, 4.0), (p::TONGUE_UP, 5.0, 8.2), (p::TIP_FWD, 5.0, 7.0), (p::TIP_UP, 5.2, 9.8), (p::ROOT, -0.9, 0.9), (p::TIP_CURL, 0.0, 1.0), (p::JAW, 0.0, 2.4), (p::LARYNX, -1.5, 1.5)];
    for (k, lo, hi) in knobs {
        for s in 0..=16 {
            r.q = d.clone();
            r.q[p::MODE] = 1.0;
            r.q[k] = lo + (hi - lo) * s as f32 / 16.0;
            r.run(0.35, false);
            r.keep_now();
        }
    }
    // and all of them at once, at random
    let mut g = Lcg(0x5eed_f00d);
    for _ in 0..1500 {
        r.q = d.clone();
        r.q[p::MODE] = 1.0;
        r.q[p::TONGUE_FWD] = g.range(1.0, 4.0) as f32;
        r.q[p::TONGUE_UP] = g.range(5.0, 8.2) as f32;
        r.q[p::ROOT] = g.range(-0.9, 0.9) as f32;
        r.q[p::JAW] = g.range(0.0, 2.4) as f32;
        r.q[p::LARYNX] = g.range(-1.5, 1.5) as f32;
        r.q[p::TIP_CURL] = if g.next() < 0.7 { 0.0 } else { g.range(0.0, 1.0) as f32 };
        if g.next() < 0.5 {
            r.q[p::TIP_FWD] = g.range(5.0, 7.0) as f32;
            r.q[p::TIP_UP] = g.range(5.2, 9.8) as f32;
        }
        r.run(0.35, false);
        r.keep_now();
    }
    mark(&r, "play knobs", &mut parts);
    cm_free(r.h);
    (r.samples, parts)
}

fn outline(t: &mut HumanVocalTract, f: &mut HumanFrame, a: &HumanArtic) -> Vec<f64> {
    t.compute(a, f);
    assert_eq!(f.tongue.len(), OUTLINE + 2, "the tongue outline's point count changed");
    f.tongue[..OUTLINE].iter().flat_map(|q| [q.x, q.y]).collect()
}

fn dot(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

fn d2(a: &[f64], b: &[f64]) -> f64 {
    a.iter().zip(b).map(|(x, y)| (x - y) * (x - y)).sum()
}

/// the top `n` principal directions of the rows (power iteration with deflation)
fn pca(rows: &[Vec<f64>], mean: &[f64], n: usize) -> (Vec<Vec<f64>>, Vec<f64>, f64) {
    let dim = mean.len();
    let mut cov = vec![vec![0.0; dim]; dim];
    for r in rows {
        let c: Vec<f64> = r.iter().zip(mean).map(|(x, m)| x - m).collect();
        for i in 0..dim {
            for j in i..dim {
                cov[i][j] += c[i] * c[j];
            }
        }
    }
    for i in 0..dim {
        for j in 0..i {
            cov[i][j] = cov[j][i];
        }
    }
    let total: f64 = (0..dim).map(|i| cov[i][i]).sum();
    let mut dirs: Vec<Vec<f64>> = Vec::new();
    let mut vals = Vec::new();
    for k in 0..n {
        let mut v: Vec<f64> = (0..dim).map(|i| ((i * 7 + k * 13) % 11) as f64 + 1.0).collect();
        let mut lambda = 0.0;
        for _ in 0..500 {
            let mut w: Vec<f64> = cov.iter().map(|row| dot(row, &v)).collect();
            for u in &dirs {
                let pr = dot(&w, u);
                for (wi, ui) in w.iter_mut().zip(u) {
                    *wi -= pr * ui;
                }
            }
            let norm = dot(&w, &w).sqrt();
            if norm == 0.0 {
                break;
            }
            lambda = norm;
            v = w.iter().map(|x| x / norm).collect();
        }
        dirs.push(v);
        vals.push(lambda);
    }
    (dirs, vals, total)
}

/// k-means++ then Lloyd's iterations
fn kmeans(pts: &[Vec<f64>], k: usize, seed: u64) -> Vec<Vec<f64>> {
    let mut g = Lcg(seed);
    let mut centers = vec![pts[(g.next() * pts.len() as f64) as usize % pts.len()].clone()];
    let mut best: Vec<f64> = pts.iter().map(|p| d2(p, &centers[0])).collect();
    while centers.len() < k {
        let sum: f64 = best.iter().sum();
        let mut t = g.next() * sum;
        let mut pick = pts.len() - 1;
        for (i, b) in best.iter().enumerate() {
            t -= b;
            if t <= 0.0 {
                pick = i;
                break;
            }
        }
        centers.push(pts[pick].clone());
        for (b, p) in best.iter_mut().zip(pts) {
            *b = b.min(d2(p, centers.last().unwrap()));
        }
    }
    for _ in 0..200 {
        let mut sums = vec![vec![0.0; pts[0].len()]; k];
        let mut counts = vec![0usize; k];
        for p in pts {
            let c = nearest(&centers, p);
            counts[c] += 1;
            for (s, x) in sums[c].iter_mut().zip(p) {
                *s += x;
            }
        }
        let mut moved = 0.0;
        for c in 0..k {
            if counts[c] == 0 {
                continue;
            }
            let new: Vec<f64> = sums[c].iter().map(|s| s / counts[c] as f64).collect();
            moved += d2(&new, &centers[c]);
            centers[c] = new;
        }
        if moved < 1e-12 {
            break;
        }
    }
    centers
}

fn nearest(centers: &[Vec<f64>], p: &[f64]) -> usize {
    let mut best = (f64::INFINITY, 0);
    for (i, c) in centers.iter().enumerate() {
        let d = d2(c, p);
        if d < best.0 {
            best = (d, i);
        }
    }
    best.1
}

/// least squares: rows x (with a 1 appended) -> y
fn fit_linear(x: &[Vec<f64>], y: &[Vec<f64>]) -> Vec<Vec<f64>> {
    let n = x[0].len() + 1;
    let m = y[0].len();
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![vec![0.0; m]; n];
    for (xr, yr) in x.iter().zip(y) {
        let xa: Vec<f64> = xr.iter().copied().chain([1.0]).collect();
        for i in 0..n {
            for j in 0..n {
                a[i][j] += xa[i] * xa[j];
            }
            for j in 0..m {
                b[i][j] += xa[i] * yr[j];
            }
        }
    }
    for (i, row) in a.iter_mut().enumerate() {
        row[i] += 1e-6 * x.len() as f64;
    }
    // Gauss-Jordan
    for c in 0..n {
        let piv = (c..n).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap()).unwrap();
        a.swap(c, piv);
        b.swap(c, piv);
        let d = a[c][c];
        for j in 0..n {
            a[c][j] /= d;
        }
        for j in 0..m {
            b[c][j] /= d;
        }
        for i in 0..n {
            if i != c {
                let f = a[i][c];
                if f != 0.0 {
                    for j in 0..n {
                        a[i][j] -= f * a[c][j];
                    }
                    for j in 0..m {
                        b[i][j] -= f * b[c][j];
                    }
                }
            }
        }
    }
    // w[out][in]
    (0..m).map(|o| (0..n).map(|i| b[i][o]).collect()).collect()
}

fn main() {
    let t0 = std::time::Instant::now();
    let (samples, parts) = collect();
    println!("fit_face: {} postures in {:.1} s ({:?})", samples.len(), t0.elapsed().as_secs_f64(), parts);
    let mut tract = HumanVocalTract::new(BODY_MALE);
    let mut fr = HumanFrame::with_capacity();
    let outlines: Vec<Vec<f64>> = samples.iter().map(|a| outline(&mut tract, &mut fr, a)).collect();
    let dim = outlines[0].len();
    let mut mean = vec![0.0; dim];
    for o in &outlines {
        for (m, x) in mean.iter_mut().zip(o) {
            *m += x / outlines.len() as f64;
        }
    }
    let (dirs, vals, total) = pca(&outlines, &mean, PCS);
    println!("fit_face: {} components keep {:.3} % of the outlines' variance", PCS, 100.0 * vals.iter().sum::<f64>() / total);
    let coords = |o: &[f64]| -> Vec<f64> {
        let c: Vec<f64> = o.iter().zip(&mean).map(|(x, m)| x - m).collect();
        dirs.iter().map(|u| dot(u, &c)).collect()
    };
    let pc: Vec<Vec<f64>> = outlines.iter().map(|o| coords(o)).collect();
    // thin out the repeats (held postures, silence) before clustering: one posture per 0.5 mm cell
    let mut seen = std::collections::HashSet::new();
    let mut thin = Vec::new();
    for (i, c) in pc.iter().enumerate() {
        let key: Vec<i64> = c.iter().map(|x| (x / 0.05).round() as i64).collect();
        if seen.insert(key) {
            thin.push(i);
        }
    }
    println!("fit_face: {} distinct postures", thin.len());
    let tpts: Vec<Vec<f64>> = thin.iter().map(|&i| pc[i].clone()).collect();
    let centers = kmeans(&tpts, K, 0xc417_3e2a);
    // each frame is the real posture nearest its cluster's center
    let reps: Vec<usize> = centers
        .iter()
        .map(|c| {
            let mut best = (f64::INFINITY, 0);
            for &i in &thin {
                let d = d2(&pc[i], c);
                if d < best.0 {
                    best = (d, i);
                }
            }
            best.1
        })
        .collect();
    let rep_pc: Vec<Vec<f64>> = reps.iter().map(|&i| pc[i].clone()).collect();
    // the run-time map: articulators -> components
    let feats: Vec<Vec<f64>> = samples.iter().map(|a| tongue_features(a).to_vec()).collect();
    let w = fit_linear(&thin.iter().map(|&i| feats[i].clone()).collect::<Vec<_>>(), &thin.iter().map(|&i| pc[i].clone()).collect::<Vec<_>>());
    // how good is the pick?
    let predict = |f: &[f64]| -> Vec<f64> { w.iter().map(|row| dot(&row[..f.len()], f) + row[f.len()]).collect() };
    let rms_px = |a: &[f64], b: &[f64]| (d2(a, b) / (a.len() / 2) as f64).sqrt() * PX_PER_CM;
    let fren = |o: &[f64]| (o[dim - 2], o[dim - 1]);
    let (mut same, mut e_true, mut e_pick, mut fren_err, mut fren_max) = (0usize, 0.0, 0.0, 0.0, 0.0f64);
    let mut worst = Vec::new();
    for &i in &thin {
        let t = nearest(&rep_pc, &pc[i]);
        let pk = nearest(&rep_pc, &predict(&feats[i]));
        same += (t == pk) as usize;
        e_true += rms_px(&outlines[i], &outlines[reps[t]]);
        let ep = rms_px(&outlines[i], &outlines[reps[pk]]);
        e_pick += ep;
        worst.push(ep);
        let (a, b) = (fren(&outlines[i]), fren(&outlines[reps[pk]]));
        let fe = ((a.0 - b.0).powi(2) + (a.1 - b.1).powi(2)).sqrt() * PX_PER_CM;
        fren_err += fe;
        fren_max = fren_max.max(fe);
    }
    worst.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = thin.len() as f64;
    println!(
        "fit_face: run-time pick = true nearest frame for {:.1} % of postures; outline error {:.2} px (nearest frame {:.2} px), 95th percentile {:.2} px; frenulum off by {:.2} px on average, {:.1} px at most",
        100.0 * same as f64 / n,
        e_pick / n,
        e_true / n,
        worst[(worst.len() as f64 * 0.95) as usize],
        fren_err / n,
        fren_max
    );
    // the tables
    let mut s = String::new();
    writeln!(s, "// Generated by examples/fit_face.rs (cargo run --release --example fit_face): do not edit.").unwrap();
    writeln!(s, "// {} tongue frames clustered from {} postures ({} distinct): speech, singing, consonants, beats,", K, samples.len(), thin.len()).unwrap();
    writeln!(s, "// the vowel pad and the Play knobs. TONGUE_MAP turns the articulators (face::tongue_features, then 1)").unwrap();
    writeln!(s, "// into the outline's principal components; TONGUE_CENTERS are the frames' outlines there.").unwrap();
    writeln!(s, "pub const PCS: usize = {PCS};").unwrap();
    writeln!(s, "pub const TONGUE_MAP: [[f32; 10]; PCS] = [").unwrap();
    for row in &w {
        writeln!(s, "    [{}],", row.iter().map(|x| format!("{:.6}", x)).collect::<Vec<_>>().join(", ")).unwrap();
    }
    writeln!(s, "];").unwrap();
    writeln!(s, "pub const TONGUE_CENTERS: [[f32; PCS]; {K}] = [").unwrap();
    for c in &rep_pc {
        writeln!(s, "    [{}],", c.iter().map(|x| format!("{:.5}", x)).collect::<Vec<_>>().join(", ")).unwrap();
    }
    writeln!(s, "];").unwrap();
    writeln!(s, "/// each frame's posture: jaw, tongue body x, y, tip x, y, curl, root, larynx, dorsum").unwrap();
    writeln!(s, "pub const TONGUE_REPS: [[f64; 9]; {K}] = [").unwrap();
    for &i in &reps {
        writeln!(s, "    [{}],", feats[i].iter().map(|x| format!("{:.5}", x)).collect::<Vec<_>>().join(", ")).unwrap();
    }
    writeln!(s, "];").unwrap();
    let path = format!("{}/src/face_tables.rs", env!("CARGO_MANIFEST_DIR"));
    std::fs::write(&path, s).unwrap();
    let neutral_frame = nearest(&rep_pc, &predict(&tongue_features(&NEUTRAL)));
    println!("fit_face: wrote {path} (the neutral posture shows frame {neutral_frame})");
}
