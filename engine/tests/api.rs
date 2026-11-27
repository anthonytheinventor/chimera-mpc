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
    // timed alike: with the transport running, Sing's lookahead delays both notes the same
    cm_transport(f.h, 1, 1, 120.0, 1, 0.0, 128);
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
    // a GM drum key outside the pads' notes (36-51): crash cymbal 2 -> the crash pad (9: "kch")
    cm_note_on(r.h, 57, 100);
    r.render(0.05);
    cm_note_off(r.h, 57);
    cm_display(r.h, shown.as_mut_ptr());
    assert_eq!(shown[2], 9, "the crash's pad");
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

#[test]
fn breathe_off_in_play_stops_the_voice() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.render(0.3);
    r.set(p::BREATHE, 1.0);
    r.render(1.0);
    assert!(rms(&r.out[r.out.len() - 4410..]) > 0.005, "Breathe makes no sound");
    r.set(p::BREATHE, 0.0);
    r.render(0.6);
    r.out.clear();
    r.render(0.3);
    assert!(rms(&r.out) < 1e-3, "the voice goes on after Breathe is off: {}", rms(&r.out));
}

#[test]
fn a_tap_types_even_when_the_key_never_reaches_a_block() {
    let mut r = Rig::new();
    r.wait_for_lyrics(); // the worker has filled the bank
    r.render(0.3);
    let mut buf = [0u8; 64];
    let before = cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    // the plugin hands the tap over directly; the Key parameter itself stays 0
    cm_tap(r.h, p::KEY as i32, key_of('h') as i32);
    r.render(0.2);
    let after = cm_text_preview(r.h, buf.as_mut_ptr(), 64);
    assert_ne!(before, after, "the tap did not type");
    let n = buf.iter().position(|&b| b == 0).unwrap_or(64);
    let text = std::str::from_utf8(&buf[..n]).unwrap();
    assert!(text.ends_with("h_"), "typed text: {text}");
}

/// The face's frames now, as the plugin reads them once a block: jaw, tongue, upper lip, lower lip,
/// larynx, sound (cm_face), then the soft palate (cm_display).
fn face_now(r: &mut Rig) -> [i32; 7] {
    let (mut f, mut d) = ([0i32; 6], [0i32; 4]);
    cm_face(r.h, f.as_mut_ptr());
    cm_display(r.h, d.as_mut_ptr());
    [f[0], f[1], f[2], f[3], f[4], f[5], d[1]]
}

/// Render `seconds` in blocks of 128, reading the face after each block as the plugin does;
/// returns the last reading.
fn render_face(r: &mut Rig, seconds: f64) -> [i32; 7] {
    let mut shown = face_now(r);
    for _ in 0..(seconds * 20.0).ceil() as usize {
        r.render(0.05);
        shown = face_now(r);
    }
    shown
}

#[test]
fn each_face_layer_follows_its_knob_in_play() {
    let mut frames = [0i32; 6];
    cm_face_frames(frames.as_mut_ptr());
    let mut rest = [0i32; 6];
    cm_face_rest(rest.as_mut_ptr());
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    let at_rest = render_face(&mut r, 0.6);
    for (k, (&s, &n)) in at_rest.iter().zip(frames.iter()).enumerate() {
        assert!(s >= 0 && s < n, "layer {k}: frame {s} of {n}");
    }
    assert_eq!(&at_rest[..5], &rest[..5], "the rest frames the plugin starts its parameters at: live {at_rest:?}, rest {rest:?}");
    assert_eq!(at_rest[6], 0, "the soft palate is shut at rest");
    let defaults = r.params.clone();
    // (knob, value, the layers that must move: indices into face_now)
    let moves: [(usize, f32, &[usize]); 8] = [
        (p::JAW, 2.0, &[0]),
        (p::TONGUE_UP, 7.8, &[1]),
        (p::TONGUE_FWD, 1.4, &[1]),
        (p::LIP_OPEN, 1.9, &[2, 3]),
        (p::PROTRUDE, 1.3, &[2, 3]),
        (p::LARYNX, -1.4, &[4]),
        (p::VELUM, 1.0, &[6]),
        (p::TIP_UP, 9.0, &[1]),
    ];
    for (knob, value, layers) in moves {
        r.params = defaults.clone();
        r.params[p::MODE] = 1.0;
        render_face(&mut r, 0.6);
        let before = face_now(&mut r);
        r.set(knob, value);
        let after = render_face(&mut r, 0.6);
        for &l in layers {
            assert_ne!(before[l], after[l], "knob {knob} to {value}: layer {l} stayed at frame {}", before[l]);
        }
    }
    // the vowel pad moves the tongue, jaw and lips together
    r.params = defaults.clone();
    r.params[p::MODE] = 1.0;
    r.pad(1.0, 0.0);
    let i = render_face(&mut r, 0.6);
    r.pad(0.5, 1.0);
    let a = render_face(&mut r, 0.6);
    assert_ne!(i[1], a[1], "the tongue from i to a");
    assert_ne!(i[0], a[0], "the jaw from i to a");
}

