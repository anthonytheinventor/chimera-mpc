#!/usr/bin/env python3
"""make_readme_images.py - draws the pictures in README.md (docs/images/).

    python "plugins/CHIMERA MPC/tools/make_readme_images.py" [--only banner,face,screens,hero] [--no-engine]

Everything is drawn with make_skin.py's own code (its Ink and glow, the head from build/face.json,
the Hershey captions) and every screen is drawn from the skin's own TUI entries by its Previewer,
so the pictures match the plugin. Nothing in skins/ is written.

Reads   build/params.json, build/face.json   (as make_skin.py does)
Writes  docs/images/banner.png      the head and the wordmark
        docs/images/hero.gif        the SING page singing lyrics line 1, 20 pictures a second
        docs/images/face.png        the head's moving layers, named
        docs/images/<page>.png      the other nine pages (play, vowel, speak, sounds, beatbox, kit, notes, words, body)

The moving pictures are the engine's own: a small program (its source is at the end of this file)
is built against engine/ in the temp folder, sings, speaks and plays a beat, and reads the face's
frames (cm_face, cm_display, cm_step_lit) and NOW / NEXT (cm_now_next) every 50 ms, as the plugin does. Set CARGO_TARGET_DIR to
choose where it builds. With --no-engine (or when cargo fails) the hero cycles the check postures
in build/face.json instead and the screens show the face at rest.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile

from PIL import Image, ImageChops

HERE = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, HERE)
import make_skin as M  # noqa: E402

G = M.G
OUT = os.path.join(M.PROJECT, "docs", "images")
WORK = os.path.join(tempfile.gettempdir(), "chimera-readme")
HERSHEY_LIGHT = os.path.join(os.path.dirname(M.HERSHEY), "AVHersheySimplexLight.otf")
LOOK_MS = 50                         # the plugin's screen looks at the engine about this often

# the tune the hero sings: lyrics line 1, one note a syllable
SING_NOTES = [57, 59, 60, 62, 64, 62, 60, 59, 57, 55]
SING_SYLLABLES = ["Hel", "lo", "there,", "I", "am", "a", "voice", "made", "of", "air."]
SING_JOINED = {0}                    # syllables joined to the next one (no space after)


# --------------------------------------------------------------------------------------------
# The engine's frames
# --------------------------------------------------------------------------------------------
def engine_frames(use_engine=True):
    """{"sing": {"notes": [...], "rows": [...]}, "speak": ..., "beat": ...}; a row is jaw, tongue,
    upper lip, lower lip, larynx, sound, soft palate, pad heard, its count, square lit, its count.
    None when the engine cannot be run."""
    if not use_engine or not shutil.which("cargo"):
        return None
    crate = os.path.join(WORK, "face-frames")
    os.makedirs(os.path.join(crate, "src"), exist_ok=True)
    engine = os.path.join(M.PROJECT, "engine").replace("\\", "/")
    with open(os.path.join(crate, "Cargo.toml"), "w", encoding="utf-8", newline="\n") as f:
        f.write('[package]\nname = "face-frames"\nversion = "0.0.0"\nedition = "2024"\n\n'
                '[dependencies]\nchimera-mpc-engine = { path = "%s" }\n\n[workspace]\n' % engine)
    with open(os.path.join(crate, "src", "main.rs"), "w", encoding="utf-8", newline="\n") as f:
        f.write(FACE_FRAMES_RS)
    lock = os.path.join(M.PROJECT, "engine", "Cargo.lock")
    if os.path.isfile(lock) and not os.path.isfile(os.path.join(crate, "Cargo.lock")):
        shutil.copyfile(lock, os.path.join(crate, "Cargo.lock"))
    env = dict(os.environ)
    env.setdefault("CARGO_TARGET_DIR", os.path.join(WORK, "target"))
    argv = ["cargo", "run", "--release", "--quiet", "--manifest-path", os.path.join(crate, "Cargo.toml"),
            "--", os.path.join(M.PROJECT, "Content")]
    print("make_readme_images: the engine's frames (%s)" % " ".join(argv[:4]), flush=True)
    r = subprocess.run(argv, env=env, stdout=subprocess.PIPE, text=True)
    if r.returncode != 0:
        print("make_readme_images: the engine program failed; using the check postures")
        return None
    return json.loads(r.stdout.strip().splitlines()[-1])


# --------------------------------------------------------------------------------------------
# The skin in memory (as make_skin.main builds it, writing nothing)
# --------------------------------------------------------------------------------------------
class Live(M.Previewer):
    """the skin's previewer with some parameters moved off their defaults: over = {name: 0..1},
    and the texts of read-only text parameters: texts = {name: text}"""
    over = {}
    texts = {}

    def value(self, handle):
        p = self.param(handle)
        if p and p.name in self.over:
            return self.over[p.name]
        return super().value(handle)

    def text(self, handle):
        p = self.param(handle)
        if p and p.name in self.texts:
            return self.texts[p.name]
        return super().text(handle)


class Skin:
    def __init__(self):
        for f in (M.PARAMS, M.FACE, M.HERSHEY):
            if not os.path.isfile(f):
                sys.exit("make_readme_images: %s not found" % f)
        self.plist = G.load_params(M.PARAMS)
        self.params = {p.name: p for p in self.plist}
        with open(M.PARAMS, encoding="utf-8") as f:
            raw = {p["name"]: p for p in json.load(f)["params"]}
        units = {n: M.UNIT_OVERRIDE.get(n, M.UNITS.get(raw[n].get("label", ""), raw[n].get("label", "").upper())) for n in raw}
        with open(M.FACE, encoding="utf-8") as f:
            self.face = json.load(f)
        M.DIAL_STRIPS = [M.dial_strip(c) for c in M.SLOTS]
        self.placements = M.make_face(self.face)
        self.placements["velum_frames"] = len(self.face["velum"])
        M.put("puck.png", M._puck())
        tui, _, _ = M.build_tui(self.params, units, self.placements)
        self.pv = Live(tui, {p.index: p for p in self.plist})
        self.frames = {"Jaw Pos": len(self.face["jaw"]), "Tongue Pos": len(self.face["tongue"]),
                       "Lips Pos": len(self.face["upper"]), "Lower Lip": len(self.face["lower"][0]),
                       "Larynx Pos": len(self.face["larynx"]), "Sound Lvl": M.SOUND_FRAMES,
                       "Velum Pos": len(self.face["velum"])}

    def choice(self, name, k):
        return k / float(len(self.params[name].choices) - 1)

    def face_over(self, row):
        """a row of the engine's frames (jaw, tongue, upper, lower, larynx, sound, soft palate) as values"""
        names = ["Jaw Pos", "Tongue Pos", "Lips Pos", "Lower Lip", "Larynx Pos", "Sound Lvl", "Velum Pos"]
        return {n: row[i] / float(self.frames[n] - 1) for i, n in enumerate(names)}

    def page(self, name, over=None, texts=None):
        k = [pg["name"] for pg in M.PAGES].index(name)
        self.pv.over = over or {}
        self.pv.texts = texts or {}
        return self.pv.draw(self.pv.tabs[k]).convert("RGB")


