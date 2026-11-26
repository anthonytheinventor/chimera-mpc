//! The engine's own thread: everything that allocates, off the audio thread.
//!
//! It loads the pronunciation dictionary once, turns the lyrics lines into syllables, plans the
//! spoken lines (again when the speech rate or the IPA switch changes), builds a voice for a
//! blended body (when the Blend mix changes), and frees whatever the audio thread hands back.
//! The audio thread only ever `try_lock`s the hub, so it never waits for this thread.
//!
//! The texts come from the text bank (`texts`): filled from Content/lyrics.txt and speech.txt at
//! the start (slots a project has typed into keep theirs), and planned again whenever the typing
//! in a target has paused for TYPING_PAUSE.

use crate::texts::{TARGET_LYRICS, TARGET_SPEECH, Texts};
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
use std::time::{Duration, Instant};

/// How long the typing in a target must pause before its texts are planned again.
const TYPING_PAUSE: Duration = Duration::from_millis(400);

/// Each slot's lyrics as the Singer holds them.
pub type Lyrics = Vec<Vec<LiveSyllable>>;
/// What Speak says, planned for each preset body.
pub struct Speech {
    /// slot -> body -> line -> gestures
    pub lines: Vec<[Vec<Vec<Gesture>>; 3]>,
    /// the consonant chart: consonant -> vowel around it -> body -> gestures
    pub pads: Vec<[[Vec<Gesture>; 3]; 4]>,
}

