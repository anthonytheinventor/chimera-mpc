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
        let h = cm_new(FS, dir.as_ptr(), std::ptr::null());
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

#[test]
fn speak_mode_says_lines_at_the_keys_pitch() {
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.set(p::SPEECH, 1.0); // "I am a voice made of air." / "Every sound comes from a simulated airway."
    r.wait_for_lyrics();
    r.out.clear();
    for note in [60, 67] {
        cm_note_on(r.h, note, 100);
        r.render(0.1);
        cm_note_off(r.h, note);
        r.render(2.4);
    }
    assert!(r.out.iter().all(|x| x.is_finite()));
    let half = r.out.len() / 2;
    assert!(rms(&r.out[..half]) > 0.005 && rms(&r.out[half..]) > 0.005, "a line was silent: {} / {}", rms(&r.out[..half]), rms(&r.out[half..]));
    write_wav("speak_slot2.wav", &r.out);
}

#[test]
fn beatbox_plays_its_pattern_pads_and_a_hum() {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.wait_for_lyrics();
    r.out.clear();
    // the default pattern (Boots and cats) on its own tempo for two bars
    r.set(p::PLAY_BEAT, 1.0);
    r.render(5.0);
    let pattern = rms(&r.out);
    r.set(p::PLAY_BEAT, 0.0);
    r.render(0.5);
    // drum keys, then a hummed note
    for note in [36, 42, 38, 42, 36, 36, 38, 46] {
        cm_note_on(r.h, note, 110);
        r.render(0.25);
        cm_note_off(r.h, note);
    }
    cm_note_on(r.h, 69, 100);
    r.render(1.2);
    cm_note_off(r.h, 69);
    r.render(0.5);
    assert!(r.out.iter().all(|x| x.is_finite()));
    assert!(pattern > 0.005, "the pattern was silent: {pattern}");
    assert_eq!(cm_failed(r.h), 0);
    write_wav("beatbox.wav", &r.out);
}