def posture_row(face, name, sound=0):
    jaw, tongue, upper, lower, larynx, velum = face["postures"][name]["frames"]
    return [jaw, tongue, upper, lower, larynx, sound, velum]


# --------------------------------------------------------------------------------------------
# The head on its own, at any size (the banner and the layers picture)
# --------------------------------------------------------------------------------------------
def head_picture(face, row, scale, w, h, left=True):
    """The head with the layers at `row`'s frames, on black; and where its handles are (px)."""
    saved = (M.FACE_SCALE, M.HEAD_BOX, M.FACE_LEFT)
    M.FACE_SCALE, M.HEAD_BOX, M.FACE_LEFT = scale, (0, 0, w, h), left
    try:
        jaw, tongue, upper, lower, larynx, sound, velum = row
        fp = M.FaceParts(face)
        fan, _ = M.fan_lines(face)
        fj, ft, fu = face["jaw"][jaw], face["tongue"][tongue], face["upper"][upper]
        fl, fy, fv = face["lower"][jaw // 2][lower], face["larynx"][larynx], face["velum"][velum]
        layers = [M.layer_image(lambda ink: M.draw_static(ink, fp)),
                  M.layer_image(lambda ink: M.draw_fan(ink, fan, sound)),
                  M.layer_image(lambda ink: M.draw_larynx(ink, fy, fp)),
                  M.layer_image(lambda ink: M.draw_tongue(ink, ft, fp), M.tongue_fill(ft)),
                  M.layer_image(lambda ink: M.draw_jaw(ink, fj, fp)),
                  M.layer_image(lambda ink: M.draw_jaw_inset(ink, fj, fp)),
                  M.layer_image(lambda ink: M.draw_velum(ink, fv, fp)),
                  M.layer_image(lambda ink: M.draw_upper(ink, fu, fp)),
                  M.layer_image(lambda ink: M.draw_lower(ink, fl, fp))]
        img = Image.new("RGBA", (w, h), (0, 0, 0, 255))
        for layer in layers:
            img.alpha_composite(layer)
        throat = min(fan, key=lambda ln: ln[0][1])
        mouth = max(fan, key=lambda ln: ln[0][0])
        at = {"jaw": M.hp(fj["handles"]["jaw"]), "tongue": M.hp(ft["handles"]["tongue"]),
              "lips": M.hp(fl["handles"]["lips"]), "velum": M.hp(fv["handles"]["velum"]),
              "larynx": M.hp(fy["handles"]["larynx"]),
              "sound": M.hp([(throat[0][k] + throat[1][k]) / 2 for k in (0, 1)]),
              "mouth": M.hp(mouth[0])}
        return img.convert("RGB"), at
    finally:
        M.FACE_SCALE, M.HEAD_BOX, M.FACE_LEFT = saved


def over_black(base, ink):
    """the ink's glowing lines added to a picture on black"""
    return ImageChops.add(base, ink.render())


def banner(skin):
    w, h = 1280, 400
    row = posture_row(skin.face, "a", 5)
    head, _ = head_picture(skin.face, row, 20.0, 432, h, left=False)
    base = Image.new("RGB", (w, h), (0, 0, 0))
    base.paste(head, (8, 0))
    ink = M.Ink(w, h)
    # the page's own frame, and captions as the screens have them
    x0, x1 = 468, w - 40
    ink.rect(x0 - 22, 8, w - 8 - (x0 - 22), h - 16, M.FRAME, 1.0)
    ink.cap(x0, 30, "VOCAL TRACT", 15, M.BLUE, "l", 0.12)
    ink.cap(x0 + 150, 32, "ADULT MALE / MIDSAGITTAL", 12, M.QUIET, "l", 0.1)
    ink.cap(x1, 30, "FOR AKAI MPC", 15, M.BLUE, "r", 0.12)
    # the wordmark, as wide as the space
    px, track = 100.0, 0.2
    words = "CHIMERA MPC"
    while M.text_width(words, px, track, HERSHEY_LIGHT) > x1 - x0:
        px -= 1
    wy = 112
    ink.cap(x0, wy, "CHIMERA", px, M.BLUE, "l", track, path=HERSHEY_LIGHT)
    ink.cap(x1, wy, "MPC", px, M.AMBER, "r", track, path=HERSHEY_LIGHT)
    ink.line([(x0, wy + px * 1.32), (x1, wy + px * 1.32)], M.BLUE_DIM, 1.0)
    ink.cap(x0, wy + px * 1.32 + 16, "A VOICE MADE OF AIR: LUNGS, VOCAL FOLDS, THROAT, MOUTH AND NOSE", 15, M.LABEL, "l", 0.12)
    out = over_black(base, ink)
    # the four modes as the top bar's chips (Sing, the default, on), and the four encoder colors
    x, cy = x0, h - 92
    for k, (text, _, cw) in enumerate(M.MODE_CHIPS):
        chip = M.solid_button(cw, 40, M.BLUE, text, 16) if k == 0 else M.outline_button(cw, 40, M.BLUE, text, 16)
        tile = Image.new("RGBA", (cw, 40), (0, 0, 0, 255))
        tile.alpha_composite(chip)
        out.paste(tile.convert("RGB"), (x, cy))
        x += cw + 10
    dx = x1 - 4 * 62
    for k in range(4):
        strip = M.DIAL_STRIPS[k]
        fr = [9, 20, 14, 26][k]
        tile = Image.new("RGBA", (M.DIAL, M.DIAL), (0, 0, 0, 255))
        tile.alpha_composite(strip.crop((0, fr * M.DIAL, M.DIAL, (fr + 1) * M.DIAL)))
        out.paste(tile.convert("RGB"), (dx + k * 62, cy - 12))
    return out


def face_layers(skin):
    """the head with its moving layers named, each in its own color"""
    w, h = 1280, 580
    bw, bh = M.HEAD_BOX[2], M.HEAD_BOX[3]
    row = posture_row(skin.face, "a", 5)
    head, at = head_picture(skin.face, row, M.FACE_SCALE, bw, bh, left=True)
    hx, hy = (w - bw) // 2 + 10, 28
    base = Image.new("RGB", (w, h), (0, 0, 0))
    base.paste(head, (hx, hy))
    ink = M.Ink(w, h)
    ink.rect(6, 6, w - 12, h - 12, M.FRAME, 1.0)
    ink.cap(22, 18, "THE FACE", 15, M.BLUE, "l", 0.12)
    ink.cap(22 + 104, 20, "ONE STILL HEAD, ITS MOVING PARTS IN COLOR", 12, M.QUIET, "l", 0.1)
    labels = [  # (handle, title, color, two lines, side, y)
        ("lips", "LIPS", M.PINK, ["THE UPPER AND THE LOWER LIP,", "EACH ON ITS OWN"], "l", 236),
        ("tongue", "TONGUE", M.GREEN, ["28 OUTLINES TAKEN FROM", "THE ENGINE'S OWN TONGUE"], "l", 340),
        ("jaw", "JAW", M.RED, ["CARRIES THE LOWER TEETH", "AND THE LOWER LIP"], "l", 444),
        ("velum", "SOFT PALATE", M.WHITE, ["OPENS AND SHUTS THE WAY", "TO THE NOSE"], "r", 180),
        ("sound", "SOUND", M.AMBER, ["BRIGHTER AS THE VOICE GETS", "LOUDER, DARK IN SILENCE"], "r", 320),
        ("larynx", "LARYNX", M.BLUE, ["THE VOCAL FOLDS, MOVING", "UP AND DOWN"], "r", 460),
    ]
    for key, title, color, lines, side, y in labels:
        px, py = at[key][0] + hx, at[key][1] + hy
        if side == "l":
            tx, anchor, ex = 300, "r", 316
        else:
            tx, anchor, ex = w - 300, "l", w - 316
        ink.cap(tx, y, title, 22, color, anchor, 0.14)
        for k, ln in enumerate(lines):
            ink.cap(tx, y + 34 + k * 18, ln, 12, M.QUIET, anchor, 0.1)
        elbow = (ex + (44 if side == "l" else -44), y + 11)
        ink.line([(ex, y + 11), elbow, (px, py)], color, 1.0, 0.55)
        ink.circle(px, py, 3.0, color, fill=True)
    return over_black(base, ink)


# --------------------------------------------------------------------------------------------
# The screens and the hero
# --------------------------------------------------------------------------------------------
def lit_frame(rows, value, count, want=None):
    """the first look at which a new hit is reported (its count moved) and the voice sounds"""
    for i in range(1, len(rows)):
        if rows[i][count] != rows[i - 1][count] and rows[i][value] > 0 and (want is None or rows[i][value] == want):
            return i
    return None


def screens(skin, eng):
    mode = lambda k: skin.choice("Mode", k)   # noqa: E731
    rest = posture_row(skin.face, "rest")
    out = {}
    # Play with Breathe on: the voice holds its vowel at the knobs' posture
    held = dict(skin.face_over(rest[:5] + [4] + rest[6:]), Mode=mode(1), Breathe=1.0)
    out["play"] = skin.page("PLAY", held)
    out["vowel"] = skin.page("VOWEL", {"Mode": mode(1)})
    speak, said = rest, {"Now": "Hello.", "Next": "Can you hear me?"}
    if eng:
        rows = eng["speak"]["rows"]
        speak = max(rows, key=lambda r: (r[5], r[0]))
        said = now_next(eng["speak"], rows.index(speak)) or said
    out["speak"] = skin.page("SPEAK", dict(skin.face_over(speak), Mode=mode(2)), said)
    out["sounds"] = skin.page("SOUNDS", {"Mode": mode(2)})
    beat = {"Mode": mode(3), "Play Beat": 1.0}
    kit = dict(beat)
    if eng:
        rows = eng["beat"]["rows"]
        i = lit_frame(rows, 9, 10, 5) or lit_frame(rows, 9, 10)
        if i is not None:
            j = min(i + 1, len(rows) - 1)
            beat.update(skin.face_over(rows[j]))
            beat["Step Lit"] = skin.choice("Step Lit", rows[i][9])
        i = lit_frame(rows, 7, 8)
        if i is not None:
            kit["Hit"] = skin.choice("Hit", rows[i][7])
    out["beatbox"] = skin.page("BEATBOX", beat)
    out["kit"] = skin.page("KIT", kit)
    # NOTES with the Hum Bass pattern picked: its steps and hum notes
    hum = {"Mode": mode(3), "Beat": skin.choice("Beat", M.HUM_BASS[0])}
    for i, (sound, note) in enumerate(zip(M.HUM_BASS[1], M.HUM_BASS[2])):
        hum["Step %d" % (i + 1)] = skin.choice("Step %d" % (i + 1), 0 if sound == "." else 1 + M.PAD_SOUNDS.index(sound))
        hum["Note %d" % (i + 1)] = skin.choice("Note %d" % (i + 1), 1 + note - M.NOTE_LOW if note else 0)
    out["notes"] = skin.page("NOTES", hum)
    out["words"] = skin.page("WORDS", {"Mode": mode(2)})
    out["body"] = skin.page("BODY")
    return out


def now_next(run, i):
    """the engine's NOW and NEXT at look i of a run, as the Now and Next parameters' texts"""
    texts = run.get("texts")
    if not texts or i >= len(texts):
        return None
    return {"Now": texts[i][0], "Next": texts[i][1]}


def lyric_strip(w, h, now):
    """the lyrics line under the hero: the syllable being sung in amber"""
    ink = M.Ink(w, h)
    px, track = 22, 0.1
    gap = M.text_width("n", px, track)
    widths = [M.text_width(s.upper(), px, track) for s in SING_SYLLABLES]
    total = sum(widths) + sum(gap * (0.25 if k in SING_JOINED else 1.0) for k in range(len(widths) - 1))
    x = (w - total) / 2
    for k, s in enumerate(SING_SYLLABLES):
        if k == now:
            ink.cap(x, (h - px) / 2, s.upper(), px, M.core(M.AMBER), "l", track)
        else:
            ink.cap(x, (h - px) / 2, s.upper(), px, M.BLUE_MID if k < now or now < 0 else M.BLUE_DIM, "l", track, 0.9)
        x += widths[k] + gap * (0.25 if k in SING_JOINED else 1.0)
    return ink.render()


def hero(skin, eng):
    w = 960
    sh = int(round(M.H * w / float(M.W)))
    strip_h = 52
    frames = []
    if eng:
        rows, notes = eng["sing"]["rows"], eng["sing"]["notes"]
        strips = {}
        for i, row in enumerate(rows):
            now = max([k for k, n in enumerate(notes) if n <= i] or [-1])
            if row[5] == 0 and now == len(notes) - 1:
                now = -1          # the line is over
            if now not in strips:
                strips[now] = lyric_strip(w, strip_h, now)
            page = skin.page("SING", skin.face_over(row), now_next(eng["sing"], i)).resize((w, sh), Image.LANCZOS)
            f = Image.new("RGB", (w, sh + strip_h), (0, 0, 0))
            f.paste(page, (0, 0))
            f.paste(strips[now], (0, sh))
            frames.append((f, LOOK_MS))
    else:
        for name, sound in (("rest", 0), ("a", 5), ("i", 4), ("u", 4), ("p", 0)):
            page = skin.page("SING", skin.face_over(posture_row(skin.face, name, sound))).resize((w, sh), Image.LANCZOS)
            frames.append((page, 600))
    # one palette for every frame, no dithering, so only what moves is stored
    step = max(1, len(frames) // 12)
    sheet = Image.new("RGB", (w, frames[0][0].height * len(frames[::step])))
    for k, (f, _) in enumerate(frames[::step]):
        sheet.paste(f, (0, k * f.height))
    pal = sheet.quantize(255, method=Image.Quantize.MEDIANCUT, dither=Image.Dither.NONE)
    q = [f.quantize(palette=pal, dither=Image.Dither.NONE) for f, _ in frames]
    path = os.path.join(OUT, "hero.gif")
    q[0].save(path, save_all=True, append_images=q[1:], duration=[d for _, d in frames], loop=0, optimize=False, disposal=1)
    return path, len(frames)


def main():
    ap = argparse.ArgumentParser(description="Draws the README's pictures.")
    ap.add_argument("--only", default="banner,face,screens,hero")
    ap.add_argument("--no-engine", action="store_true", help="do not build and run the engine program")
    args = ap.parse_args()
    only = set(args.only.split(","))
    os.makedirs(OUT, exist_ok=True)
    os.makedirs(WORK, exist_ok=True)
    eng = engine_frames(not args.no_engine) if only & {"screens", "hero"} else None
    skin = Skin()
    if "banner" in only:
        banner(skin).save(os.path.join(OUT, "banner.png"), optimize=True)
    if "face" in only:
        face_layers(skin).save(os.path.join(OUT, "face.png"), optimize=True)
    if "screens" in only:
        for name, img in screens(skin, eng).items():
            img.save(os.path.join(OUT, name + ".png"), optimize=True)
    if "hero" in only:
        path, n = hero(skin, eng)
        print("make_readme_images: hero.gif %d pictures, %.2f MB" % (n, os.path.getsize(path) / 1e6))
    print("make_readme_images: pictures in %s" % OUT)


# --------------------------------------------------------------------------------------------
# The engine program: sings lyrics line 1, says speech 1's two lines and plays the beat, and
# prints the face's frames at every look as one line of JSON
# --------------------------------------------------------------------------------------------
FACE_FRAMES_RS = r'''
use chimera_mpc_engine::*;
use std::ffi::CString;

const FS: f64 = 44100.0;
const LOOK: usize = 2205; // 50 ms

struct Rig {
    h: *mut ChimeraMpc,
    q: Vec<f32>,
    n: usize,
    next: usize,
    rec: bool,
    rows: Vec<[i32; 11]>,
    texts: Vec<(String, String)>,
    notes: Vec<usize>,
}

impl Rig {
    fn new(dir: &str) -> Rig {
        let dir = CString::new(dir).unwrap();
        let h = cm_new(FS, dir.as_ptr(), std::ptr::null());
        let mut q = vec![0.0f32; p::COUNT];
        cm_param_defaults(q.as_mut_ptr(), p::COUNT as i32);
        Rig { h, q, n: 0, next: 0, rec: false, rows: Vec::new(), texts: Vec::new(), notes: Vec::new() }
    }

    fn render(&mut self, seconds: f64) {
        let mut buf = [0.0f32; 128];
        for _ in 0..(seconds * FS / 128.0).round() as usize {
            cm_set_params(self.h, self.q.as_ptr(), p::COUNT as i32);
            cm_render(self.h, buf.as_mut_ptr(), 128);
            let (mut f, mut d, mut s) = ([0i32; 6], [0i32; 4], [0i32; 2]);
            cm_face(self.h, f.as_mut_ptr());
            cm_display(self.h, d.as_mut_ptr());
            cm_step_lit(self.h, s.as_mut_ptr());
            self.n += 128;
            if self.rec && self.n >= self.next {
                self.next += LOOK;
                self.rows.push([f[0], f[1], f[2], f[3], f[4], f[5], d[1], d[2], d[3], s[0], s[1]]);
                let mut t = [0u8; 64];
                cm_now_next(self.h, t.as_mut_ptr(), 64);
                let t = std::ffi::CStr::from_bytes_until_nul(&t).unwrap().to_string_lossy().into_owned();
                let (now, next) = t.split_once('\n').unwrap_or(("", ""));
                self.texts.push((now.to_string(), next.to_string()));
            }
        }
    }

    fn ready(&mut self) {
        for _ in 0..800 {
            self.render(0.025);
            if cm_lyrics_ready(self.h) == 1 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(25));
        }
        self.render(0.5);
        self.rec = true;
        self.next = self.n;
    }

    fn note(&mut self, note: i32, on: f64, off: f64) {
        self.notes.push(self.rows.len());
        cm_note_on(self.h, note, 100);
        self.render(on);
        cm_note_off(self.h, note);
        self.render(off);
    }

    fn json(&self) -> String {
        let rows: Vec<String> = self.rows.iter().map(|r| format!("{:?}", r)).collect();
        let texts: Vec<String> = self.texts.iter().map(|(a, b)| format!("[{:?}, {:?}]", a, b)).collect();
        format!("{{\"notes\": {:?}, \"rows\": [{}], \"texts\": [{}]}}", self.notes, rows.join(", "), texts.join(", "))
    }
}

fn main() {
    let dir = std::env::args().nth(1).expect("the Content folder");
    // Sing: lyrics line 1 on a little tune
    let mut sing = Rig::new(&dir);
    sing.q[p::MODE] = 0.0;
    sing.q[p::LYRICS] = 0.0;
    sing.ready();
    sing.render(0.3);
    for note in [57, 59, 60, 62, 64, 62, 60, 59, 57, 55] {
        sing.note(note, 0.4, 0.05);
    }
    sing.render(0.75);
    // Speak: speech 1, "Hello." and "Can you hear me?"
    let mut speak = Rig::new(&dir);
    speak.q[p::MODE] = 2.0;
    speak.ready();
    speak.note(60, 0.1, 1.3);
    speak.note(60, 0.1, 2.0);
    // Beatbox: the default beat, one bar and a little more
    let mut beat = Rig::new(&dir);
    beat.q[p::MODE] = 3.0;
    beat.ready();
    beat.q[p::PLAY_BEAT] = 1.0;
    beat.render(3.0);
    println!("{{\"sing\": {}, \"speak\": {}, \"beat\": {}}}", sing.json(), speak.json(), beat.json());
    for r in [sing, speak, beat] {
        cm_free(r.h);
    }
}
'''

if __name__ == "__main__":
    main()
