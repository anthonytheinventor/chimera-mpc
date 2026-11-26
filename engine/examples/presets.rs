//! Render the built-in voice presets (src/CHIMERAMPC.cpp, addProgram) to compare them: each sings
//! the lyrics tune in Sing and holds an "ah" in Play, and prints the loudness and peak so the
//! presets can be level-matched. The engine applies Gain to the voice's level, before its limiter
//! (ceiling 0.75), so a boost turns the voice up into it and never clips. WAVs go to build/listen/presets/.
//!
//!   cargo run --release --example presets
//!
//! PRESETS must match the plugin's addProgram lists (plain values, parameters 0-13).
use chimera_mpc_engine::*;
use std::ffi::CString;

const FS: f64 = 44100.0;

// Gain dB, Breath cmH2O, Pressed %, Bright %, Glide ms, Breathy %, Rough %, Fry %, Ring %,
// Vibrato ct, Vib Rate Hz, Body (0 male 1 female 2 child 3 blend), Female %, Child %
const PRESETS: &[(&str, [f32; 14])] = &[
    ("Init", [0.0, 7.2, 50.0, 25.0, 60.0, 15.0, 10.0, 0.0, 0.0, 35.0, 6.0, 0.0, 50.0, 0.0]),
    ("Natural", [-1.0, 8.0, 45.0, 30.0, 40.0, 12.0, 5.0, 0.0, 0.0, 15.0, 5.5, 0.0, 50.0, 0.0]),
    ("Breathy", [2.0, 6.0, 20.0, 15.0, 80.0, 60.0, 0.0, 0.0, 0.0, 25.0, 5.5, 1.0, 50.0, 0.0]),
    ("Belt", [-3.0, 14.0, 80.0, 65.0, 50.0, 3.0, 15.0, 0.0, 30.0, 25.0, 5.8, 0.0, 50.0, 0.0]),
    ("Opera", [-6.0, 12.0, 60.0, 45.0, 90.0, 5.0, 0.0, 0.0, 80.0, 70.0, 5.8, 0.0, 50.0, 0.0]),
    ("Airy", [3.0, 5.0, 15.0, 10.0, 120.0, 75.0, 0.0, 0.0, 0.0, 20.0, 5.0, 1.0, 50.0, 0.0]),
    ("Rough", [-4.0, 11.0, 70.0, 40.0, 40.0, 10.0, 70.0, 20.0, 0.0, 10.0, 5.5, 0.0, 50.0, 0.0]),
    ("Child", [2.0, 7.0, 45.0, 40.0, 50.0, 20.0, 0.0, 0.0, 0.0, 20.0, 6.0, 2.0, 50.0, 0.0]),
];

const PCT: [usize; 8] = [p::PRESSED, p::BRIGHT, p::BREATHY, p::ROUGH, p::FRY, p::RING, p::BODY_FEMALE, p::BODY_CHILD];

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

struct Rig {
    h: *mut ChimeraMpc,
    params: Vec<f32>,
    out: Vec<f32>,
}

impl Rig {
    fn new(preset: &[f32; 14], mode: f32) -> Rig {
        let dir = CString::new(format!("{}/../Content", env!("CARGO_MANIFEST_DIR"))).unwrap();
        let h = cm_new(FS, dir.as_ptr(), std::ptr::null());
        assert!(!h.is_null());
        let mut params = vec![0.0f32; p::COUNT];
        cm_param_defaults(params.as_mut_ptr(), p::COUNT as i32);
        // as the plugin passes them: percent -> fraction, ms -> s
        for (i, v) in preset.iter().enumerate() {
            params[i] = *v;
        }
        for i in PCT {
            params[i] *= 0.01;
        }
        params[p::GLIDE] *= 0.001;
        params[p::MODE] = mode;
        Rig { h, params, out: Vec::new() }
    }

    fn render(&mut self, seconds: f64) {
        let mut buf = [0.0f32; 128];
        for _ in 0..(seconds * FS / 128.0) as usize {
            cm_set_params(self.h, self.params.as_ptr(), p::COUNT as i32);
            cm_render(self.h, buf.as_mut_ptr(), 128);
            self.out.extend_from_slice(&buf);
        }
    }

