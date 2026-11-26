//! How much aliasing the MPC's voice has: a steady sung note holds energy only at its harmonics
//! (k * f0); a harmonic above half the rate the voice computes at folds back to a frequency that
//! can be worked out, and with f0 chosen so fs / f0 has a fractional part of 1/3, every folded
//! line falls a third of the way between two harmonics. The power at those lines, measured with a
//! long FFT, is the aliasing. The same voice at 4x oversampling (its decimator filters before the
//! output rate) is the reference: what it has at those frequencies is the noise floor, so the
//! difference is aliasing the MPC build adds.
//!
//!   cargo run --release --example alias
//!
//! The voice is set up as the plugin's (engine/src/lib.rs mpc_voice), with no vibrato, drift,
//! breathiness, roughness, jitter or shimmer, so the harmonics stay sharp lines.
use chimera::airway::network::{APPROX_KINETIC, APPROX_WALLS};
use chimera::anatomy::human::BodyMix;
use chimera::voice::human_voice::{HumanVoice, SourcePatch};

const FS: f64 = 44100.0;
const N: usize = 1 << 17; // FFT length: 0.34 Hz bins

fn voice(body: BodyMix, os: usize, f0: f64) -> Box<HumanVoice> {
    let mut v = Box::new(HumanVoice::with_sections(FS, body.body(), os, 2.0, 1.4));
    v.set_anat_every(4);
    v.set_geo_every(4);
    v.set_contact_reapply(false);
    v.airway.net.approx = APPROX_WALLS | APPROX_KINETIC;
    v.set_fast_airway(true);
    v.set_source(&SourcePatch {
        f0: Some(f0),
        vibrato_depth: Some(0.0),
        drift: Some(0.0),
        breathiness: Some(0.0),
        roughness: Some(0.0),
        fry: Some(0.0),
        voiced: Some(true),
        ..Default::default()
    });
    v.glottis.params.jitter = 0.0;
    v.glottis.params.shimmer = 0.0;
    v.set_gate(1.0);
    v
}