#[test]
fn the_sound_layer_lights_while_the_voice_sounds() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    assert_eq!(render_face(&mut r, 0.5)[5], 0, "silent: the sound layer is dark");
    // a held note
    cm_note_on(r.h, 48, 100);
    let mut lit = 0;
    for _ in 0..20 {
        lit = lit.max(render_face(&mut r, 0.05)[5]);
    }
    assert!(lit > 0, "a held note leaves the sound layer dark");
    cm_note_off(r.h, 48);
    assert_eq!(render_face(&mut r, 1.5)[5], 0, "the sound layer goes dark after the note");
    // Breathe, no key
    r.set(p::BREATHE, 1.0);
    let breathing = render_face(&mut r, 1.0)[5];
    assert!(breathing > 0, "Breathe leaves the sound layer dark");
    // louder breath, a brighter fan
    r.set(p::BREATH, 20.0);
    r.set(p::GAIN, 6.0);
    let loud = render_face(&mut r, 1.0)[5];
    assert!(loud >= breathing, "more breath dimmed the fan: {breathing} -> {loud}");
    r.set(p::BREATHE, 0.0);
    assert_eq!(render_face(&mut r, 1.5)[5], 0, "silent again after Breathe");
}

#[test]
fn the_face_follows_speech() {
    let mut r = Rig::new();
    r.wait_for_lyrics();
    r.set(p::MODE, 2.0);
    r.render(0.1);
    cm_note_on(r.h, 60, 100);
    let mut seen = [std::collections::HashSet::new(), std::collections::HashSet::new(), std::collections::HashSet::new(), std::collections::HashSet::new()];
    for _ in 0..60 {
        let f = render_face(&mut r, 0.03);
        for (s, v) in seen.iter_mut().zip([f[0], f[1], f[3], f[5]]) {
            s.insert(v);
        }
    }
    cm_note_off(r.h, 60);
    assert!(seen[0].len() >= 2, "the jaw stood still while speaking: {:?}", seen[0]);
    assert!(seen[1].len() >= 4, "the tongue barely moved while speaking: {:?}", seen[1]);
    assert!(seen[2].len() >= 2, "the lower lip stood still while speaking: {:?}", seen[2]);
    assert!(seen[3].len() >= 2, "the sound layer did not follow the speech: {:?}", seen[3]);
}

#[test]
fn the_pads_play_the_beatbox_squares_and_light_them() {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.set(p::FOLLOW, 0.0);
    for i in 0..16 {
        r.set(p::STEP_1 + i, 0.0); // all rests
    }
    r.set(p::STEP_1, 1.0); // square 1: kick
    r.render(0.3);
    let mut lit = [0i32; 2];
    // pad 1 (note 36): square 1 lights and its kick sounds
    cm_note_on(r.h, PAD_NOTE_FIRST as i32, 110);
    r.out.clear();
    r.render(0.4);
    cm_note_off(r.h, PAD_NOTE_FIRST as i32);
    cm_step_lit(r.h, lit.as_mut_ptr());
    assert_eq!(lit[0], 1, "square 1 lit");
    assert!(rms(&r.out) > 0.003, "pad 1 played nothing: {}", rms(&r.out));
    let count = lit[1];
    // pad 6 (note 41) on a rest: square 6 lights, nothing sounds
    r.render(0.6);
    cm_note_on(r.h, PAD_NOTE_FIRST as i32 + 5, 110);
    r.out.clear();
    r.render(0.3);
    cm_note_off(r.h, PAD_NOTE_FIRST as i32 + 5);
    cm_step_lit(r.h, lit.as_mut_ptr());
    assert_eq!(lit[0], 6, "square 6 lit");
    assert_ne!(lit[1], count, "the count moved");
    assert!(rms(&r.out) < 1e-3, "a rest square sounded: {}", rms(&r.out));
    // the beat lights the steps it reaches, rests too
    r.set(p::PLAY_BEAT, 1.0);
    let mut seen = std::collections::HashSet::new();
    for _ in 0..120 {
        r.render(0.025);
        cm_step_lit(r.h, lit.as_mut_ptr());
        seen.insert(lit[0]);
    }
    assert!(seen.len() >= 8, "the beat lit only {seen:?}");
}