#[test]
fn a_blend_plays_once_its_voice_is_built() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.set(p::BODY, 3.0);
    r.set(p::BODY_FEMALE, 0.5);
    r.set(p::BODY_CHILD, 0.3);
    let mut ready = false;
    for _ in 0..400 {
        r.render(0.025);
        if cm_body_ready(r.h) == 1 {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    assert!(ready, "the blended voice never arrived");
    r.out.clear();
    cm_note_on(r.h, 57, 100);
    r.render(1.0);
    cm_note_off(r.h, 57);
    r.render(0.3);
    // and back to a preset at once
    r.set(p::BODY, 0.0);
    r.render(0.05);
    assert_eq!(cm_body_ready(r.h), 1);
    assert!(r.out.iter().all(|x| x.is_finite()) && rms(&r.out) > 0.005);
    write_wav("play_blend.wav", &r.out);
}

#[test]
fn restart_starts_the_lyrics_over_when_the_transport_starts() {
    let mut r = Rig::new();
    r.set(p::MODE, 0.0);
    r.wait_for_lyrics();
    let sing = |r: &mut Rig, n: usize| {
        for _ in 0..n {
            cm_note_on(r.h, 60, 100);
            r.render(0.2);
            cm_note_off(r.h, 60);
            r.render(0.05);
        }
    };
    // three syllables in, then the transport starts: the next note is the first syllable again
    sing(&mut r, 3);
    let mark = r.out.len();
    cm_transport(r.h, 1, 1, 120.0, 1, 0.0, 128);
    sing(&mut r, 1);
    let restarted = r.out[mark..].to_vec();
    // a fresh engine's first note, for comparison
    let mut f = Rig::new();
    f.set(p::MODE, 0.0);
    f.wait_for_lyrics();
    let fresh_mark = f.out.len();
    sing(&mut f, 1);
    let fresh = f.out[fresh_mark..].to_vec();
    // the same syllable: their spectra are close (both "Hel-"), measured as the level within 3 dB
    let (a, b) = (rms(&restarted), rms(&fresh));
    println!("restarted {a:.4}, fresh {b:.4}");
    assert!((20.0 * (a / b).log10()).abs() < 3.0);
}

/// The pitch (Hz) of a steady stretch: the autocorrelation's highest peak between 60 and 600 Hz,
/// looked for only after it first dips below zero (else the shortest lag always wins).
fn pitch_of(x: &[f32]) -> f64 {
    let (lo, hi) = ((FS / 600.0) as usize, (FS / 60.0) as usize);
    let n = x.len() - hi;
    let ac = |lag: usize| -> f64 { (0..n).map(|i| x[i] as f64 * x[i + lag] as f64).sum() };
    let mut lag = 1;
    while lag < hi && ac(lag) > 0.0 {
        lag += 1;
    }
    let mut best = (f64::MIN, lo);
    for l in lag.max(lo)..hi {
        let c = ac(l);
        if c > best.0 {
            best = (c, l);
        }
    }
    FS / best.1 as f64
}

#[test]
fn the_consonant_chart_says_a_consonant_between_two_vowels() {
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.wait_for_lyrics();
    r.out.clear();
    for (c, v) in [(17.0, 0.0), (3.0, 1.0), (19.0, 2.0)] {
        // s between ah, t between ee, sh between oo
        r.set(p::PAD_VOWEL, v);
        r.set(p::CONSONANT, c);
        r.render(0.9);
        r.set(p::CONSONANT, 0.0);
        r.render(0.05);
    }
    assert!(r.out.iter().all(|x| x.is_finite()) && rms(&r.out) > 0.005, "silent: {}", rms(&r.out));
    write_wav("speak_consonants.wav", &r.out);
}

#[test]
fn mpe_bends_a_note_on_its_own_channel_and_pressure_breathes_harder() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.set(p::VIBRATO, 0.0);
    r.set(p::MPE, 1.0);
    r.render(0.05); // the engine takes the parameters at its next render
    // a note on channel 3, then that channel bent up 12 semitones (of 48)
    cm_midi(r.h, 0x92, 57, 100);
    r.render(0.8);
    let a = r.out.len();
    r.render(0.4);
    let plain = pitch_of(&r.out[a..]);
    let up = (8192.0 + 8191.0 * 12.0 / 48.0) as u32;
    cm_midi(r.h, 0xE2, (up & 127) as i32, (up >> 7) as i32);
    r.render(0.8);
    let b = r.out.len();
    r.render(0.4);
    let bent = pitch_of(&r.out[b..]);
    println!("MPE: {plain:.1} Hz, bent {bent:.1} Hz");
    assert!((bent / plain - 2.0).abs() < 0.08, "an octave up: {plain:.1} -> {bent:.1}");
    // channel pressure raises the breath: stronger high harmonics (the voice's own limiter keeps
    // the level, so loudness is not the measure), as the first difference's share of the signal
    let bright = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>()) / rms(x);
    let c = r.out.len();
    r.render(0.5);
    let soft = bright(&r.out[c..]);
    cm_midi(r.h, 0xD2, 127, 0);
    r.render(0.5);
    let d = r.out.len();
    r.render(0.5);
    let hard = bright(&r.out[d..]);
    println!("pressure: high-harmonic share {soft:.4} -> {hard:.4}");
    assert!(hard > soft * 1.05);
    // without MPE, the same channel's bend is the wheel (2 semitones)
    r.set(p::MPE, 0.0);
    r.render(0.6);
    let e = r.out.len();
    r.render(0.4);
    let wheel = pitch_of(&r.out[e..]);
    println!("no MPE: {wheel:.1} Hz");
    // 12/48 of the way up the wheel's +-2 semitones: half a semitone
    assert!((wheel / plain - 2f64.powf(0.5 / 12.0)).abs() < 0.02);
    cm_midi(r.h, 0x82, 57, 0);
    write_wav("mpe.wav", &r.out);
}