/// The consonant chart's sounds (the Consonant parameter's choices after "-"), and the vowels
/// that can go around them (Pad Vowel), as CHIMERA's chart offers them.
pub const CONSONANTS: [&str; 25] = ["p", "b", "t", "d", "k", "g", "ʔ", "m", "n", "ŋ", "r", "ɾ", "f", "v", "θ", "ð", "s", "z", "ʃ", "ʒ", "x", "h", "ɹ", "j", "l"];
pub const PAD_VOWELS: [&str; 4] = ["ɑ", "i", "u", "ə"];

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
    Lyrics(#[allow(dead_code)] Lyrics),
    Syllables(#[allow(dead_code)] Vec<LiveSyllable>),
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

pub fn start(hub: Arc<Hub>, fs: f64, content: Option<String>, texts: Texts) {
    let _ = std::thread::Builder::new().name("chimera-worker".into()).spawn(move || {
        let _ = catch_unwind(AssertUnwindSafe(|| run(&hub, fs, content.as_deref(), &texts)));
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

/// One consonant between two vowels, in flat intonation, as CHIMERA's chart says it.
fn plan_pad(ipa: &str, body: BodyMix, rate: f64) -> Vec<Gesture> {
    plan_line(&[Phrase { segments: tokenize_ipa(ipa), intonation: Intonation::Flat, pause: 0.25, words: Vec::new() }], body, rate)
}

/// The bank's texts of one target, as the planners take them: lyrics one string a slot, speech a
/// list of lines a slot (a slot with no words says CHIMERA's default).
fn bank_texts(texts: &Texts, target: usize) -> (Vec<String>, u64) {
    let b = match texts.lock() {
        Ok(b) => b,
        Err(p) => p.into_inner(),
    };
    let strings = b.slots[target].iter().map(|s| s.as_str().to_string()).collect();
    (strings, b.generation[target])
}

fn speech_lines(slots: &[String]) -> Vec<Vec<String>> {
    slots
        .iter()
        .map(|t| {
            let lines: Vec<String> = t.lines().map(str::trim).filter(|l| !l.is_empty()).map(String::from).collect();
            if lines.is_empty() { vec!["Hello.".to_string()] } else { lines }
        })
        .collect()
}

fn plan_lyrics(slots: &[String], g2p: &EnglishG2P) -> Lyrics {
    slots.iter().map(|t| syllables_from(&lyrics_to_syllables(if t.trim().is_empty() { DEFAULT_LYRICS } else { t }, g2p))).collect()
}

fn plan_speech(slots: &[Vec<String>], g2p: &EnglishG2P, rate: f64, ipa: bool) -> Speech {
    let pads = CONSONANTS
        .iter()
        .map(|c| {
            let per_vowel = |v: &str| {
                let say = format!("ˈ{v}{c}{v}");
                [plan_pad(&say, BODIES[0], rate), plan_pad(&say, BODIES[1], rate), plan_pad(&say, BODIES[2], rate)]
            };
            [per_vowel(PAD_VOWELS[0]), per_vowel(PAD_VOWELS[1]), per_vowel(PAD_VOWELS[2]), per_vowel(PAD_VOWELS[3])]
        })
        .collect();
    let lines = slots
        .iter()
        .map(|lines| {
            let phrases: Vec<Vec<Phrase>> = lines.iter().map(|l| if ipa { ipa_phrases(l) } else { g2p.to_phrases(l) }).filter(|p| !p.is_empty()).collect();
            let per_body = |b: usize| phrases.iter().map(|p| plan_line(p, BODIES[b], rate)).collect::<Vec<_>>();
            [per_body(0), per_body(1), per_body(2)]
        })
        .collect();
    Speech { lines, pads }
}

fn run(hub: &Hub, fs: f64, content: Option<&str>, texts: &Texts) {
    // the text files fill the slots no project has typed into (once per plugin: the bank outlives
    // engines)
    if let Ok(mut b) = texts.lock()
        && !b.filled
    {
        let lyrics = lyric_slots(&read_text(content, "lyrics.txt"));
        let speech = speech_slots(&read_text(content, "speech.txt"));
        for s in 0..SLOTS {
            b.original[TARGET_LYRICS][s].set(&lyrics[s]);
            b.original[TARGET_SPEECH][s].set(&speech[s].join("\n"));
            if !b.edited[TARGET_LYRICS][s] {
                b.slots[TARGET_LYRICS][s] = b.original[TARGET_LYRICS][s];
            }
            if !b.edited[TARGET_SPEECH][s] {
                b.slots[TARGET_SPEECH][s] = b.original[TARGET_SPEECH][s];
            }
        }
        b.filled = true;
    }
    let g2p = load_g2p(content);
    let (lyric_text, mut lyrics_gen) = bank_texts(texts, TARGET_LYRICS);
    if let Ok(mut o) = hub.out.lock() {
        o.lyrics = Some(plan_lyrics(&lyric_text, &g2p));
    }
    let (speech_text, mut speech_gen) = bank_texts(texts, TARGET_SPEECH);
    let mut speech_text = speech_lines(&speech_text);
    let mut built_speech: Option<(f32, bool)> = None;
    let mut built_blend: Option<(f32, f32)> = None;
    let mut typed_at: [Option<Instant>; 2] = [None, None];
    while !hub.quit.load(Ordering::Relaxed) {
        // typing: plan a target again once its texts have stopped changing for a moment
        let gens = texts.lock().map(|b| b.generation).unwrap_or([lyrics_gen, speech_gen]);
        let now = Instant::now();
        for t in [TARGET_SPEECH, TARGET_LYRICS] {
            let seen = if t == TARGET_SPEECH { speech_gen } else { lyrics_gen };
            if gens[t] != seen {
                let at = *typed_at[t].get_or_insert(now);
                if now.duration_since(at) < TYPING_PAUSE {
                    continue;
                }
                let (strings, g) = bank_texts(texts, t);
                // still moving: wait for it to rest
                if g != gens[t] {
                    typed_at[t] = Some(now);
                    continue;
                }
                typed_at[t] = None;
                if t == TARGET_SPEECH {
                    speech_gen = g;
                    speech_text = speech_lines(&strings);
                    built_speech = None;
                } else {
                    lyrics_gen = g;
                    let l = plan_lyrics(&strings, &g2p);
                    if let Ok(mut o) = hub.out.lock() {
                        o.lyrics = Some(l);
                    }
                }
            } else {
                typed_at[t] = None;
            }
        }
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