/// Power at `hz` (Goertzel over the whole signal, Hann window).
fn tone_power(x: &[f32], hz: f64) -> f64 {
    let n = x.len();
    let w = 2.0 * std::f64::consts::PI * hz / FS;
    let c = 2.0 * w.cos();
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for (i, &v) in x.iter().enumerate() {
        let win = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos();
        let s = v as f64 * win + c * s1 - s2;
        s2 = s1;
        s1 = s;
    }
    s1 * s1 + s2 * s2 - c * s1 * s2
}

#[test]
fn consonants_have_no_tone_at_a_quarter_of_the_rate() {
    // geometry every 4 samples at 1x made an 11 kHz line at consonants; the voice notches it
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.wait_for_lyrics();
    r.render(0.5);
    r.out.clear();
    for _ in 0..3 {
        cm_note_on(r.h, 60, 100);
        r.render(0.05);
        cm_note_off(r.h, 60);
        r.render(1.4);
    }
    let line = tone_power(&r.out, FS / 4.0);
    // the neighbourhood: a few frequencies around 9.5 kHz, where the voice has its own noise
    let around = [9300.0, 9500.0, 9700.0].iter().map(|&f| tone_power(&r.out, f)).sum::<f64>() / 3.0;
    let db = 10.0 * (line / around.max(1e-30)).log10();
    eprintln!("11025 Hz vs 9.5 kHz: {db:.1} dB");
    assert!(db < -20.0, "a tone at fs/4: {db:.1} dB against its neighbourhood");
}

#[test]
fn play_breathes_harder_for_a_harder_key() {
    let level = |vel: i32| {
        let mut r = Rig::new();
        r.set(p::MODE, 1.0);
        r.render(0.3);
        cm_note_on(r.h, 48, vel);
        r.out.clear();
        r.render(1.0);
        rms(&r.out[r.out.len() / 2..])
    };
    let (soft, hard) = (level(25), level(127));
    assert!(hard > soft * 1.2, "velocity does not change Play's breath: soft {soft}, hard {hard}");
}

/// When the hit of a beatbox pad lands after its note (its loudest moment), in ms.
fn pad_hit_ms(playing: bool, square_sound: f32) -> f64 {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.set(p::FOLLOW, 0.0);
    for i in 0..16 {
        r.set(p::STEP_1 + i, 0.0);
    }
    r.set(p::STEP_1, square_sound);
    let mut buf = [0.0f32; 128];
    let mut block = |r: &mut Rig| {
        cm_transport(r.h, playing as i32, 1, 120.0, 1, 0.0, 128);
        cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
        cm_render(r.h, buf.as_mut_ptr(), 128);
        r.out.extend_from_slice(&buf);
    };
    for _ in 0..100 {
        block(&mut r);
    }
    r.out.clear();
    cm_note_on(r.h, PAD_NOTE_FIRST as i32, 110);
    for _ in 0..140 {
        block(&mut r);
    }
    // the hit is the burst: the loudest 2 ms window
    let w = (0.002 * FS) as usize;
    let e: Vec<f32> = r.out.windows(w).step_by(16).map(|x| x.iter().map(|v| v * v).sum::<f32>()).collect();
    let i = e.iter().enumerate().fold((0, 0.0f32), |b, (i, &v)| if v > b.1 { (i, v) } else { b }).0;
    (i * 16 + w / 2) as f64 / FS * 1000.0
}

