//! The engine's own thread: everything that allocates, off the audio thread.
//!
//! It loads the pronunciation dictionary once, turns the lyrics lines into syllables, plans the
//! spoken lines (again when the speech rate or the IPA switch changes), builds a voice for a
//! blended body (when the Blend mix changes), and frees whatever the audio thread hands back.
//! The audio thread only ever `try_lock`s the hub, so it never waits for this thread.

use crate::{BODIES, DEFAULT_LYRICS, SLOTS, mpc_voice};
use chimera::anatomy::human::BodyMix;
use chimera::control::score::Gesture;
use chimera::control::singer::syllables_from;
use chimera::engine::default_f0;
use chimera::phonetics::english::{EnglishG2P, Phrase};
use chimera::phonetics::ipa::tokenize_ipa;
use chimera::phonetics::planner::{PhraseInput, PlanOptions, plan_phrases};
use chimera::phonetics::sing::LiveSyllable;
use chimera::phonetics::syllables::lyrics_to_syllables;
use chimera::phonetics::Intonation;
use chimera::voice::human_voice::HumanVoice;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Each slot's lyrics as the Singer holds them.
pub type Lyrics = Vec<Vec<LiveSyllable>>;
/// Each slot's spoken lines, planned for each preset body: slot -> body -> line -> gestures.
pub type Speech = Vec<[Vec<Vec<Gesture>>; 3]>;

/// What the audio thread asks for. It writes the whole request when something changes.
#[derive(Clone, Copy, PartialEq)]
pub struct Request {
    pub speech_rate: f32,
    pub ipa: bool,
    /// the Blend mix wanted (None: no blended body)
    pub blend: Option<(f32, f32)>,
}

/// Something the audio thread no longer needs; this thread frees it.
pub enum Trash {
    Voice(#[allow(dead_code)] Box<HumanVoice>),
    Speech(#[allow(dead_code)] Speech),
}

#[derive(Default)]
pub struct Outbox {
    pub lyrics: Option<Lyrics>,
    pub speech: Option<Speech>,
    /// a voice built for this Blend mix
    pub blend: Option<((f32, f32), Box<HumanVoice>)>,
}

pub struct Hub {
    pub request: Mutex<Option<Request>>,
    pub out: Mutex<Outbox>,
    pub trash: Mutex<Vec<Trash>>,
    pub quit: AtomicBool,
}

/// Room in the trash, so the audio thread's push never allocates.
pub const TRASH_ROOM: usize = 64;

impl Hub {
    pub fn new() -> Arc<Hub> {
        Arc::new(Hub { request: Mutex::new(None), out: Mutex::new(Outbox::default()), trash: Mutex::new(Vec::with_capacity(TRASH_ROOM)), quit: AtomicBool::new(false) })
    }
}

pub fn start(hub: Arc<Hub>, fs: f64, content: Option<String>) {
    let _ = std::thread::Builder::new().name("chimera-worker".into()).spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| run(&hub, fs, content.as_deref())));
    });
}

fn read_text(content: Option<&str>, name: &str) -> String {
    content.and_then(|c| std::fs::read_to_string(Path::new(c).join(name)).ok()).unwrap_or_default()
}

fn load_g2p(content: Option<&str>) -> EnglishG2P {
    let mut g = EnglishG2P::new();
    if let Some(gz) = content.and_then(|c| std::fs::read(Path::new(c).join("cmudict.txt.gz")).ok()) {
        let mut text = String::new();
        if flate2::read::GzDecoder::new(&gz[..]).read_to_string(&mut text).is_ok() {
            g.load_dict(&text);
        }
    }
    // without the dictionary, letter-to-sound rules
    g
}

/// The lyrics slots of `lyrics.txt`: up to SLOTS non-empty lines, blank and `#` lines skipped;
/// an empty slot sings CHIMERA's default line.
pub fn lyric_slots(text: &str) -> Vec<String> {
    let mut slots: Vec<String> = text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).take(SLOTS).map(String::from).collect();
    while slots.len() < SLOTS {
        slots.push(DEFAULT_LYRICS.to_string());
    }
    slots
}

