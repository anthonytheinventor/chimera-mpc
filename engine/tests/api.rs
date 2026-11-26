//! The engine through its C interface, as the plugin drives it: Sing mode advances through the
//! lyrics from Content/, and the vowel pad reaches the vowels it should. Each test writes a WAV
//! to build/listen/ to hear the result.
use chimera_mpc_engine::*;
use std::ffi::CString;

const FS: f64 = 44100.0;

fn content_dir() -> String {
    format!("{}/../Content", env!("CARGO_MANIFEST_DIR"))
}

fn write_wav(name: &str, x: &[f32]) {
    let dir = format!("{}/../build/listen", env!("CARGO_MANIFEST_DIR"));
    std::fs::create_dir_all(&dir).unwrap();
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
    std::fs::write(format!("{dir}/{name}"), b).unwrap();
}

struct Rig {
    h: *mut ChimeraMpc,
    params: Vec<f32>,
    out: Vec<f32>,
}

impl Rig {
    fn new() -> Rig {
        let dir = CString::new(content_dir()).unwrap();
        let h = cm_new(FS, dir.as_ptr());
        assert!(!h.is_null());
        let mut params = vec![0.0f32; p::COUNT];
        cm_param_defaults(params.as_mut_ptr(), p::COUNT as i32);
        Rig { h, params, out: Vec::new() }
    }

    fn set(&mut self, i: usize, v: f32) {
        self.params[i] = v;
    }

    fn render(&mut self, seconds: f64) {
        let mut buf = [0.0f32; 128];
        for _ in 0..(seconds * FS / 128.0) as usize {
            cm_set_params(self.h, self.params.as_ptr(), p::COUNT as i32);
            cm_render(self.h, buf.as_mut_ptr(), 128);
            self.out.extend_from_slice(&buf);
        }
    }

    fn wait_for_lyrics(&mut self) {
        for _ in 0..400 {
            self.render(0.025);
            if cm_lyrics_ready(self.h) == 1 {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        panic!("the lyrics were not ready within 10 s");
    }

    /// the vowel pad, as the plugin applies it: its posture written into the articulators
    fn pad(&mut self, front: f32, open: f32) {
        self.set(p::VOWEL_FRONT, front);
        self.set(p::VOWEL_OPEN, open);
        let mut a = [0.0f32; 9];
        cm_vowel_posture(self.h, front, open, a.as_mut_ptr());
        let keys = [p::TONGUE_FWD, p::TONGUE_UP, p::JAW, p::LIP_OPEN, p::PROTRUDE, p::ROUNDING, p::ROOT, p::LARYNX, p::TIP_CURL];
        for (k, x) in keys.iter().zip(a) {
            self.params[*k] = if *k == p::ROUNDING { 1.0 - x } else { x };
        }
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        cm_free(self.h);
    }
}

fn rms(x: &[f32]) -> f64 {
    (x.iter().map(|&v| (v as f64) * (v as f64)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

#[test]
fn sing_mode_walks_through_the_lyrics() {
    let mut r = Rig::new();
    r.set(p::MODE, 0.0);
    r.set(p::LYRICS, 0.0);
    r.wait_for_lyrics();
    r.out.clear();
    // "Hello there, I am a voice made of air." on a little tune
    for note in [57, 59, 60, 62, 64, 62, 60, 59, 57, 55] {
        cm_note_on(r.h, note, 100);
        r.render(0.32);
        cm_note_off(r.h, note);
        r.render(0.06);
    }
    r.render(0.6);
    assert!(r.out.iter().all(|x| x.is_finite()));
    assert!(rms(&r.out) > 0.01, "too quiet: {}", rms(&r.out));
    assert_eq!(cm_failed(r.h), 0);
    write_wav("sing_slot1.wav", &r.out);
}

#[test]
fn the_vowel_pad_reaches_i_a_and_u() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    let mut postures = vec![];
    for (front, open) in [(1.0, 0.0), (0.45, 1.0), (0.0, 0.0)] {
        r.pad(front, open);
        postures.push((r.params[p::TONGUE_FWD], r.params[p::JAW], r.params[p::ROUNDING]));
        cm_note_on(r.h, 52, 100);
        r.render(0.8);
        cm_note_off(r.h, 52);
        r.render(0.25);
    }
    let (i, a, u) = (postures[0], postures[1], postures[2]);
    println!("tongue fwd / jaw / rounding: i {i:?}, a {a:?}, u {u:?}");
    // /i/ has the tongue forward of /u/, /a/ the most open jaw, /u/ the roundest lips
    assert!(i.0 > u.0, "tongue: i {} u {}", i.0, u.0);
    assert!(a.1 > i.1 && a.1 > u.1, "jaw: i {} a {} u {}", i.1, a.1, u.1);
    assert!(u.2 > i.2, "rounding: i {} u {}", i.2, u.2);
    assert!(r.out.iter().all(|x| x.is_finite()));
    write_wav("play_vowels_i_a_u.wav", &r.out);
}

#[test]
fn a_body_change_while_singing_keeps_going() {
    let mut r = Rig::new();
    r.set(p::MODE, 0.0);
    r.wait_for_lyrics();
    r.out.clear();
    for (k, note) in [60, 62, 64, 65, 67, 69].into_iter().enumerate() {
        r.set(p::BODY, (k % 3) as f32);
        cm_note_on(r.h, note, 100);
        r.render(0.3);
        cm_note_off(r.h, note);
        r.render(0.08);
    }
    assert!(r.out.iter().all(|x| x.is_finite()));
    assert_eq!(cm_failed(r.h), 0);
    write_wav("sing_bodies.wav", &r.out);
}