    fn settle(&mut self) {
        // the lyrics and the chosen body's voice are made on the engine's own thread
        for _ in 0..400 {
            self.render(0.025);
            if cm_lyrics_ready(self.h) == 1 && cm_body_ready(self.h) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.render(0.3);
        self.out.clear();
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        cm_free(self.h);
    }
}

fn db(x: f64) -> f64 {
    20.0 * x.max(1e-9).log10()
}

/// loudness of the voiced part (blocks within 20 dB of the loudest), dBFS RMS
fn active_rms_db(x: &[f32]) -> f64 {
    let blocks: Vec<f64> = x.chunks(1024).map(|c| (c.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / c.len() as f64).sqrt()).collect();
    let top = blocks.iter().cloned().fold(0.0, f64::max);
    let act: Vec<f64> = blocks.into_iter().filter(|&b| b > top * 0.1).collect();
    db((act.iter().map(|b| b * b).sum::<f64>() / act.len().max(1) as f64).sqrt())
}

/// spectral centroid (Hz) of 4096 samples from the middle of the held note
fn centroid(x: &[f32]) -> f64 {
    let n = 4096;
    let a = x.len() / 2 - n / 2;
    let w: Vec<f64> = (0..n).map(|i| x[a + i] as f64 * (0.5 - 0.5 * (std::f64::consts::TAU * i as f64 / n as f64).cos())).collect();
    let (mut num, mut den) = (0.0, 0.0);
    for k in 1..n / 2 {
        let (mut re, mut im) = (0.0, 0.0);
        let dw = std::f64::consts::TAU * k as f64 / n as f64;
        for (i, v) in w.iter().enumerate() {
            let t = dw * i as f64;
            re += v * t.cos();
            im -= v * t.sin();
        }
        let m = (re * re + im * im).sqrt();
        num += m * k as f64 * FS / n as f64;
        den += m;
    }
    num / den.max(1e-12)
}

fn peak(x: &[f32]) -> f64 {
    x.iter().fold(0.0f64, |m, &v| m.max((v as f64).abs()))
}

fn main() {
    let dir = format!("{}/../build/listen/presets", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
    let only: Option<String> = std::env::args().nth(1);
    let mut all = Vec::new();
    println!("{:<10} {:>9} {:>7} {:>9} {:>7} {:>9}", "preset", "sing dB", "peak", "play dB", "peak", "bright Hz");
    for (k, (name, v)) in PRESETS.iter().enumerate() {
        if only.as_deref().is_some_and(|o| o != *name) {
            continue;
        }
        // a male tune, an octave up for the female and child bodies
        let up = if v[11] >= 1.0 { 12 } else { 0 };
        let mut s = Rig::new(v, 0.0);
        s.settle();
        for note in [57, 59, 60, 62, 64, 62, 60, 59, 57, 55] {
            cm_note_on(s.h, note + up, 100);
            s.render(0.32);
            cm_note_off(s.h, note + up);
            s.render(0.06);
        }
        s.render(0.5);
        let mut pl = Rig::new(v, 1.0);
        pl.settle();
        cm_note_on(pl.h, 52 + up, 100);
        pl.render(1.6);
        cm_note_off(pl.h, 52 + up);
        pl.render(0.4);
        println!("{:<10} {:>9.1} {:>7.2} {:>9.1} {:>7.2} {:>9.0}", name, active_rms_db(&s.out), peak(&s.out), active_rms_db(&pl.out), peak(&pl.out), centroid(&pl.out[..(1.6 * FS) as usize]));
        write_wav(&format!("{dir}/{}_{}_sing.wav", k + 1, name.to_lowercase()), &s.out);
        write_wav(&format!("{dir}/{}_{}_play.wav", k + 1, name.to_lowercase()), &pl.out);
        all.extend_from_slice(&s.out);
        all.extend_from_slice(&pl.out);
        all.extend(std::iter::repeat_n(0.0f32, (0.5 * FS) as usize));
    }
    if only.is_none() {
        write_wav(&format!("{dir}/all_presets.wav"), &all);
    }
}