/// The speech slots of `speech.txt`: blocks of lines separated by a blank line, each line said
/// by one note; `#` lines skipped. A missing slot says CHIMERA's default.
pub fn speech_slots(text: &str) -> Vec<Vec<String>> {
    let mut slots = Vec::new();
    let mut cur: Vec<String> = Vec::new();
    for l in text.lines().map(str::trim) {
        if l.starts_with('#') {
            continue;
        }
        if l.is_empty() {
            if !cur.is_empty() {
                slots.push(std::mem::take(&mut cur));
            }
            continue;
        }
        cur.push(l.to_string());
    }
    if !cur.is_empty() {
        slots.push(cur);
    }
    slots.truncate(SLOTS);
    while slots.len() < SLOTS {
        slots.push(vec!["Hello.".to_string(), "Can you hear me?".to_string()]);
    }
    slots
}

/// The same rules as CHIMERA's: `|`, `.`, `?` and `!` separate phrases.
fn ipa_phrases(text: &str) -> Vec<Phrase> {
    text.split(['|', '.', '?', '!']).map(str::trim).filter(|p| !p.is_empty()).map(|p| Phrase { segments: tokenize_ipa(p), intonation: Intonation::Statement, pause: 0.3, words: Vec::new() }).collect()
}

/// A line's gestures for a body, as CHIMERA's plugin plans Speak lines.
fn plan_line(phrases: &[Phrase], body: BodyMix, rate: f64) -> Vec<Gesture> {
    let input: Vec<PhraseInput> = phrases.iter().map(|p| PhraseInput { segments: p.segments.clone(), intonation: Some(p.intonation), pause: Some(p.pause) }).collect();
    plan_phrases(&input, &PlanOptions { body, t0: 0.0, rate: Some(rate), f0: Some(default_f0(body)), ..Default::default() }).gestures
}

fn plan_speech(slots: &[Vec<String>], g2p: &EnglishG2P, rate: f64, ipa: bool) -> Speech {
    slots
        .iter()
        .map(|lines| {
            let phrases: Vec<Vec<Phrase>> = lines.iter().map(|l| if ipa { ipa_phrases(l) } else { g2p.to_phrases(l) }).filter(|p| !p.is_empty()).collect();
            let per_body = |b: usize| phrases.iter().map(|p| plan_line(p, BODIES[b], rate)).collect::<Vec<_>>();
            [per_body(0), per_body(1), per_body(2)]
        })
        .collect()
}

fn run(hub: &Hub, fs: f64, content: Option<&str>) {
    let g2p = load_g2p(content);
    let lyrics: Lyrics = lyric_slots(&read_text(content, "lyrics.txt")).iter().map(|t| syllables_from(&lyrics_to_syllables(t, &g2p))).collect();
    if let Ok(mut o) = hub.out.lock() {
        o.lyrics = Some(lyrics);
    }
    let speech_text = speech_slots(&read_text(content, "speech.txt"));
    let mut built_speech: Option<(f32, bool)> = None;
    let mut built_blend: Option<(f32, f32)> = None;
    while !hub.quit.load(Ordering::Relaxed) {
        let req = hub.request.lock().ok().and_then(|r| *r);
        if let Some(r) = req {
            if built_speech != Some((r.speech_rate, r.ipa)) {
                let s = plan_speech(&speech_text, &g2p, r.speech_rate as f64, r.ipa);
                built_speech = Some((r.speech_rate, r.ipa));
                // an unclaimed older plan is replaced (and freed) here
                if let Ok(mut o) = hub.out.lock() {
                    o.speech = Some(s);
                }
            }
            if let Some(mix) = r.blend
                && built_blend != Some(mix)
            {
                let v = mpc_voice(fs, BodyMix::new(mix.0 as f64, mix.1 as f64));
                built_blend = Some(mix);
                if let Ok(mut o) = hub.out.lock() {
                    o.blend = Some((mix, v));
                }
            }
        }
        // free what the audio thread handed back (dropped outside the lock)
        let dropped: Vec<Trash> = hub.trash.lock().map(|mut t| t.drain(..).collect()).unwrap_or_default();
        drop(dropped);
        std::thread::sleep(Duration::from_millis(30));
    }
}