#[test]
fn beatbox_hits_land_a_fixed_time_after_their_notes_while_the_sequence_runs() {
    // square 1 as a kick (1) or a hat (2): different set-up times
    let (kick, hat) = (pad_hit_ms(true, 1.0), pad_hit_ms(true, 2.0));
    eprintln!("sequence running: kick {kick:.1} ms, hat {hat:.1} ms");
    assert!((kick - hat).abs() < 12.0, "hits uneven: kick {kick:.1} ms, hat {hat:.1} ms");
    assert!(kick > 160.0 && kick < 230.0, "kick at {kick:.1} ms");
    // stopped: as soon as the mouth can make it (earlier, by sound)
    let (kick0, hat0) = (pad_hit_ms(false, 1.0), pad_hit_ms(false, 2.0));
    eprintln!("stopped: kick {kick0:.1} ms, hat {hat0:.1} ms");
    assert!(hat0 < hat - 20.0, "a live hat is not earlier: {hat0:.1} vs {hat:.1} ms");
}

/// Sing "ah", then "stra" (lyrics typed into slot 1); returns when each vowel arrives after its
/// note (ms): the first 5 ms window at half the note's loudest.
fn sung_vowels_ms(playing: bool) -> (f64, f64) {
    let mut r = Rig::new();
    r.set(p::MODE, 0.0);
    r.wait_for_lyrics();
    r.set(p::TYPE_INTO, 1.0);
    r.set(p::TYPE_SLOT, 0.0);
    r.set(p::LYRICS, 0.0);
    r.set(p::KEY, 35.0); // Clear
    r.render(0.01);
    r.set(p::KEY, 0.0);
    r.render(0.01);
    for c in "ah stra".chars() {
        r.set(p::KEY, key_of(c));
        r.render(0.006);
        r.set(p::KEY, 0.0);
        r.render(0.006);
    }
    // the pause, then the lyrics are planned again
    for _ in 0..60 {
        r.render(0.02);
        std::thread::sleep(std::time::Duration::from_millis(15));
    }
    let mut buf = [0.0f32; 128];
    // the transport running (or not) before the notes; Restart off, so starting it keeps the cursor
    r.set(p::RESTART, 0.0);
    for _ in 0..20 {
        cm_transport(r.h, playing as i32, 1, 120.0, 1, 0.0, 128);
        cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
        cm_render(r.h, buf.as_mut_ptr(), 128);
    }
    let mut note = |r: &mut Rig, n: i32| -> f64 {
        r.out.clear();
        cm_note_on(r.h, n, 100);
        for _ in 0..(0.7 * FS / 128.0) as usize {
            cm_transport(r.h, playing as i32, 1, 120.0, 1, 0.0, 128);
            cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
            cm_render(r.h, buf.as_mut_ptr(), 128);
            r.out.extend_from_slice(&buf);
        }
        cm_note_off(r.h, n);
        for _ in 0..(0.5 * FS / 128.0) as usize {
            cm_transport(r.h, playing as i32, 1, 120.0, 1, 0.0, 128);
            cm_set_params(r.h, r.params.as_ptr(), p::COUNT as i32);
            cm_render(r.h, buf.as_mut_ptr(), 128);
        }
        let w = (0.005 * FS) as usize;
        let e: Vec<f32> = r.out.chunks(w).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let top = e.iter().cloned().fold(0.0f32, f32::max);
        e.iter().position(|&v| v >= 0.5 * top).unwrap() as f64 * 5.0
    };
    (note(&mut r, 57), note(&mut r, 59))
}

#[test]
fn while_the_sequence_runs_sung_vowels_land_a_fixed_time_after_their_notes() {
    let (ah, stra) = sung_vowels_ms(true);
    eprintln!("sequence running: ah {ah:.0} ms, stra {stra:.0} ms");
    assert!((ah - stra).abs() < 40.0, "vowels uneven: ah {ah:.0}, stra {stra:.0} ms");
    assert!(ah > 140.0 && ah < 260.0, "ah's vowel at {ah:.0} ms");
    let (ah0, stra0) = sung_vowels_ms(false);
    eprintln!("stopped: ah {ah0:.0} ms, stra {stra0:.0} ms");
    assert!(stra0 > ah0 + 40.0, "live, 'stra' should reach its vowel later: ah {ah0:.0}, stra {stra0:.0} ms");
    assert!(ah0 < ah - 60.0, "live notes should be immediate: {ah0:.0} vs {ah:.0} ms");
}

