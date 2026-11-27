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
/// The preview: two rows of up to ROW_CHARS characters (and the cursor), broken at spaces. A row
/// is one parameter's display text, which the SDK framework keeps to 31 bytes.
pub const ROW_CHARS: usize = 29;
const ROW_BYTES: usize = 31;
/// The preview buffer: two rows and the break between them.
pub const PREVIEW_BYTES: usize = 2 * ROW_BYTES + 2;
/// The cursor: a thin bar, about as wide as a space.
const CURSOR: char = '|';

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

    /// The byte position `from_end` characters before the end (the cursor; clamped to the start).
    fn pos_from_end(&self, from_end: usize) -> usize {
        let mut pos = self.len;
        for _ in 0..from_end {
            if pos == 0 {
                break;
            }
            pos -= 1;
            while pos > 0 && self.bytes[pos] & 0xC0 == 0x80 {
                pos -= 1;
            }
        }
        pos
    }

    /// Put byte `b` (ASCII) in at byte position `pos`.
    fn insert(&mut self, pos: usize, b: u8) {
        if self.len < TEXT_CAP && pos <= self.len {
            self.bytes.copy_within(pos..self.len, pos + 1);
            self.bytes[pos] = b;
            self.len += 1;
        }
    }

    /// Take out the character before byte position `pos` (all of its bytes).
    fn remove_before(&mut self, pos: usize) {
        if pos == 0 || pos > self.len {
            return;
        }
        let mut start = pos - 1;
        while start > 0 && self.bytes[start] & 0xC0 == 0x80 {
            start -= 1;
        }
        self.bytes.copy_within(pos..self.len, start);
        self.len -= pos - start;
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
    /// Type key `k` (the Key parameter's step) into a slot at the cursor, `from_end` characters
    /// before the end (0: at the end). Typing and Del keep the cursor where it is counted from the
    /// end. Allocation-free.
    pub fn key(&mut self, target: usize, slot: usize, k: usize, from_end: usize) {
        let (t, s) = (target.min(1), slot.min(SLOTS - 1));
        let lines = target == TARGET_SPEECH;
        let sl = &mut self.slots[t][s];
        let pos = sl.pos_from_end(from_end);
        match k {
            1..=26 => sl.insert(pos, b'a' + (k - 1) as u8),
            27..=32 => sl.insert(pos, PUNCT[k - 27]),
            KEY_DEL => sl.remove_before(pos),
            // a new line: in Speech, what the next note says; in lyrics, a space
            KEY_LINE => sl.insert(pos, if lines { b'\n' } else { b' ' }),
            KEY_CLEAR => sl.len = 0,
            _ => return,
        }
        self.edited[t][s] = true;
        self.generation[t] += 1;
    }

    /// The slot around its cursor (`from_end` characters before the end) as two rows into `out`:
    /// the text wrapped at spaces into rows of up to ROW_CHARS characters (a line break shown as
    /// "/"), the row with the cursor and the one before it, a "\n" between them, and the cursor
    /// as a thin bar. Returns the length. Allocation-free (fixed arrays on the stack).
    pub fn preview(&self, target: usize, slot: usize, from_end: usize, out: &mut [u8; PREVIEW_BYTES]) -> usize {
        let text = self.slots[target.min(1)][slot.min(SLOTS - 1)].as_str();
        let mut chars = ['\0'; TEXT_CAP];
        let mut n = 0;
        for ch in text.chars().take(TEXT_CAP) {
            chars[n] = if ch == '\n' { '/' } else { ch };
            n += 1;
        }
        let at = n - from_end.min(n);
        // the rows: each starts where the last ended, breaks after its last space that fits
        let mut starts = [0usize; TEXT_CAP + 2];
        let mut rows = 1;
        let mut s = 0;
        while n - s > ROW_CHARS {
            let mut end = s + ROW_CHARS;
            if let Some(sp) = (s + 1..=end).rev().find(|&i| chars[i - 1] == ' ') {
                end = sp;
            }
            starts[rows] = end;
            rows += 1;
            s = end;
        }
        starts[rows] = n;
        // the cursor's row (at the very end: the last row), and the one before it
        let k = if at >= n { rows - 1 } else { (0..rows).rev().find(|&r| starts[r] <= at).unwrap_or(0) };
        let first = k.saturating_sub(1);
        // one character into the row (dropped if the row or the buffer is full)
        fn put(ch: char, out: &mut [u8; PREVIEW_BYTES], len: &mut usize, row_len: &mut usize) {
            let mut b = [0u8; 4];
            let e = ch.encode_utf8(&mut b);
            if *row_len + e.len() <= ROW_BYTES && *len + e.len() < out.len() {
                out[*len..*len + e.len()].copy_from_slice(e.as_bytes());
                *len += e.len();
                *row_len += e.len();
            }
        }
        let mut len = 0;
        let mut row_len = 0;
        for r in first..=(first + 1).min(rows - 1) {
            if r > first {
                out[len] = b'\n';
                len += 1;
                row_len = 0;
            }
            for i in starts[r]..starts[r + 1] {
                if i == at {
                    put(CURSOR, out, &mut len, &mut row_len);
                }
                put(chars[i], out, &mut len, &mut row_len);
            }
            if at == n && r == rows - 1 {
                put(CURSOR, out, &mut len, &mut row_len);
            }
        }
        len
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