fn write_wav(path: &str, x: &[f32]) {
    let mut b = Vec::with_capacity(44 + x.len() * 4);
    let data = (x.len() * 4) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&3u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&(FS as u32).to_le_bytes());
    b.extend_from_slice(&((FS as u32) * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&32u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for v in x {
        b.extend_from_slice(&v.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

fn render(v: &mut HumanVoice, seconds: f64) -> Vec<f32> {
    let mut out = vec![0.0f32; (seconds * FS) as usize];
    for c in out.chunks_mut(128) {
        v.render(c);
    }
    out
}

/// in-place radix-2 FFT
fn fft(re: &mut [f64], im: &mut [f64]) {
    let n = re.len();
    let mut j = 0;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let a = -std::f64::consts::TAU / len as f64;
        let (wr, wi) = (a.cos(), a.sin());
        for s in (0..n).step_by(len) {
            let (mut cr, mut ci) = (1.0, 0.0);
            for k in 0..len / 2 {
                let (ur, ui) = (re[s + k], im[s + k]);
                let (xr, xi) = (re[s + k + len / 2], im[s + k + len / 2]);
                let (vr, vi) = (xr * cr - xi * ci, xr * ci + xi * cr);
                re[s + k] = ur + vr;
                im[s + k] = ui + vi;
                re[s + k + len / 2] = ur - vr;
                im[s + k + len / 2] = ui - vi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
        }
        len <<= 1;
    }
}

/// power spectrum (Blackman-Harris window), bins 0..N/2
fn spectrum(x: &[f32]) -> Vec<f64> {
    let mut re = vec![0.0; N];
    let mut im = vec![0.0; N];
    let a = x.len() - N;
    for i in 0..N {
        let t = std::f64::consts::TAU * i as f64 / N as f64;
        let w = 0.35875 - 0.48829 * t.cos() + 0.14128 * (2.0 * t).cos() - 0.01168 * (3.0 * t).cos();
        re[i] = x[a + i] as f64 * w;
    }
    fft(&mut re, &mut im);
    (0..N / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}

const BIN: f64 = FS / N as f64;

/// the power of the line nearest `f` (the peak within +-4 bins, with its neighbours: the window's
/// main lobe)
fn line(p: &[f64], f: f64) -> f64 {
    let c = (f / BIN).round() as isize;
    let mut best = c;
    for k in c - 4..=c + 4 {
        if k > 2 && (k as usize) < p.len() - 3 && p[k as usize] > p[best.clamp(3, p.len() as isize - 4) as usize] {
            best = k;
        }
    }
    let b = best.clamp(3, p.len() as isize - 4) as usize;
    p[b - 2..=b + 2].iter().sum()
}

/// f0 from the spectrum: harmonics 1..8, each peak refined, averaged as f / k
fn measure_f0(p: &[f64], guess: f64) -> f64 {
    let mut f0 = guess;
    for _ in 0..2 {
        let mut acc = 0.0;
        let mut n = 0.0;
        for k in 1..=8 {
            let c = (k as f64 * f0 / BIN).round() as usize;
            let lo = c.saturating_sub(6).max(1);
            let (mut b, mut m) = (c, 0.0);
            for i in lo..=(c + 6).min(p.len() - 2) {
                if p[i] > m {
                    m = p[i];
                    b = i;
                }
            }
            // parabolic peak on log power
            let (y0, y1, y2) = (p[b - 1].max(1e-30).ln(), p[b].max(1e-30).ln(), p[b + 1].max(1e-30).ln());
            let d = 0.5 * (y0 - y2) / (y0 - 2.0 * y1 + y2);
            acc += (b as f64 + if d.is_finite() { d } else { 0.0 }) * BIN / k as f64;
            n += 1.0;
        }
        f0 = acc / n;
    }
    f0
}

/// f0 near `target` with fs / f0 = n + 1/3: every folded line sits a third of the way between
/// two harmonics
fn third_pitch(target: f64) -> f64 {
    let n = (FS / target - 1.0 / 3.0).round();
    FS / (n + 1.0 / 3.0)
}

const BANDS: [(f64, f64); 4] = [(0.0, 2000.0), (2000.0, 5000.0), (5000.0, 10000.0), (10000.0, 20000.0)];

struct Result {
    /// per band: folded lines' power over the harmonics' power there, dB
    band_db: [f64; 4],
    alias_db: f64,      // all folded lines, relative to the whole signal
    worst_db: f64,      // the strongest folded line, relative to the strongest harmonic
    worst_hz: f64,
}

fn measure(p: &[f64], f0: f64, os: usize) -> Result {
    let total: f64 = p.iter().sum();
    let strongest = (1..=40).map(|k| line(p, k as f64 * f0)).fold(0.0, f64::max);
    let rate = FS * os as f64;
    let mut used = std::collections::HashSet::new();
    let (mut sum, mut sum5, mut worst, mut worst_hz) = (0.0, 0.0, 0.0, 0.0);
    let mut band_a = [0.0f64; 4];
    let mut band_h = [0.0f64; 4];
    let band = |f: f64| BANDS.iter().position(|(a, b)| f >= *a && f < *b);
    let mut k = 1;
    while k as f64 * f0 < FS / 2.0 - 50.0 {
        if let Some(i) = band(k as f64 * f0) {
            band_h[i] += line(p, k as f64 * f0);
        }
        k += 1;
    }
    // the harmonics above the output's Nyquist, up to three times the rate the voice computes at
    let k0 = (FS / 2.0 / f0).ceil() as usize;
    let k1 = (3.0 * rate / f0) as usize;
    for k in k0..=k1 {
        let mut f = (k as f64 * f0) % rate;
        if f > rate / 2.0 {
            f = rate - f;
        }
        if f >= FS / 2.0 - 50.0 || f < 30.0 {
            continue; // above the output band (os > 1: removed by the decimator) or at DC
        }
        // must sit clear of the harmonics
        let off = (f / f0).fract();
        if (off - 0.5).abs() > 0.4 {
            continue;
        }
        let b = (f / BIN).round() as i64;
        if !used.insert(b / 3) {
            continue;
        }
        let e = line(p, f);
        sum += e;
        if f < 5000.0 {
            sum5 += e;
        }
        if let Some(i) = band(f) {
            band_a[i] += e;
        }
        if e > worst {
            worst = e;
            worst_hz = f;
        }
    }
    let db = |x: f64, r: f64| 10.0 * (x.max(1e-30) / r).log10();
    let band_db = [0, 1, 2, 3].map(|i| db(band_a[i], band_h[i].max(1e-30)));
    let _ = sum5;
    Result { band_db, alias_db: db(sum, total), worst_db: db(worst, strongest), worst_hz }
}

fn main() {
    let cases: [(&str, BodyMix, [f64; 3]); 3] = [
        ("male", BodyMix::MALE, [98.0, 196.0, 392.0]),
        ("female", BodyMix::FEMALE, [196.0, 392.0, 784.0]),
        ("child", BodyMix::CHILD, [262.0, 523.0, 1047.0]),
    ];
    println!("Aliasing in a steady note. MPC = the plugin (no oversampling); 2x = CHIMERA's own plugin's");
    println!("setting at 44.1 kHz; 4x = the reference (what it shows is the measurement's noise floor).");
    println!("all: power at every in-between frequency where folded harmonics land / the whole signal.");
    println!("bands (MPC): folded power / the voice's own harmonics in that band (0 dB = as loud as the voice there).
");
    println!("{:<7} {:>7} | {:>7} {:>7} {:>7} | {:>8} {:>8} | {:>7} {:>7} {:>7} {:>7} | {:>5}", "body", "f0 Hz", "MPC", "2x", "4x", "worst", "at Hz", "0-2k", "2-5k", "5-10k", "10-20k", "peak");
    let dir = format!("{}/../build/listen/alias", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    // the lowest note of each body, rendered three ways one after another: MPC, 2x, 4x
    let mut listen = Vec::new();
    for (name, body, pitches) in cases {
        for target in pitches {
            let f = third_pitch(target);
            let mut rows = Vec::new();
            for os in [1usize, 2, 4] {
                let mut v = voice(body, os, f);
                render(&mut v, 1.0); // settle
                let x = render(&mut v, N as f64 / FS + 0.1);
                let peak = x.iter().fold(0.0f32, |m, s| m.max(s.abs()));
                if target == pitches[0] {
                    listen.extend_from_slice(&x[..(2.5 * FS) as usize]);
                    listen.extend(std::iter::repeat_n(0.0f32, (0.4 * FS) as usize));
                }
                let p = spectrum(&x);
                let f0 = measure_f0(&p, f);
                rows.push((measure(&p, f0, 1), f0, peak));
            }
            let (a, f0, peak) = &rows[0];
            println!(
                "{:<7} {:>7.1} | {:>7.1} {:>7.1} {:>7.1} | {:>8.1} {:>8.0} | {:>7.1} {:>7.1} {:>7.1} {:>7.1} | {:>5.2}",
                name, f0, a.alias_db, rows[1].0.alias_db, rows[2].0.alias_db, a.worst_db, a.worst_hz,
                a.band_db[0], a.band_db[1], a.band_db[2], a.band_db[3], peak
            );
            let r = &rows[2].0;
            println!(
                "{:<7} {:>7} | {:>7} {:>7} {:>7} | {:>8} {:>8} | {:>7.1} {:>7.1} {:>7.1} {:>7.1} |   (4x floor)",
                "", "", "", "", "", "", "", r.band_db[0], r.band_db[1], r.band_db[2], r.band_db[3]
            );
        }
    }
    write_wav(&format!("{dir}/alias_compare.wav"), &listen);
    println!("
build/listen/alias/alias_compare.wav: male 98 Hz, female 196 Hz, child 262 Hz; each as MPC, 2x, 4x (2.5 s each)");
}
