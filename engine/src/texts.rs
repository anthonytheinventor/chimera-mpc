//! The words you type on the MPC: a bank of fixed-size texts, one per speech slot and lyrics slot.
//!
//! The plugin makes the bank once (`cm_texts_new`) and keeps it for its whole life, so the typed
//! words survive an engine rebuilt for a new sample rate, and it saves and loads it with the
//! project (`cm_texts_save`, `cm_texts_load`, from the host's thread). The audio thread types into
//! it in place (fixed arrays: no allocation) under a `try_lock`; the engine's own thread reads it
//! and plans a slot again once the typing has paused. A slot starts as the plugin's text file has
//! it; once typed into, it is the project's.

use crate::SLOTS;
use std::sync::{Arc, Mutex};

/// Bytes a slot holds (UTF-8; typed keys are ASCII).
pub const TEXT_CAP: usize = 600;
pub const TARGET_SPEECH: usize = 0;
pub const TARGET_LYRICS: usize = 1;

/// The Key parameter's steps after "-": the letters, then these, then Del, Line and Clear.
const PUNCT: [u8; 6] = [b' ', b'.', b',', b'?', b'!', b'\''];
pub const KEY_DEL: usize = 33;
pub const KEY_LINE: usize = 34;
pub const KEY_CLEAR: usize = 35;
/// Characters of a slot the preview shows.
pub const PREVIEW_CHARS: usize = 28;

#[derive(Clone, Copy)]
pub struct Slot {
    bytes: [u8; TEXT_CAP],
    len: usize,
}

impl Default for Slot {
    fn default() -> Self {
        Slot { bytes: [0; TEXT_CAP], len: 0 }
    }
}

impl Slot {
    pub fn as_str(&self) -> &str {
        match std::str::from_utf8(&self.bytes[..self.len]) {
            Ok(s) => s,
            Err(e) => std::str::from_utf8(&self.bytes[..e.valid_up_to()]).unwrap_or(""),
        }
    }

    /// The text, cut at a character boundary if it is too long.
    pub fn set(&mut self, s: &str) {
        let mut n = s.len().min(TEXT_CAP);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        self.bytes[..n].copy_from_slice(&s.as_bytes()[..n]);
        self.len = n;
    }

    fn push(&mut self, b: u8) {
        if self.len < TEXT_CAP {
            self.bytes[self.len] = b;
            self.len += 1;
        }
    }

    /// Remove the last character (all of its bytes).
    fn pop(&mut self) {
        while self.len > 0 {
            self.len -= 1;
            if self.bytes[self.len] & 0xC0 != 0x80 {
                break;
            }
        }
    }
}

pub struct TextBank {
    pub slots: [[Slot; SLOTS]; 2],
    /// what the text files say, for a slot a loaded project has not typed into
    pub original: [[Slot; SLOTS]; 2],
    /// typed into (so the project keeps it, and the text file no longer fills it)
    pub edited: [[bool; SLOTS]; 2],
    /// moves at every change of a target's texts
    pub generation: [u64; 2],
    /// the text files have been read in
    pub filled: bool,
}

pub type Texts = Arc<Mutex<TextBank>>;

pub fn new_bank() -> Texts {
    Arc::new(Mutex::new(TextBank { slots: [[Slot::default(); SLOTS]; 2], original: [[Slot::default(); SLOTS]; 2], edited: [[false; SLOTS]; 2], generation: [0; 2], filled: false }))
}

impl TextBank {
    /// Type key `k` (the Key parameter's step) into a slot. Allocation-free.
    pub fn key(&mut self, target: usize, slot: usize, k: usize) {
        let (t, s) = (target.min(1), slot.min(SLOTS - 1));
        let lines = target == TARGET_SPEECH;
        let sl = &mut self.slots[t][s];
        match k {
            1..=26 => sl.push(b'a' + (k - 1) as u8),
            27..=32 => sl.push(PUNCT[k - 27]),
            KEY_DEL => sl.pop(),
            // a new line: in Speech, what the next note says; in lyrics, a space
            KEY_LINE => sl.push(if lines { b'\n' } else { b' ' }),
            KEY_CLEAR => sl.len = 0,
            _ => return,
        }
        self.edited[t][s] = true;
        self.generation[t] += 1;
    }

    /// The end of a slot as one line ("/" between its lines) with a "_" cursor, into `out`;
    /// returns its length. Allocation-free.
    pub fn preview(&self, target: usize, slot: usize, out: &mut [u8; 64]) -> usize {
        let text = self.slots[target.min(1)][slot.min(SLOTS - 1)].as_str();
        let mut chars: [char; PREVIEW_CHARS] = [' '; PREVIEW_CHARS];
        let mut n = 0;
        // the last PREVIEW_CHARS characters, a line break shown as "/"
        for ch in text.chars().rev() {
            if n == PREVIEW_CHARS {
                break;
            }
            chars[n] = if ch == '\n' { '/' } else { ch };
            n += 1;
        }
        let mut len = 0;
        for ch in chars[..n].iter().rev() {
            let mut b = [0u8; 4];
            let e = ch.encode_utf8(&mut b);
            if len + e.len() + 1 > out.len() {
                break;
            }
            out[len..len + e.len()].copy_from_slice(e.as_bytes());
            len += e.len();
        }
        out[len] = b'_';
        len + 1
    }

    /// The typed slots, for the project: "CMT1", then per slot target, slot, length (u16 LE), text.
    pub fn save(&self) -> Vec<u8> {
        let mut out = b"CMT1".to_vec();
        for t in 0..2 {
            for s in 0..SLOTS {
                if self.edited[t][s] {
                    let b = self.slots[t][s].as_str().as_bytes();
                    out.push(t as u8);
                    out.push(s as u8);
                    out.extend_from_slice(&(b.len() as u16).to_le_bytes());
                    out.extend_from_slice(b);
                }
            }
        }
        out
    }

    /// A project's typed slots (as `save` wrote them); the slots it did not type into go back to
    /// the text files' words. Returns false if the data is not ours.
    pub fn load(&mut self, data: &[u8]) -> bool {
        if data.len() < 4 || &data[..4] != b"CMT1" {
            return false;
        }
        for t in 0..2 {
            for s in 0..SLOTS {
                if self.edited[t][s] {
                    self.slots[t][s] = self.original[t][s];
                    self.edited[t][s] = false;
                    self.generation[t] += 1;
                }
            }
        }
        let mut i = 4;
        while i + 4 <= data.len() {
            let (t, s) = (data[i] as usize, data[i + 1] as usize);
            let n = u16::from_le_bytes([data[i + 2], data[i + 3]]) as usize;
            i += 4;
            if i + n > data.len() || t > 1 || s >= SLOTS {
                break;
            }
            let text = String::from_utf8_lossy(&data[i..i + n]);
            self.slots[t][s].set(&text);
            self.edited[t][s] = true;
            self.generation[t] += 1;
            i += n;
        }
        true
    }
}