/// The step of the Key parameter that types this character (as the plugin's keyboard numbers them).
fn key_of(c: char) -> f32 {
    match c {
        'a'..='z' => (c as u8 - b'a' + 1) as f32,
        ' ' => 27.0,
        '.' => 28.0,
        ',' => 29.0,
        '?' => 30.0,
        '!' => 31.0,
        '\'' => 32.0,
        '/' => 34.0, // Line
        _ => 0.0,
    }
}

#[test]
fn typed_words_are_shown_and_spoken() {
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.wait_for_lyrics();
    // type into speech slot 4: clear it, then two lines
    r.set(p::TYPE_INTO, 0.0);
    r.set(p::TYPE_SLOT, 3.0);
    r.set(p::KEY, 35.0); // Clear
    r.render(0.01);
    r.set(p::KEY, 0.0);
    r.render(0.01);
    for c in "hi there/bye".chars() {
        r.set(p::KEY, key_of(c));
        r.render(0.006);
        r.set(p::KEY, 0.0);
        r.render(0.006);
    }
    let mut buf = [0u8; 64];
    cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    let shown = std::ffi::CStr::from_bytes_until_nul(&buf).unwrap().to_str().unwrap().to_string();
    println!("typed: {shown:?}");
    assert_eq!(shown, "hi there/bye_");
    // Del takes the last letter away
    r.set(p::KEY, 33.0);
    r.render(0.01);
    r.set(p::KEY, 0.0);
    r.render(0.01);
    cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    assert_eq!(std::ffi::CStr::from_bytes_until_nul(&buf).unwrap().to_str().unwrap(), "hi there/by_");
    // after the pause the slot is planned again: speak it (slot 4, from its first line)
    r.render(0.8);
    std::thread::sleep(std::time::Duration::from_millis(600));
    r.render(0.2);
    r.set(p::SPEECH, 3.0);
    r.render(0.05);
    r.out.clear();
    cm_note_on(r.h, 60, 100);
    r.render(0.1);
    cm_note_off(r.h, 60);
    r.render(1.4);
    assert!(rms(&r.out) > 0.005, "silent: {}", rms(&r.out));
    write_wav("typed_hi_there.wav", &r.out);
}

#[test]
fn typed_texts_survive_a_save_and_load() {
    let bank = cm_texts_new();
    let data = b"CMT1\x00\x02\x05\x00hello";
    assert_eq!(cm_texts_load(bank, data.as_ptr(), data.len() as i32), 1);
    let n = cm_texts_save(bank, std::ptr::null_mut(), 0);
    let mut out = vec![0u8; n as usize];
    assert_eq!(cm_texts_save(bank, out.as_mut_ptr(), n), n);
    assert_eq!(&out, data);
    cm_texts_free(bank);
}

#[test]
fn breathe_left_on_in_play_does_not_breathe_in_another_mode() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.set(p::BREATHE, 1.0);
    r.render(1.0);
    assert!(rms(&r.out[r.out.len() / 2..]) > 0.005, "Breathe makes no sound in Play");
    // to Sing with Breathe still on, no note: silence once the breath has died away
    r.set(p::MODE, 0.0);
    r.render(1.5);
    r.out.clear();
    r.render(0.5);
    assert!(rms(&r.out) < 1e-3, "the voice kept breathing in Sing: {}", rms(&r.out));
    // Breathe off there, back to Play: still silent with no key held
    r.set(p::BREATHE, 0.0);
    r.render(0.1);
    r.set(p::MODE, 1.0);
    r.render(1.5);
    r.out.clear();
    r.render(0.5);
    assert!(rms(&r.out) < 1e-3, "the voice is stuck on in Play: {}", rms(&r.out));
}