impl Rig {
    fn wait_for_body(&mut self, secs: usize) -> bool {
        for _ in 0..secs * 40 {
            self.render(0.025);
            if cm_body_ready(self.h) == 1 {
                return true;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        false
    }
}

#[test]
fn the_same_blend_after_a_preset_is_built_again() {
    let mut r = Rig::new();
    r.set(p::MODE, 1.0);
    r.set(p::BODY, 3.0);
    r.set(p::BODY_FEMALE, 0.5);
    r.set(p::BODY_CHILD, 0.3);
    assert!(r.wait_for_body(10), "the first blend never arrived");
    r.set(p::BODY, 0.0);
    r.render(0.2);
    assert_eq!(cm_body_ready(r.h), 1);
    std::thread::sleep(std::time::Duration::from_millis(200));
    r.render(0.2);
    r.set(p::BODY, 3.0);
    assert!(r.wait_for_body(10), "back to the same Blend mix, the blended voice never arrives");
}

#[test]
fn a_parked_voice_brings_back_no_stale_beat_hits() {
    let mut r = Rig::new();
    r.set(p::MODE, 3.0);
    r.set(p::MIC, 0.0);
    r.set(p::PLAY_BEAT, 1.0);
    r.render(1.03);
    // switch body and stop the beat in the same block, then back to the first body
    r.set(p::BODY, 1.0);
    r.set(p::PLAY_BEAT, 0.0);
    r.render(1.5);
    r.set(p::BODY, 0.0);
    r.out.clear();
    r.render(0.6);
    assert!(rms(&r.out) < 0.002, "stale hits from the parked voice: rms {}", rms(&r.out));
}

/// How long Speak's first and second lines sound (s), with or without a body change between.
fn speak_two_lines(change_body: bool) -> (f64, f64) {
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.set(p::SPEECH, 1.0);
    r.wait_for_lyrics();
    let mut voiced = |r: &mut Rig, s: f64| {
        r.out.clear();
        r.render(s);
        r.out.chunks(128).filter(|c| (c.iter().map(|x| x * x).sum::<f32>() / 128.0).sqrt() > 0.003).count() as f64 * 128.0 / FS
    };
    cm_note_on(r.h, 60, 100);
    let a = voiced(&mut r, 3.0);
    cm_note_off(r.h, 60);
    if change_body {
        r.set(p::BODY, 1.0);
        r.render(0.1);
        r.set(p::BODY, 0.0);
        r.render(0.1);
    }
    cm_note_on(r.h, 60, 100);
    let b = voiced(&mut r, 4.0);
    (a, b)
}

#[test]
fn a_body_change_keeps_the_next_line() {
    let plain = speak_two_lines(false);
    let changed = speak_two_lines(true);
    eprintln!("plain {plain:?}, with a body change {changed:?}");
    assert!((changed.1 - plain.1).abs() < 0.3, "after a body change the second note said another line: {changed:?} vs {plain:?}");
}

#[test]
fn the_cursor_moves_where_keys_type_and_delete() {
    let mut r = Rig::new();
    r.set(p::MODE, 2.0);
    r.wait_for_lyrics();
    r.set(p::TYPE_INTO, 0.0);
    r.set(p::TYPE_SLOT, 2.0);
    let preview = |r: &mut Rig| {
        r.render(0.01);
        let mut buf = [0u8; 64];
        cm_text_preview(r.h, buf.as_mut_ptr(), 64);
        std::ffi::CStr::from_bytes_until_nul(&buf).unwrap().to_str().unwrap().to_string()
    };
    r.render(0.05); // the slot settings reach the engine before the taps
    cm_tap(r.h, p::KEY as i32, 35); // Clear
    for c in "helo".chars() {
        cm_tap(r.h, p::KEY as i32, key_of(c) as i32);
    }
    assert_eq!(preview(&mut r), "helo_");
    // one character back: between "hel" and "o"; type "l"
    cm_text_cursor(r.h, 1);
    assert_eq!(preview(&mut r), "hel_o");
    cm_tap(r.h, p::KEY as i32, key_of('l') as i32);
    assert_eq!(preview(&mut r), "hell_o", "typed at the cursor");
    // Del takes the character before the cursor
    cm_tap(r.h, p::KEY as i32, 33);
    cm_tap(r.h, p::KEY as i32, 33);
    assert_eq!(preview(&mut r), "he_o", "deleted before the cursor");
    // past the start: at the start
    cm_text_cursor(r.h, 50);
    assert_eq!(preview(&mut r), "_heo");
    cm_text_cursor(r.h, 0);
    assert_eq!(preview(&mut r), "heo_");
}