#[test]
fn drum_hits_are_reported_for_the_screen() {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.render(0.2);
    let mut shown = [0i32; 4];
    cm_display(r.h, shown.as_mut_ptr());
    let before = shown[3];
    // a GM snare key: the snare pad (3: "Pf")
    cm_note_on(r.h, 38, 100);
    r.render(0.05);
    cm_note_off(r.h, 38);
    cm_display(r.h, shown.as_mut_ptr());
    assert_eq!(shown[2], 3, "the snare's pad");
    assert_ne!(shown[3], before, "the hit count moved");
    // the kit pad Kick (1), tapped on the screen
    r.set(p::PAD, 1.0);
    r.render(0.05);
    r.set(p::PAD, 0.0);
    r.render(0.05);
    cm_display(r.h, shown.as_mut_ptr());
    assert_eq!(shown[2], 1, "the kick's pad");
    // the beat's own steps light their pads as they sound
    r.set(p::FOLLOW, 0.0);
    r.set(p::PLAY_BEAT, 1.0);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..80 {
        r.render(0.025);
        cm_display(r.h, shown.as_mut_ptr());
        seen.insert(shown[2]);
    }
    assert!(seen.len() >= 3, "the beat lit only {seen:?}");
}

/// Render `blocks` blocks of 128 with the host's transport playing at 120 bpm from `beats`;
/// returns the pads the screen was told about and the beat position reached.
fn follow_host(r: &mut Rig, beats: f64, blocks: usize) -> (std::collections::HashSet<i32>, f64) {
    let mut seen = std::collections::HashSet::new();
    let mut b = beats;
    let mut buf = [0.0f32; 128];
    let mut shown = [0i32; 4];
    for _ in 0..blocks {
        cm_transport(r.h, 1, 1, 120.0, 1, b, 128);
        cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
        cm_render(r.h, buf.as_mut_ptr(), 128);
        b += 128.0 / FS * 2.0;
        cm_display(r.h, shown.as_mut_ptr());
        seen.insert(shown[2]);
    }
    (seen, b)
}

#[test]
fn the_beat_lights_its_pads_while_following_the_host_and_after_a_body_change() {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.set(p::FOLLOW, 1.0);
    r.render(0.2);
    let (seen, beats) = follow_host(&mut r, 0.0, 700);
    assert!(seen.len() >= 3, "following the host, the beat lit only {seen:?}");
    // a body change starts the new voice's clock at zero: the lights go on
    r.set(p::BODY, 1.0);
    let (seen, _) = follow_host(&mut r, beats, 700);
    assert!(seen.len() >= 3, "after a body change, the beat lit only {seen:?}");
}

#[test]
fn gain_turns_the_voice_up_into_its_limiter_without_clipping() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.set(p::GAIN, 12.0);
    r.set(p::BREATHE, 1.0);
    r.render(1.5);
    let peak = r.out.iter().fold(0.0f32, |m, x| m.max(x.abs()));
    assert!(peak <= 1.0, "Gain +12 dB clipped: peak {peak}");
    let loud = rms(&r.out[r.out.len() / 2..]);
    let mut q = Rig::new();
    q.set(p::MODE, 1.0);
    q.set(p::GAIN, -12.0);
    q.set(p::BREATHE, 1.0);
    q.render(1.5);
    let soft = rms(&q.out[q.out.len() / 2..]);
    assert!(loud > soft * 2.0, "Gain does not change the level: {loud} vs {soft}");
}

#[test]
fn the_keyboard_does_not_type_while_the_sequence_plays() {
    let mut r = Rig::new();
    r.render(0.3);
    let mut buf = [0u8; 64];
    let before = cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    let mut out = [0.0f32; 128];
    // a recorded tap played back by the sequence: ignored
    for k in [1.0f32, 0.0] {
        r.set(p::KEY, k);
        cm_transport(r.h, 1, 1, 120.0, 1, 0.0, 128);
        cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
        cm_render(r.h, out.as_mut_ptr(), 128);
    }
    let during = cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    assert_eq!(before, during, "a key typed while the sequence played");
    // stopped: the same tap types
    for k in [1.0f32, 0.0] {
        r.set(p::KEY, k);
        cm_transport(r.h, 0, 1, 120.0, 1, 0.0, 128);
        cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
        cm_render(r.h, out.as_mut_ptr(), 128);
    }
    assert_ne!(cm_text_preview(r.h, buf.as_mut_ptr(), 64), during, "a tap did not type while stopped");
}
