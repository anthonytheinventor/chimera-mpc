#!/usr/bin/env python3
"""make_skin.py - draws CHIMERA MPC's screens and writes its hand-made MPC skin (user interface, version 2).

    python "plugins/CHIMERA MPC/tools/make_skin.py"

Reads   build/params.json   (written by the build; run build_chimera.py first)
        build/face.json     (the tract and the head around it: cargo run --release --example dump_face in engine/)
Writes  skins/TUI.json, skins/Q-Links.json and every PNG they use
        build/preview/page_N_<name>.png   (each page at the defaults, drawn from TUI.json itself)

The layout is build-tmp/chimera-mpc/ui-v2/design.md (approved 2026-11-26), mirrored left to right:
the Q-Link encoders are on the LEFT and the face screen on the RIGHT, because the MPC's Q-Link
window slides in from the right (x >= ~1025) and must not cover controls. The top bar is mirrored
the same way: the page's main button and chips at the left, the four mode chips in the middle,
the CHIMERA wordmark at the right.

The 16 encoders are grouped as CHIMERA draws its pages of four: one bank per group, green, red,
pink and white. GROUPING picks rows (the approved look: each row is one page of four, Bank
Direction RowTopDown) or columns (each column one page of four, Bank Direction Column).

Ink: everything with a glow is a baked PNG (Hershey Simplex captions, dials, the head); everything
without a glow is text the MPC draws itself (Roboto Light / Regular Labels).

The face is step 2 of the design (B.6): the static head (skin, skull, brain, eye, nasal cavity,
palate, vertebrae) from CHIMERA's head template, ported into dump_face.rs so it shares the engine's
coordinates, and over it independent transparent layers, each a film strip on its own read-only
parameter, bottom to top: the amber sound fan ("Sound Lvl", 6 steps), the larynx ("Larynx Pos"),
the tongue ("Tongue Pos": real outlines clustered by engine/examples/fit_face.rs, its inside opaque
black so the fan's lines end at its surface), the jaw ("Jaw Pos", drawn over the tongue), the soft
palate ("Velum Pos"), the upper lip ("Lips Pos") and the lower lip ("Lower Lip": one strip per
pair of jaw frames, shown with RangedEnabling while Jaw Pos is at either, so the lower lip sits on
the jaw that is shown, at most half a jaw step off). The engine picks every frame
(engine/src/face.rs); the old "Face" parameter stays, unused.

No Max[...] handles (they crashed the MPC One+ in French 36).
"""

import argparse
import hashlib
import json
import math
import os
import re
import sys

from PIL import Image, ImageChops, ImageDraw, ImageFilter, ImageFont, ImageMath

HERE = os.path.dirname(os.path.abspath(__file__))
PROJECT = os.path.dirname(HERE)
SDK = os.path.dirname(os.path.dirname(PROJECT))
CHIMERA = os.path.abspath(os.path.join(SDK, "..", "chimera-vocal-synth"))
sys.path.insert(0, os.path.join(SDK, "tools"))
import gen_ui as G  # noqa: E402  (the SDK's generator: its JSON building blocks)

SKINS = os.path.join(PROJECT, "skins")
PREVIEW = os.path.join(PROJECT, "build", "preview")
PARAMS = os.path.join(PROJECT, "build", "params.json")
FACE = os.path.join(PROJECT, "build", "face.json")
HERSHEY = os.path.join(CHIMERA, "public", "fonts", "AVHersheySimplexMedium.otf")
WIN_FONTS = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts")
USER_FONTS = os.path.join(os.environ.get("LOCALAPPDATA", ""), "Microsoft", "Windows", "Fonts")


def find_font(name, fallback):
    for d in (USER_FONTS, WIN_FONTS):
        if os.path.isfile(os.path.join(d, name)):
            return os.path.join(d, name)
    return os.path.join(WIN_FONTS, fallback)


# Hershey Simplex has no IPA and no "%": phonetic symbols are baked in Segoe UI, "%" in Roboto.
# Roboto Light / Regular stand in for the MPC's live Labels in the previews.
SEGOE = os.path.join(WIN_FONTS, "segoeui.ttf")
ROBOTO_LIGHT = find_font("Roboto-Light.ttf", "segoeuil.ttf")
ROBOTO = find_font("Roboto-Regular.ttf", "segoeui.ttf")

# --------------------------------------------------------------------------------------------
# The one switch for the encoder grouping: "rows" (approved: each row one of CHIMERA's pages of
# four, Bank Direction RowTopDown) or "columns" (each column one page of four, Bank Direction Column)
# --------------------------------------------------------------------------------------------
GROUPING = "columns"   # the owner, 2026-11-26: the Q-Link window shows a column

# --------------------------------------------------------------------------------------------
# Geometry (the MPC's tab area is 1280 x 628). Mirrored from design.md B.3.
# --------------------------------------------------------------------------------------------
W, H = 1280, 628
SS = 2                                   # supersampling for baked art
TOP, BOT = 52, 622                       # where the screens start and end
GRID_X, GRID_W = 12, 658                 # the encoder block, LEFT
SX, SW = 680, 588                        # the right screen (face, vowel chart, consonants, pads)
SIDEBAR_X = 1025                         # the Q-Link window covers x >= ~1025
DIAL = 64                                # dial film-strip frame (ring radius 21, ticks to 29)
KNOB_FRAMES = 33
HEAD_BOX = (SX + 2, TOP + 28, SW - 4, BOT - TOP - 30)   # where the head is drawn: 682 80 584 540
FACE_SCALE = 27.0                        # px per cm
VIEW_X0, VIEW_Y1 = -6.2, 18.6            # body cm at HEAD_BOX's top-left corner (before mirroring)
FACE_LEFT = True                         # the head faces left, in toward the controls (the screen is on the right)
MAIN_BTN = (12, 4, 150, 40)              # the page's main button (amber)
PAGE_CHIPS_X = 170                       # the page's own chips start here
MODE_CHIPS = [("SING", 552, 104), ("PLAY", 664, 104), ("SPEAK", 776, 104), ("BEATBOX", 888, 128)]


# --------------------------------------------------------------------------------------------
# CHIMERA's palette (src/ui/colors.ts) and design.md B.9
# --------------------------------------------------------------------------------------------
def hexrgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


BLUE, BLUE_MID, BLUE_DIM, FRAME = hexrgb("4fb6f2"), hexrgb("3487c4"), hexrgb("1f5f8c"), hexrgb("133954")
GREEN, RED, PINK, WHITE, AMBER = hexrgb("5be381"), hexrgb("ff5d40"), hexrgb("ff73cf"), hexrgb("eef6f8"), hexrgb("ffb340")
SLOTS = [GREEN, RED, PINK, WHITE]
SLOT_NAMES = ["green", "red", "pink", "white"]
INK, LABEL, QUIET = hexrgb("effbff"), hexrgb("a9dcf8"), hexrgb("7cbfe6")
BTN_TEXT = (29, 26, 16)
ARGB_BG = "ff000000"
ARGB_OVERLAY = "ff060a10"


def argb(rgb, a=255):
    return "%02x%02x%02x%02x" % ((a,) + tuple(rgb))


def mix(a, b, u):
    return tuple(int(round(x * (1 - u) + y * u)) for x, y in zip(a, b))


def core(c):
    return mix(c, (255, 255, 255), 0.55)


def light(c):
    return mix(c, (255, 255, 255), 0.3)


def dim(c, o):
    return tuple(int(round(v * o)) for v in c)


# --------------------------------------------------------------------------------------------
# Pages (design.md B.2 / B.4 / B.5). "groups" are CHIMERA's pages of four: (title, [4 names]).
# --------------------------------------------------------------------------------------------
SPEAK_GROUPS = [("SPEAK", ["Speech Rate", "Start Line", "Speech", "IPA"]), ("VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
                ("TONE", ["Breathy", "Rough", "Fry", "Ring"]), ("SHAPE", ["Jaw", "Lips Open", "Larynx", "Soft Palate"])]
PAGES = [
    {"name": "SING", "fn": 0, "sub": 0, "right": "face", "sub_word": "SINGING", "readouts": True,
     "groups": [("SONG", ["Vibrato", "Vib Rate", "Breathy", "Ring"]), ("VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
                ("TONE", ["Rough", "Fry", "Twang", "False Folds"]), ("LYRICS", ["Lyrics", "Start Syl", "Restart", "Gain"])]},
    {"name": "PLAY", "fn": 1, "sub": 0, "right": "face", "sub_word": "MIDSAGITTAL", "readouts": True,
     "chips": [("WHISPER", "Whisper", 116)], "main": ("BREATHE", "Breathe"),
     "groups": [("VOICE", ["Pitch", "Breath", "Pressed", "Vibrato"]), ("TONE", ["Bright", "Breathy", "Rough", "Fry"]),
                ("MOUTH", ["Rounding", "Lips Open", "Protrude", "Jaw"]), ("THROAT", ["Soft Palate", "Larynx", "Twang", "False Folds"])]},
    {"name": "VOWEL", "fn": 1, "sub": 1, "right": "vowel",
     "chips": [("WHISPER", "Whisper", 116)], "main": ("BREATHE", "Breathe"),
     "groups": [("VOWEL", ["Front", "Open", "Soft Palate", "Rounding"]), ("TONGUE", ["Tongue Fwd", "Tongue Up", "Tip Fwd", "Tip Up"]),
                ("MORE", ["Tongue Root", "Tip Curl", "Glide", "Vib Rate"]), ("VOICE", ["Pitch", "Breath", "Pressed", "Bright"])]},
    {"name": "SPEAK", "fn": 2, "sub": 0, "right": "face", "sub_word": "SPEAKING", "readouts": True,
     "chips": [("IPA", "IPA", 80)], "main": ("SPEAK", "Speak"), "groups": SPEAK_GROUPS},
    {"name": "SOUNDS", "fn": 2, "sub": 1, "right": "sounds",
     "chips": [("IPA", "IPA", 80)], "main": ("SPEAK", "Speak"), "groups": SPEAK_GROUPS},
    {"name": "BEATBOX", "fn": 3, "sub": 0, "right": "face", "sub_word": "BEATBOX", "left": "steps",
     "chips": [("MIC FX", "Mic FX", 96), ("FOLLOW", "Follow Host", 104)], "main": ("PLAY BEAT", "Play Beat")},
    {"name": "KIT", "fn": 3, "sub": 1, "right": "pads",
     "chips": [("MIC FX", "Mic FX", 96), ("FOLLOW", "Follow Host", 104)], "main": ("PLAY BEAT", "Play Beat"),
     "groups": [("BEAT", ["Beat", "Tempo", "Swing", "Mic FX"]), ("HUM VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
                ("TRANSPORT", ["Follow Host", "Play Beat", "Gain", None]), ("", [None, None, None, None])]},
    {"name": "WORDS", "fn": 4, "sub": 0, "left": "words", "main": ("SPEAK", "Speak"), "caption": "SAYS THE NEXT LINE"},
    {"name": "BODY", "fn": 5, "sub": 0, "right": "face", "sub_word": "MIDSAGITTAL", "credit": True, "body_chips": True,
     "groups": [("BODY", ["Body", "Female", "Child", "Gain"]), ("SETUP", ["MPE", "Restart", "Whisper", "Breathe"]),
                ("", [None, None, None, None]), ("", [None, None, None, None])]},
]
WORDS_ROW = ["Type Into", "Type Slot", "Speech Rate", "Start Line"]
BIPOLAR = {"Larynx", "Tongue Root"}
MOMENTARY = {"Speak"}      # back to off when the finger lifts: each tap acts once
BODIES = ["MALE", "FEMALE", "CHILD", "BLEND"]
BODY_CAPTIONS = ["ADULT MALE", "ADULT FEMALE", "CHILD", "BLEND"]
UNITS = {"ct": "CENTS", "cmH2O": "CMH2O", "dB": "DB", "x": "X"}
UNIT_OVERRIDE = {"Lyrics": "SLOT", "Speech": "SLOT", "Type Slot": "SLOT", "Start Syl": "SYLLABLE", "Start Line": "LINE"}
PADS = [("KICK", "B"), ("HAT", "t"), ("SNARE", "Pf"), ("RIM", "K"), ("INWARD", "^K"),
        ("CLICK", "!"), ("POP", "P!"), ("OPEN HAT", "tss"), ("CRASH", "kch"), ("HUM", "m")]
CONSONANTS = ["p", "b", "t", "d", "k", "g", "ʔ", "m", "n", "ŋ", "r", "ɾ", "f", "v", "θ", "ð",
              "s", "z", "ʃ", "ʒ", "x", "h", "ɹ", "j", "l"]
PAD_VOWELS = ["ɑ", "i", "u", "ə"]
# the vowel chart: (ipa, F1, F2) for the male body (CHIMERA's VowelRef targets), on the pad's ranges
VOWEL_CHART = [("i", 342, 2322), ("ɪ", 427, 2034), ("e", 476, 2089), ("ɛ", 580, 1799), ("æ", 640, 1760),
               ("a", 780, 1450), ("ɑ", 768, 1333), ("ɔ", 652, 997), ("o", 497, 910), ("ʊ", 469, 1122),
               ("u", 378, 997), ("ʌ", 623, 1200), ("ə", 500, 1450)]
F1_RANGE, F2_RANGE = (230.0, 950.0), (700.0, 2700.0)
VOWEL_BOX = (SX + 14, TOP + 34, SW - 28, 484)
# the keyboard (design.md B.5): rows of (label, Key step); the Key steps are "-", A..Z, Space, . , ? ! ', Del, Line, Clear
KEY_ROWS = [[(c, ord(c) - 64) for c in "QWERTYUIOP"], [(c, ord(c) - 64) for c in "ASDFGHJKL"] + [("'", 32)],
            [(c, ord(c) - 64) for c in "ZXCVBNM"] + [(",", 29), (".", 28), ("?", 30)]]
KEY_W, KEY_H, KEY_PITCH_X, KEY_PITCH_Y, KEYS_X, KEYS_Y = 118, 100, 126, 108, 14, 198
WIDE_KEYS = [("CLEAR", 35, 14, 180, RED), ("!", 31, 202, 118, None), ("SPACE", 27, 328, 562, LABEL),
             ("LINE /", 34, 898, 180, GREEN), ("DEL", 33, 1086, 180, WHITE)]
# previews only: what the live text labels show when their parameter's default text is empty
PREVIEW_SAMPLE = {"Typed": "hello. can you hear me?"}


# --------------------------------------------------------------------------------------------
# Baking: a supersampled canvas that gets CHIMERA's glow (design.md B.8, make_mockups.py's Page)
# --------------------------------------------------------------------------------------------
_fonts = {}


def font(path, px):
    key = (path, int(px))
    if key not in _fonts:
        _fonts[key] = ImageFont.truetype(path, int(px))
    return _fonts[key]


HERSHEY_MISSING = set("%<>@[]^_`{}~\\")     # not in Hershey Simplex: drawn in Roboto


def text_font(text):
    return HERSHEY if all(ord(ch) < 128 for ch in text) else SEGOE


class Ink:
    """A baked picture: lines drawn at 2x on black, then the glow; rgba() keys it out of the black."""

    def __init__(self, w, h):
        self.w, self.h = int(math.ceil(w)), int(math.ceil(h))
        self.big = Image.new("RGB", (self.w * SS, self.h * SS), (0, 0, 0))
        self.d = ImageDraw.Draw(self.big)
        self.post = []             # drawn after the glow (solid fills and the text on them)

    def line(self, pts, color, w=1.2, o=1.0, closed=False, draw=None):
        d = draw or self.d
        p = [(x * SS, y * SS) for x, y in pts]
        if closed and p:
            p.append(p[0])
        if len(p) >= 2:
            d.line(p, fill=dim(color, o), width=max(1, int(round(w * SS))), joint="curve")

    def hot(self, pts, color, w, o=1.0, closed=False, draw=None):
        """a bright beam: the line, then its whitish core"""
        self.line(pts, color, w, o, closed, draw)
        self.line(pts, core(color), w * 0.42, o, closed, draw)

    def rect(self, x, y, w, h, color, lw=1.0, o=1.0, fill=None):
        box = (x * SS, y * SS, (x + w) * SS - 1, (y + h) * SS - 1)
        if fill is not None:
            self.d.rectangle(box, fill=fill)
        self.d.rectangle(box, outline=dim(color, o), width=max(1, int(round(lw * SS))))

    def circle(self, cx, cy, r, color, lw=1.2, o=1.0, fill=False, draw=None):
        d = draw or self.d
        box = ((cx - r) * SS, (cy - r) * SS, (cx + r) * SS, (cy + r) * SS)
        if fill:
            d.ellipse(box, fill=dim(color, o))
        else:
            d.ellipse(box, outline=dim(color, o), width=max(1, int(round(lw * SS))))

    def cap(self, x, y, text, px, color, anchor="l", track=0.08, o=1.0, path=None):
        """a caption with tracking; (x, y) is the top of the text line. Returns its width."""
        path = path or text_font(text)
        f = font(path, px * SS)
        alt = font(ROBOTO, px * SS * 0.95)
        fonts = [alt if ch in HERSHEY_MISSING and path == HERSHEY else f for ch in text]
        widths = [ft.getlength(ch) for ft, ch in zip(fonts, text)]
        tr = track * px * SS
        total = sum(widths) + tr * (len(text) - 1)
        x0 = x * SS - (total if anchor == "r" else total / 2 if anchor == "m" else 0)
        for ch, wd, ft in zip(text, widths, fonts):
            self.d.text((x0, y * SS), ch, font=ft, fill=dim(color, o))
            x0 += wd + tr
        return total / SS

    def glyph(self, cx, cy, text, px, color, path=None):
        """one symbol centered on (cx, cy)"""
        path = path or text_font(text)
        if path == HERSHEY and any(ch in HERSHEY_MISSING for ch in text):
            path = ROBOTO
        self.d.text((cx * SS, cy * SS), text, font=font(path, px * SS), fill=color, anchor="mm")

    def fill_post(self, x, y, w, h, color):
        self.post.append(lambda d: d.rectangle((x, y, x + w - 1, y + h - 1), fill=color))

    def text_post(self, cx, y, text, px, color, track=0.1):
        def fn(d):
            f = font(HERSHEY, px)
            widths = [f.getlength(ch) for ch in text]
            x0 = cx - (sum(widths) + track * px * (len(text) - 1)) / 2
            for ch, wd in zip(text, widths):
                d.text((x0, y), ch, font=f, fill=color)
                x0 += wd + track * px
        self.post.append(fn)

    def clipped(self, polygon, draw_fn):
        """draw_fn(draw) on its own layer, kept only inside polygon (canvas px), added to the picture"""
        layer = Image.new("RGB", self.big.size, (0, 0, 0))
        draw_fn(ImageDraw.Draw(layer))
        mask = Image.new("L", self.big.size, 0)
        ImageDraw.Draw(mask).polygon([(x * SS, y * SS) for x, y in polygon], fill=255)
        self.big = ImageChops.add(self.big, Image.composite(layer, Image.new("RGB", layer.size, (0, 0, 0)), mask))
        self.d = ImageDraw.Draw(self.big)

    def render(self, glow=True):
        sharp = self.big.resize((self.w, self.h), Image.LANCZOS)
        if glow:
            b1 = sharp.filter(ImageFilter.GaussianBlur(1.1)).point(lambda v: int(v * 0.55))
            b2 = sharp.filter(ImageFilter.GaussianBlur(3.6)).point(lambda v: int(v * 0.6))
            sharp = ImageChops.add(ImageChops.add(sharp, b1), b2)
        d = ImageDraw.Draw(sharp)
        for fn in self.post:
            fn(d)
        return sharp

    def rgba(self, glow=True):
        return key_out(self.render(glow))


def key_out(rgb):
    """black -> transparent: alpha is the brightest channel, the color is divided back up, so the
    picture over the black page looks exactly as drawn"""
    r, g, b = rgb.split()
    a = ImageChops.lighter(ImageChops.lighter(r, g), b)

    def up(ch):
        return ImageMath.lambda_eval(lambda e: e["convert"](e["min"](e["c"] * 255 / e["max"](e["a"], 1), 255), "L"), c=ch, a=a)
    out = Image.merge("RGBA", (up(r), up(g), up(b), a))
    return out


# --------------------------------------------------------------------------------------------
# The picture registry (every PNG is drawn once; shared ones are reused by name)
# --------------------------------------------------------------------------------------------
IMAGES = {}


def put(name, img):
    if name not in IMAGES:
        IMAGES[name] = img
    return name


def slug(text):
    s = re.sub(r"[^a-z0-9]+", "_", text.lower()).strip("_")
    return s[:24] or "sym"


_baked = {}


def baked(key, w, h, fn, prefix="t"):
    """A cropped glowing picture. fn(ink, m) draws with its origin at (m, m). Returns
    (file, dx, dy): place the file at (x + dx, y + dy) to put the drawing's origin at (x, y)."""
    if key in _baked:
        return _baked[key]
    m = 8
    ink = Ink(w + 2 * m, h + 2 * m)
    fn(ink, m)
    img = ink.rgba()
    bb = img.getbbox() or (0, 0, 1, 1)
    img = img.crop(bb)
    name = "%s_%s_%s.png" % (prefix, slug(str(key[0]) if isinstance(key, tuple) else str(key)), hashlib.md5(repr(key).encode("utf-8")).hexdigest()[:6])
    put(name, img)
    _baked[key] = (name, bb[0] - m, bb[1] - m)
    return _baked[key]


def text_width(text, px, track, path=None):
    path = path or text_font(text)
    f = font(path, px * SS)
    alt = font(ROBOTO, px * SS * 0.95)
    widths = [(alt if ch in HERSHEY_MISSING and path == HERSHEY else f).getlength(ch) for ch in text]
    return (sum(widths) + track * px * SS * (len(text) - 1)) / SS


def caption(text, px, color, track=0.1, o=1.0, path=None):
    """(file, dx, dy, width) for a baked caption whose top-left is the origin"""
    w = text_width(text, px, track, path)
    f, dx, dy = baked(("cap", text, px, color, track, o, path), w + 4, px * 1.5,
                      lambda ink, m: ink.cap(m, m, text, px, color, "l", track, o, path))
    return f, dx, dy, w


# --------------------------------------------------------------------------------------------
# The face (design.md B.6, step 1)
# --------------------------------------------------------------------------------------------
def hp(p):
    """body cm -> px inside HEAD_BOX (mirrored when FACE_LEFT: the engine's profile faces right)"""
    x = (p[0] - VIEW_X0) * FACE_SCALE
    return (HEAD_BOX[2] - x if FACE_LEFT else x, (VIEW_Y1 - p[1]) * FACE_SCALE)


def chaikin(pts, it=1):
    for _ in range(it):
        if len(pts) < 3:
            return pts
        out = [pts[0]]
        for a, b in zip(pts, pts[1:]):
            out += [(0.75 * a[0] + 0.25 * b[0], 0.75 * a[1] + 0.25 * b[1]), (0.25 * a[0] + 0.75 * b[0], 0.25 * a[1] + 0.75 * b[1])]
        out.append(pts[-1])
        pts = out
    return pts


def runs_away_from(pts, avoid, r):
    """the stretches of pts farther than r from every point of avoid (each drawn as its own line)"""
    out, cur = [], []
    for p in pts:
        if min(math.hypot(p[0] - q[0], p[1] - q[1]) for q in avoid) > r:
            cur.append(p)
        elif cur:
            out.append(cur)
            cur = []
    if cur:
        out.append(cur)
    return out


WS = 0.97     # CHIMERA's mock-up line widths at 27 px per cm (its SVG is 32 px per cm, drawn 1.15x)


def handle_ring(ink, p, color, r, o=1.0):
    x, y = hp(p)
    t = 4
    ink.circle(x, y, r, color, 1.3, o)
    for a, b in (((x - r - t, y), (x - r + 2, y)), ((x + r - 2, y), (x + r + t, y)), ((x, y - r - t), (x, y - r + 2)), ((x, y + r - 2), (x, y + r + t))):
        ink.line([a, b], color, 1.2, o)


# The layers, bottom to top (design.md B.6; the jaw is drawn over the tongue so the tongue's opaque
# inside never hides the teeth or the jaw bone): (name, read-only parameter)
FACE_LAYERS = [("sound", "Sound Lvl"), ("larynx", "Larynx Pos"), ("tongue", "Tongue Pos"), ("jaw", "Jaw Pos"),
               ("jaw_inset", "Jaw Pos"), ("velum", "Velum Pos"), ("upper", "Lips Pos"), ("lower", "Lower Lip")]
SOUND_FRAMES = 6
SOUND_LEVELS = [0.0, 0.22, 0.38, 0.56, 0.78, 1.0]   # the fan's brightness per Sound Lvl step
FAN_MARGIN = 0.15                                    # cm the fan's lines reach into the lowest tongue


def static_prefix(polys, tol=1e-3):
    """how many leading points every polyline shares"""
    n = min(len(p) for p in polys)
    for i in range(n):
        p0 = polys[0][i]
        if any(abs(p[i][0] - p0[0]) > tol or abs(p[i][1] - p0[1]) > tol for p in polys):
            return i
    return n


def static_suffix(polys, tol=1e-3):
    return static_prefix([list(reversed(p)) for p in polys], tol)


def faded(ink, pts, color, w, o0, o1, hot=False):
    """a polyline whose opacity runs from o0 to o1 (a soft joint between two layers)"""
    n = len(pts) - 1
    for i in range(n):
        o = o0 + (o1 - o0) * (i + 0.5) / n
        (ink.hot if hot else ink.line)([pts[i], pts[i + 1]], color, w, o)


class FaceParts:
    """Which part of each outline is static, from the frames of the layers that move it."""

    def __init__(self, face):
        self.face = face
        # the upper skin from the top of the upper lip on (face and cranium) never moves
        self.upper_static = static_suffix([fr["head"]["skin_upper"] for fr in face["upper"]])
        # the neck's skin below the jaw's reach: still to within 0.8 mm (2 px, under a line's glow)
        self.neck_static = static_prefix([fr["head"]["neck_front"] for fr in face["jaw"]], 0.08)
        # the skin's inner line runs under the chin too: its middle moves with the jaw
        inset = [fr["head"]["inset"] for fr in face["jaw"]]
        self.inset_n = len(inset[0])
        self.inset_pre, self.inset_suf = static_prefix(inset, 0.12), static_suffix(inset, 0.12)
        # where its moving stretch turns up behind the lips: drawn as a strip of its own (on Jaw Pos
        # too), so the jaw's strip stays the size of the jaw
        rest_inset = face["rest"]["head"]["inset"]
        self.inset_split = next((i for i in range(self.inset_pre, self.inset_n - self.inset_suf) if rest_inset[i][1] > 6.5),
                                self.inset_n - self.inset_suf)
        # the back wall up to the lowest the soft palate's lid starts, from the lowest glottis
        self.wall_top = min(fr["head"]["wall"][-1][1] for fr in face["velum"])
        self.wall_bottom = min(fr["glottis"][1] for fr in face["larynx"])


def wall_run(wall, y0, y1):
    """the wall's points between heights y0 and y1, cut exactly at both"""
    out = []
    if wall and y0 <= wall[0][1] <= y1:
        out.append(tuple(wall[0]))
    for p, q in zip(wall, wall[1:]):
        if (p[1] - y0) * (q[1] - y0) < 0:
            u = (y0 - p[1]) / (q[1] - p[1])
            out.append((p[0] + u * (q[0] - p[0]), y0))
        if (p[1] - y1) * (q[1] - y1) < 0:
            u = (y1 - p[1]) / (q[1] - p[1])
            out.append((p[0] + u * (q[0] - p[0]), y1))
            return out
        if y0 <= q[1] <= y1:
            out.append(tuple(q))
    return out


def draw_static(ink, fp):
    """the head around the moving parts: nothing in it moves"""
    h = fp.face["rest"]["head"]

    def L(name, color, w, o=1.0, closed=False):
        ink.line([hp(p) for p in h[name]], color, w * WS, o, closed)
    inset = [hp(p) for p in h["inset"]]
    ink.line(inset[:fp.inset_pre], BLUE_DIM, 0.9 * WS, 0.9)
    ink.line(inset[fp.inset_n - fp.inset_suf:], BLUE_DIM, 0.9 * WS, 0.9)
    L("skull", BLUE_DIM, 0.9)
    L("brain", BLUE_MID, 1.1)
    L("sulcus", BLUE_DIM, 0.9)
    L("eye", BLUE_MID, 1.1)
    L("lens", BLUE_DIM, 0.9)
    for v in h["vertebrae"]:
        ink.line([hp(p) for p in v], BLUE_DIM, 1.0 * WS, closed=True)
    L("cord_a", BLUE_DIM, 0.8, 0.8)
    L("cord_b", BLUE_DIM, 0.8, 0.6)
    L("nasal", BLUE_MID, 1.0)
    for t in h["turbinates"]:
        ink.line([hp(p) for p in t], BLUE_DIM, 0.9 * WS, closed=True)
    L("pal_oral", BLUE_MID, 1.1)
    L("pal_nasal", BLUE_MID, 1.1)
    su = h["skin_upper"]
    ink.hot([hp(p) for p in su[len(su) - fp.upper_static:]], BLUE, 1.7 * WS)
    ink.hot([hp(p) for p in h["neck_front"][:fp.neck_static]], BLUE, 1.7 * WS)
    # the back wall of the throat from the lowest larynx up to the soft palate's lid, and the hard palate
    ink.hot([hp(p) for p in wall_run(h["wall"], fp.wall_bottom, fp.wall_top)], BLUE, 1.4 * WS)
    ink.hot([hp(p) for p in h["palate"]], BLUE, 1.4 * WS)
    L("upper_teeth", BLUE_MID, 1.1)


def draw_larynx(ink, fr, fp):
    h = fr["head"]

    def L(name, color, w, o=1.0, closed=False):
        ink.line([hp(p) for p in h[name]], color, w * WS, o, closed)
    L("hyoid", BLUE_DIM, 1.0)
    L("thyroid", BLUE_DIM, 1.0)
    L("trachea_l", BLUE_MID, 1.0)
    L("trachea_r", BLUE_MID, 1.0)
    for r in h["rings"]:
        ink.line([hp(p) for p in r], BLUE_DIM, 0.9 * WS, closed=True)
    L("glottis_wall", BLUE, 1.2, 0.8)
    L("fold_l", BLUE, 1.0, closed=True)
    L("fold_r", BLUE, 1.0, closed=True)
    # the epiglottis from its base to its tip (the tongue draws it on to the tongue's root)
    ink.line([hp(p) for p in h["epiglottis"][:2]], BLUE, 2.2 * WS, 0.85)
    handle_ring(ink, fr["handles"]["larynx"], BLUE_MID, 5, 0.8)


# the tongue outline (head["tongue"], closed): root 0-8, dorsum 9-39, blade 40-45, underside 46-50
# (50 the frenulum, carried by the jaw), the floor 51-52 behind the lower teeth, then the first point again
TONGUE_FREN = 50


def draw_tongue(ink, fr, fp):
    h = fr["head"]
    tongue = [hp(p) for p in h["tongue"]]
    ink.clipped(tongue, lambda d: [ink.line([hp(p) for p in fb], GREEN, 0.8 * WS, 0.32, draw=d) for fb in h["fibres"]])
    ink.hot(tongue[:TONGUE_FREN - 3], GREEN, 1.6 * WS)
    # the underside fades out toward the frenulum, where the jaw takes over (the two layers can
    # differ there by the jaw's step)
    faded(ink, tongue[TONGUE_FREN - 4:TONGUE_FREN + 1], GREEN, 1.6 * WS, 1.0, 0.15, hot=True)
    ink.line([hp(p) for p in h["epiglottis"][1:]], BLUE, 2.2 * WS, 0.85)
    ink.line([hp(p) for p in h["inner_wall"][1:4]], BLUE, 1.3 * WS, 0.9)
    hd = fr["handles"]
    handle_ring(ink, hd["tongue"], GREEN, 8)
    handle_ring(ink, hd["tip"], BLUE_MID, 5, 0.8)


def tongue_fill(fr):
    return [hp(p) for p in fr["head"]["tongue"]]


def draw_jaw(ink, fr, fp):
    h = fr["head"]
    ink.hot([hp(p) for p in h["neck_front"][max(0, fp.neck_static - 1):]], BLUE, 1.7 * WS)
    ink.line([hp(p) for p in h["inset"][max(0, fp.inset_pre - 1):fp.inset_split]], BLUE_DIM, 0.9 * WS, 0.9)
    ink.hot([hp(p) for p in h["jaw_lower"]], RED, 1.5 * WS)
    ink.line([hp(p) for p in h["jaw_upper"]], RED, 1.0 * WS, 0.45)
    ink.line([hp(p) for p in h["lower_teeth"]], BLUE_MID, 1.1 * WS)
    # the floor of the mouth under the tongue, fading in from the frenulum
    t = [hp(p) for p in h["tongue"]]
    faded(ink, t[TONGUE_FREN:TONGUE_FREN + 3], GREEN, 1.6 * WS, 0.3, 1.0, hot=True)
    handle_ring(ink, fr["handles"]["jaw"], RED, 7, 0.95)


def draw_jaw_inset(ink, fr, fp):
    """the skin's inner line from the chin up behind the lips (it moves with the jaw)"""
    ink.line([hp(p) for p in fr["head"]["inset"][fp.inset_split - 1:fp.inset_n - fp.inset_suf + 1]], BLUE_DIM, 0.9 * WS, 0.9)


def draw_velum(ink, fr, fp):
    h = fr["head"]
    # the back wall above the static part, and the lid across to the uvula's tip
    run = wall_run(h["wall"], fp.wall_top, 99.0) + [tuple(h["velum"][11])]
    ink.hot([hp(p) for p in run], BLUE, 1.4 * WS)
    ink.line(chaikin([hp(p) for p in h["velum"]], 3), WHITE, 1.25 * WS, 0.92)
    handle_ring(ink, fr["handles"]["velum"], WHITE, 6, 0.9)


def draw_upper(ink, fr, fp):
    h = fr["head"]
    su = h["skin_upper"]
    ink.hot([hp(p) for p in su[:len(su) - fp.upper_static + 1]], BLUE, 1.7 * WS)
    ink.hot(chaikin([hp(p) for p in h["upper_lip"]], 1), PINK, 1.5 * WS)


def draw_lower(ink, fr, fp):
    h = fr["head"]
    sl = h["skin_lower"]
    ink.hot([hp(p) for p in sl[len(h["neck_front"]) - 1:]], BLUE, 1.7 * WS)
    ink.hot(chaikin([hp(p) for p in h["lower_lip"]], 1), PINK, 1.5 * WS)
    handle_ring(ink, fr["handles"]["lips"], PINK, 7, 0.95)


def ray_hits(o, d, poly):
    """distances along the ray o + t d (t > 0) where it crosses the closed polygon's edges, sorted"""
    out = []
    for a, b in zip(poly, poly[1:] + poly[:1]):
        ex, ey = b[0] - a[0], b[1] - a[1]
        den = d[0] * ey - d[1] * ex
        if abs(den) < 1e-12:
            continue
        wx, wy = a[0] - o[0], a[1] - o[1]
        t = (wx * ey - wy * ex) / den
        u = (wx * d[1] - wy * d[0]) / den
        if t > 1e-9 and 0.0 <= u < 1.0:
            out.append(t)
    return sorted(out)


def fan_lines(face):
    """The amber fan: the tract's cross-section lines at rest (the throat's and the mouth's), from the
    back wall and the roof down toward the tongue, drawn long enough to reach the lowest tongue frame
    and ending inside every frame, so the tongue's opaque inside makes them end at its surface."""
    tongues = [[tuple(p) for p in fr["head"]["tongue"][:-1]] for fr in face["tongue"]]
    lines, short = [], 0
    for ox, oy, ix, iy, region in face["rest"]["grid"]:
        if region not in (1, 2):
            continue
        dx, dy = ix - ox, iy - oy
        n = math.hypot(dx, dy)
        if n < 1e-6:
            continue
        d = (dx / n, dy / n)
        enter, leave, missed = [], [], False
        for poly in tongues:
            hits = ray_hits((ox, oy), d, poly)
            if len(hits) < 2:
                missed = True
                continue
            enter.append(hits[0])
            leave.append(hits[1])
        if missed or not enter:
            end = n    # a line some tongue never reaches (below its root): to the surface at rest
        else:
            end = min(max(enter) + FAN_MARGIN, min(leave) - 0.05)
            short += max(enter) > end
        lines.append([(ox, oy), (ox + d[0] * end, oy + d[1] * end)])
    return lines, short


def draw_fan(ink, lines, level):
    o = SOUND_LEVELS[level]
    if o <= 0:
        return
    for ln in lines:
        pts = [hp(p) for p in ln]
        if level >= 4:
            ink.hot(pts, AMBER, 1.15, o)
        else:
            ink.line(pts, AMBER, 1.1, o)


def layer_image(draw, fill=None):
    """one frame over the whole head box, transparent; `fill` (page px polygon) is opaque black"""
    bw, bh = HEAD_BOX[2], HEAD_BOX[3]
    ink = Ink(bw, bh)
    draw(ink)
    rgb = ink.render()
    img = key_out(rgb)
    if fill:
        mask = Image.new("L", (bw * SS, bh * SS), 0)
        ImageDraw.Draw(mask).polygon([(x * SS, y * SS) for x, y in fill], fill=255)
        mask = mask.resize((bw, bh), Image.LANCZOS)
        img = Image.composite(rgb.convert("RGBA"), img, mask)
    return img


def strip_of(name, frames):
    """the frames cropped to their common box, stacked; returns (file, bounds in page px, count)"""
    box = None
    for img in frames:
        bb = img.getbbox()
        if bb:
            box = bb if box is None else (min(box[0], bb[0]), min(box[1], bb[1]), max(box[2], bb[2]), max(box[3], bb[3]))
    box = box or (0, 0, 1, 1)
    w, h = box[2] - box[0], box[3] - box[1]
    strip = Image.new("RGBA", (w, h * len(frames)), (0, 0, 0, 0))
    for k, img in enumerate(frames):
        strip.paste(img.crop(box), (0, k * h))
    put(name, strip)
    return name, (HEAD_BOX[0] + box[0], HEAD_BOX[1] + box[1], w, h), len(frames)


def make_face(face):
    """The head (static) and every moving layer's strip. Returns the placements in page px, in draw
    order: (name, file, frames, parameter, bounds, when), `when` = (first jaw frame, last, jaw frames)
    for the lower lip's strips (one per pair of jaw frames, shown while Jaw Pos is at either), else
    None; and the pictures for the posture previews."""
    fp = FaceParts(face)
    head = layer_image(lambda ink: draw_static(ink, fp))
    hb = head.getbbox()
    put("face_head.png", head.crop(hb))
    fan, short = fan_lines(face)
    out = {"head": (HEAD_BOX[0] + hb[0], HEAD_BOX[1] + hb[1], hb[2] - hb[0], hb[3] - hb[1]), "layers": [],
           "fan_lines": len(fan), "fan_short": short}
    frames = {}
    frames["sound"] = [layer_image(lambda ink, k=k: draw_fan(ink, fan, k)) for k in range(SOUND_FRAMES)]
    frames["larynx"] = [layer_image(lambda ink, fr=fr: draw_larynx(ink, fr, fp)) for fr in face["larynx"]]
    frames["tongue"] = [layer_image(lambda ink, fr=fr: draw_tongue(ink, fr, fp), tongue_fill(fr)) for fr in face["tongue"]]
    frames["jaw"] = [layer_image(lambda ink, fr=fr: draw_jaw(ink, fr, fp)) for fr in face["jaw"]]
    frames["jaw_inset"] = [layer_image(lambda ink, fr=fr: draw_jaw_inset(ink, fr, fp)) for fr in face["jaw"]]
    frames["velum"] = [layer_image(lambda ink, fr=fr: draw_velum(ink, fr, fp)) for fr in face["velum"]]
    frames["upper"] = [layer_image(lambda ink, fr=fr: draw_upper(ink, fr, fp)) for fr in face["upper"]]
    frames["lower"] = [[layer_image(lambda ink, fr=fr: draw_lower(ink, fr, fp)) for fr in group] for group in face["lower"]]
    nj = len(face["jaw"])
    for name, param in FACE_LAYERS:
        if name == "lower":
            for g, group in enumerate(frames["lower"]):
                f, bounds, n = strip_of("face_lower_%d.png" % g, group)
                out["layers"].append(("lower lip %d" % g, f, n, param, bounds, (2 * g, min(2 * g + 1, nj - 1), nj)))
        else:
            f, bounds, n = strip_of("face_%s.png" % name, frames[name])
            out["layers"].append((name, f, n, param, bounds, None))
    out["pictures"] = (head, frames)
    return out


def posture_previews(face, placements):
    """build/preview/face_<posture>.png: the head box with the frames the engine picks for each check
    posture (dump_face.rs; "a_loud" with the sound fan lit), and face_<posture>_check.png with the
    posture's true tongue, jaw and lips outlines over it in thin white, to judge how the layers line up"""
    head, frames = placements["pictures"]
    bw, bh = HEAD_BOX[2], HEAD_BOX[3]
    written = []
    for name, ps in face["postures"].items():
        jaw, tongue, upper, lower, larynx, velum = ps["frames"]
        for sound in ((0, 5) if name == "a" else (0,)):
            img = Image.new("RGBA", (bw, bh), (0, 0, 0, 255))
            for layer in (head, frames["sound"][sound], frames["larynx"][larynx], frames["tongue"][tongue],
                          frames["jaw"][jaw], frames["jaw_inset"][jaw], frames["velum"][velum], frames["upper"][upper], frames["lower"][jaw // 2][lower]):
                img.alpha_composite(layer)
            tag = name + ("_loud" if sound else "")
            img.convert("RGB").save(os.path.join(PREVIEW, "face_%s.png" % tag))
            written.append(tag)
            if sound:
                continue
            h = ps["true"]["head"]
            d = ImageDraw.Draw(img)
            for part in ("tongue", "lower_lip", "upper_lip", "jaw_lower", "lower_teeth"):
                d.line([hp(p) for p in h[part]], fill=(255, 255, 255, 255), width=1)
            img.convert("RGB").save(os.path.join(PREVIEW, "face_%s_check.png" % tag))
    return written


# --------------------------------------------------------------------------------------------
# Controls' pictures
# --------------------------------------------------------------------------------------------
def dial_strip(color):
    """CHIMERA's encoder: a thin ring in the slot color, ticks outside, a pointer from a center dot"""
    out = Image.new("RGBA", (DIAL, DIAL * KNOB_FRAMES), (0, 0, 0, 0))
    c, r, a0, a1 = DIAL / 2, 21.0, 135.0, 405.0
    for k in range(KNOB_FRAMES):
        ink = Ink(DIAL, DIAL)
        for i in range(11):
            a = math.radians(a0 + (a1 - a0) * i / 10)
            ink.line([(c + math.cos(a) * (r + 4), c + math.sin(a) * (r + 4)), (c + math.cos(a) * (r + 8), c + math.sin(a) * (r + 8))], color, 1.0, 0.55)
        ink.circle(c, c, r, color, 1.5, 0.95)
        a = math.radians(a0 + (a1 - a0) * k / (KNOB_FRAMES - 1))
        ink.line([(c, c), (c + math.cos(a) * (r - 3), c + math.sin(a) * (r - 3))], core(color), 2.0)
        ink.circle(c, c, 3.2, core(color), fill=True)
        out.paste(ink.rgba(), (0, k * DIAL))
    return out


def dial_file(slot):
    return put("dial_%s.png" % SLOT_NAMES[slot], DIAL_STRIPS[slot])


def solid_button(w, h, color, text, px, path=None, track=0.12):
    """an "on" button (the owner, 2026-11-26: on = solid, off = outline): filled with its color,
    the text dark on it; opaque (buttons sit on the black page)"""
    ink = Ink(w, h)
    ink.fill_post(0, 0, w, h, core(color) if color == AMBER else light(color))

    def txt(d):
        f = font(path or HERSHEY, px + (6 if path else 0))
        if path:
            d.text((w / 2, h / 2), text, font=f, fill=BTN_TEXT, anchor="mm")
            return
        widths = [f.getlength(ch) for ch in text]
        x0 = w / 2 - (sum(widths) + track * px * (len(text) - 1)) / 2
        for ch, wd in zip(text, widths):
            d.text((x0, (h - px) / 2 - 1), ch, font=f, fill=BTN_TEXT)
            x0 += wd + track * px
    ink.post.append(txt)
    return ink.render(False).convert("RGBA")


def outline_button(w, h, color, text, px, path=None, track=0.13):
    """an "off" button: a clear 1.4 px outline and the text in its color"""
    ink = Ink(w, h)
    ink.rect(0, 0, w, h, color, 1.4, 0.85)
    if path:
        ink.glyph(w / 2, h / 2, text, px + 6, color, path)
    else:
        ink.cap(w / 2, (h - px) / 2 - 1, text, px, color, "m", track, 0.9)
    return ink.rgba()


def chip_files(text, w, h=40, path=None, px=15):
    """a square chip: on = solid blue, dark text; off = blue outline, blue text"""
    names = []
    for on in (True, False):
        img = solid_button(w, h, BLUE, text, px, path) if on else outline_button(w, h, BLUE, text, px, path)
        tag = slug(text) if all(ord(c) < 128 for c in text) else "u%s" % "".join("%04x" % ord(c) for c in text)
        names.append(put("chip_%s_%d_%s.png" % (tag, w, "on" if on else "off"), img))
    return names


def main_button_files(text):
    w, h = MAIN_BTN[2], MAIN_BTN[3]
    names = []
    for on in (True, False):
        img = solid_button(w, h, AMBER, text, 17, track=0.1) if on else outline_button(w, h, AMBER, text, 17, track=0.1)
        names.append(put("main_%s_%s.png" % (slug(text), "on" if on else "off"), img))
    return names


def toggle_files(slot):
    color = SLOTS[slot]
    names = []
    for on in (True, False):
        img = solid_button(100, 44, color, "ON", 16, track=0.14) if on else outline_button(100, 44, color, "OFF", 16, track=0.14)
        names.append(put("toggle_%s_%s.png" % (SLOT_NAMES[slot], "on" if on else "off"), img))
    return names


def keycap_files(name, w, h, color, o_off=0.8, label=None, label_px=20):
    """a key: off = 1 px frame; on = bright frame and a faint fill (the glyph under it shows through)"""
    names = []
    for on in (True, False):
        ink = Ink(w, h)
        if on:
            ink.rect(0, 0, w, h, core(color), 1.6, fill=dim(color, 0.16))
        else:
            ink.rect(0, 0, w, h, color, 1.0, o_off)
        if label:
            ink.cap(w / 2, (h - label_px) / 2 - 1, label, label_px, core(color) if on else color, "m", 0.16)
        names.append(put("%s_%s.png" % (name, "on" if on else "off"), ink.rgba()))
    return names


def frame_file(color):
    """a 2 x 2 PNG of one color, stretched by an Image to draw a frame edge (design.md B.9)"""
    return put("px_%02x%02x%02x.png" % color, Image.new("RGBA", (2, 2), color + (255,)))


# --------------------------------------------------------------------------------------------
# TUI.json pieces
# --------------------------------------------------------------------------------------------
def label_data(label_type, color, height, justification="horizontallyCentred verticallyCentred", style="Light", text=None):
    pairs = [("version", 1),
             ("textStyle", G.obj(("version", 1),
                                 ("font", G.obj(("version", 1), ("name", "Roboto"), ("style", style), ("height", float(height)))),
                                 ("colour", color), ("justification", justification), ("case", "Original"))),
             ("type", label_type), ("handleName", "Invalid" if label_type == "Text" else "Data")]
    if text is not None:
        pairs.append(("text", text))
    return G.Obj(pairs)


def rect_str(x, y, w, h):
    return "%d %d %d %d" % (int(round(x)), int(round(y)), int(round(w)), int(round(h)))


def image_entry(name, image, bounds, when=None):
    """when = (choice, number of choices, handle): shown only while that parameter is at that choice"""
    data = G.obj(("version", 2), ("imageType", "Regular"), ("colour", "0"), ("image", image))
    inv = ["IndexedEnabling/%d/%d/%s" % when] if when else None
    return G.entry(name, "Image", data, rect_str(*bounds), "No", "Show", "Always", invalidating=inv)


def film_entry(name, image, frames, handle, bounds, when=None):
    """when: an enabling handle (RangedEnabling/...): shown only while it is valid"""
    data = G.obj(("version", 2), ("imageType", "FilmStrip"), ("colour", "0"), ("image", image),
                 ("numFrames", frames), ("handleName", "Data"))
    inv = [when] if when else None
    return G.entry(name, "Image", data, rect_str(*bounds), "No", "Show", "Always", remap=[("Data", handle)], invalidating=inv)


def focus_entry(color, w, h):
    data = G.obj(("version", 1), ("backgroundColour", argb(color, 0x16)), ("outlineColour", argb(color)),
                 ("backgroundInset", 2.0), ("outlineThickness", 2.0))
    return G.entry("focus", "Focus", data, rect_str(0, 0, w, h), "No", "Show", "WhenFocussed")


def xypad_entry(name, hx, hy, puck, bounds):
    data = G.obj(("version", 1), ("xPositionHandle", hx), ("yPositionHandle", hy), ("image", puck))
    return G.entry(name, "XYPad", data, rect_str(*bounds), "No", "Show", "Always")


# cell styles: name y / px, dial center y, value y / px, unit y, toggle y
CELL_STYLE = {"compact": dict(name_y=8, name_px=15, dial_y=55, value_y=79, value_px=28, unit_y=117, toggle_y=40),
              "full": dict(name_y=3, name_px=16, dial_y=52, value_y=80, value_px=31, unit_y=115, toggle_y=34)}


class Builder:
    """Collects the component definitions and the parameters a page refers to."""

    def __init__(self, params, units):
        self.params = params
        self.units = units
        self.defs = []
        self.referenced = {}

    def need(self, name, make):
        if not any(k == name for k, _ in self.defs):
            self.defs.append((name, make()))
        return name

    def ref(self, p):
        self.referenced[p.index] = p
        return p

    # ---- pieces on a page ------------------------------------------------------------------
    def frame(self, ch, x, y, w, h, color=FRAME, gap=None):
        """a 1 px rectangle as four stretched edges; gap = (x0, x1) leaves the top edge open there"""
        f = frame_file(color)
        tops = [(x, x + w)] if not gap else [(x, gap[0]), (gap[1], x + w)]
        for i, (a, b) in enumerate(tops):
            if b > a:
                ch.append(image_entry("edge", f, (a, y, b - a, 1)))
        ch.append(image_entry("edge", f, (x, y + h - 1, w, 1)))
        ch.append(image_entry("edge", f, (x, y, 1, h)))
        ch.append(image_entry("edge", f, (x + w - 1, y, 1, h)))

    def cap(self, ch, x, y, text, px, color, anchor="l", track=0.1, o=1.0, path=None):
        f, dx, dy, w = caption(text, px, color, track, o, path)
        x0 = x - (w if anchor == "r" else w / 2 if anchor == "m" else 0)
        img = IMAGES[f]
        ch.append(image_entry("cap " + text, f, (x0 + dx, y + dy, img.width, img.height)))
        return w

    def pic(self, ch, f, x, y):
        img = IMAGES[f]
        ch.append(image_entry(f, f, (x, y, img.width, img.height)))

    def label(self, ch, p, bounds, px, color, just="horizontallyCentred verticallyCentred", style="Light", invalidating=None):
        self.ref(p)
        ch.append(G.entry("value " + p.name, "Label", label_data("Value", argb(color), px, just, style), rect_str(*bounds),
                          "No", "Show", "Always", remap=[("Data", p.handle)], invalidating=invalidating))

    def button(self, ch, p, on, off, k, n, x, y, momentary=False, name=None):
        self.ref(p)
        dn = self.need("chimButton %s %s %d %d" % (on, off, k, n), lambda: G.component_definition(
            [], [G.entry("b", "Button", G.button_data(on, off, k, n), rect_str(0, 0, *IMAGES[on].size), "No", "Show", "Always")], "00000000"))
        handle = ("ResetOnGestureEnd/" + p.handle) if momentary else p.handle
        ch.append(G.entry(name or "%s %d" % (p.name, k), dn, G.obj(("version", 1), ("handleName", "Data")),
                          rect_str(x, y, *IMAGES[on].size), "No", "Hide", "Always", remap=[("Data", handle)]))

    def control_def(self, style, w, h, slot, p):
        st = CELL_STYLE[style]
        color = SLOTS[slot]
        if p.kind == "bool":
            on, off = toggle_files(slot)

            def make():
                return G.component_definition(G.control_actions("menu overlay", double_click=False),
                                              [focus_entry(color, w, h),
                                               G.entry("toggle", "Button", G.button_data(on, off, 1, 1), rect_str((w - 100) / 2, st["toggle_y"], 100, 44), "No", "Show", "Always")],
                                              "00000000")
            return self.need("chimToggle %s %d %d %d" % (style, w, h, slot), make)
        overlay = "menu overlay" if p.is_discrete else ("knob overlay bipolar" if p.name in BIPOLAR else "knob overlay")

        def make():
            return G.component_definition(G.control_actions(overlay), [
                focus_entry(color, w, h),
                G.entry("knob", "Knob", G.knob_filmstrip_data(dial_file(slot), KNOB_FRAMES), rect_str(w / 2 - DIAL / 2, st["dial_y"] - DIAL / 2, DIAL, DIAL), "No", "Show", "Always"),
                G.entry("value", "Label", label_data("Value", argb(light(color)), st["value_px"]), rect_str(0, st["value_y"], w, 34), "No", "Show", "Always")],
                "00000000")
        return self.need("chimKnob %s %d %d %d %s" % (style, w, h, slot, overlay), make)

    def cell(self, ch, style, x, y, w, h, slot, name):
        """one Q-Link cell: the name and unit baked, the dial / chip and numeral in a component"""
        st = CELL_STYLE[style]
        p = self.ref(self.params[name])
        color = SLOTS[slot]
        self.cap(ch, x + w / 2, y + st["name_y"], name.upper(), st["name_px"], color, "m", 0.1)
        unit = self.units.get(name, "")
        if unit and p.kind != "bool":
            self.cap(ch, x + w / 2, y + st["unit_y"], unit, 12, color, "m", 0.12, 0.8)
        dn = self.control_def(style, int(w), int(h), slot, p)
        handle = ("ResetOnGestureEnd/" + p.handle) if name in MOMENTARY else p.handle
        ch.append(G.entry(p.name, dn, G.obj(("version", 1), ("handleName", "Data")), rect_str(x, y, w, h),
                          "Yes", "Hide", "Always", remap=[("Data", handle)]))


# --------------------------------------------------------------------------------------------
# Layout of the encoder block
# --------------------------------------------------------------------------------------------
def group_layout(g):
    """(frame (x, y, w, h), title (x, y, anchor), cells [(x, y, w, h)] * 4, style, qlink bounds)"""
    if GROUPING == "rows":
        y0 = TOP + g * 144
        cells = [(GRID_X + round(i * GRID_W / 4), y0 + 6, round(GRID_W / 4), 132) for i in range(4)]
        return (GRID_X, y0 + 6, GRID_W, 132), (GRID_X + 20, y0, "l"), cells, "compact", rect_str(GRID_X - 2, y0 + 4, GRID_W + 4, 136)
    x0 = GRID_X + g * 166
    cells = [(x0, TOP + 34 + i * 134, 160, 134) for i in range(4)]
    return (x0, TOP, 160, BOT - TOP), (x0 + 80, TOP + 7, "m"), cells, "full", rect_str(x0 - 2, TOP - 2, 164, BOT - TOP + 4)


def qlink_for(g, i):
    """Q-Link number for member i of group g (Q-Link 13 is the top-left knob)"""
    return 13 + i - 4 * g if GROUPING == "rows" else 13 + g - 4 * i


def bank_direction():
    return "RowTopDown" if GROUPING == "rows" else "Column"


def encoder_block(b, ch, groups):
    bounds = []
    for g, (title, names) in enumerate(groups):
        frame, (tx, ty, anchor), cells, style, qb = group_layout(g)
        if not any(names):
            bounds.append("0 0 0 0")
            continue
        bounds.append(qb)
        color = BLUE_DIM if g == 0 else FRAME
        if GROUPING == "rows":
            tw = text_width(title, 14, 0.14)
            b.frame(ch, *frame, color=color, gap=(tx - 8, tx + tw + 8))
            b.cap(ch, tx, ty, title, 14, BLUE, "l", 0.14)
        else:
            b.frame(ch, *frame, color=color)
            b.cap(ch, tx, ty, title, 15, BLUE, "m", 0.14)
            ch.append(image_entry("rule", frame_file(FRAME), (frame[0] + 10, TOP + 27, frame[2] - 20, 1)))
        for i, name in enumerate(names):
            if name:
                b.cell(ch, style, *cells[i], i, name)
    return bounds


# --------------------------------------------------------------------------------------------
# Pages
# --------------------------------------------------------------------------------------------
def top_bar(b, ch, page):
    """mirrored: the main button and the page's chips at the left, the mode chips, the wordmark at the right"""
    params = b.params
    if page.get("main"):
        text, pname = page["main"]
        on, off = main_button_files(text)
        b.button(ch, params[pname], on, off, 1, 1, MAIN_BTN[0], MAIN_BTN[1], momentary=pname in MOMENTARY, name="main " + text)
    x = PAGE_CHIPS_X
    for text, pname, w in page.get("chips", []):
        on, off = chip_files(text, w)
        b.button(ch, params[pname], on, off, 1, 1, x, 4, name="chip " + text)
        x += w + 8
    if page.get("caption"):
        b.cap(ch, x + 4, 17, page["caption"], 13, QUIET, "l", 0.1)
    if page.get("body_chips"):
        p = params["Body"]
        for k, text in enumerate(BODIES):
            on, off = chip_files(text, 104)
            b.button(ch, p, on, off, k, len(p.choices), 12 + k * 112, 4)
    p = params["Mode"]
    for k, (text, mx, w) in enumerate(MODE_CHIPS):
        on, off = chip_files(text, w, px=16)
        b.button(ch, p, on, off, k, len(p.choices), mx, 4)
    b.cap(ch, W - 16, 12, "CHIMERA", 24, BLUE, "r", 0.28)


def screen(b, ch, x, y, w, h, title, sub=None, focus=False):
    b.frame(ch, x, y, w, h, BLUE_DIM if focus else FRAME)
    tw = b.cap(ch, x + 12, y + 7, title, 15, BLUE, "l", 0.12)
    if sub:
        b.cap(ch, x + 12 + tw + 18, y + 9, sub, 12, QUIET, "l", 0.1)
    return x + 12 + tw + 18


def body_caption_strip(word):
    """4 frames on Body: "ADULT MALE / <word>" ... (one male head for every body; the caption names it)"""
    texts = ["%s / %s" % (c, word) for c in BODY_CAPTIONS]
    m = 6
    w = int(max(text_width(t, 12, 0.1) for t in texts)) + 2 * m + 2
    h = 12 + 2 * m + 4
    strip = Image.new("RGBA", (w, h * 4), (0, 0, 0, 0))
    for k, t in enumerate(texts):
        ink = Ink(w, h)
        ink.cap(m, m, t, 12, QUIET, "l", 0.1)
        strip.paste(ink.rgba(), (0, k * h))
    return put("body_%s.png" % slug(word), strip), m, w, h


def face_screen(b, ch, page, face):
    params = b.params
    tx = screen(b, ch, SX, TOP, SW, BOT - TOP, "VOCAL TRACT")
    f, m, w, h = body_caption_strip(page["sub_word"])
    ch.append(film_entry("body caption", f, 4, b.ref(params["Body"]).handle, (tx - m, TOP + 9 - m, w, h)))
    ch.append(image_entry("head", "face_head.png", face["head"]))
    jaw = b.ref(params["Jaw Pos"])
    for name, f, n, pname, bounds, when in face["layers"]:
        ch.append(film_entry(name, f, n, b.ref(params[pname]).handle, bounds,
                             ("RangedEnabling/%d/%d/%d/%s" % (when + (jaw.handle,))) if when else None))
    if page.get("readouts"):
        # the slot readouts under the chin (design.md B.3), as live values of the knobs that set them;
        # TONGUE (nearest vowel) needs a new display parameter and is left for step 2
        # in front of the neck: right of it when the head faces right, left of it when it faces left
        bx, by = (SX + 14 if FACE_LEFT else SX + 330), TOP + 452
        cells = ((bx, by), (bx + 128, by + 58), (bx, by + 58)) if FACE_LEFT else ((bx + 128, by), (bx, by + 58), (bx + 128, by + 58))
        for (lab, unit, pname, slot), (cx, cy) in zip((("JAW", "CM", "Jaw", 1), ("LIPS", "CM", "Lips Open", 2), ("VELUM", "CM2", "Soft Palate", 3)),
                                                      cells):
            color = SLOTS[slot]
            fdot, dx, dy = baked(("dot", lab, unit, color), 120, 20, lambda ink, mm, lab=lab, unit=unit, color=color: (
                ink.circle(mm + 5, mm + 7, 4.5, color, fill=True),
                ink.cap(mm + 16 + ink.cap(mm + 16, mm, lab, 13, color, "l", 0.12) + 7, mm + 1, unit, 12, color, "l", 0.1, 0.7)), prefix="ro")
            b.pic(ch, fdot, cx + dx, cy + dy)
            if pname != "Soft Palate":
                b.label(ch, params[pname], (cx - 2, cy + 15, 126, 42), 34, light(color), "left verticallyCentred")
                continue
            # the soft palate says "Shut" while the port is closed (Velum Pos at its frame 0), its
            # value otherwise
            vp = b.ref(params["Velum Pos"])
            nv = face["velum_frames"]
            b.label(ch, params[pname], (cx - 2, cy + 15, 126, 42), 34, light(color), "left verticallyCentred",
                    invalidating=["RangedEnabling/1/%d/%d/%s" % (nv - 1, nv, vp.handle)])
            fs, sx, sy = baked(("shut", color), 120, 40, lambda ink, mm, color=color: ink.cap(mm, mm, "SHUT", 26, light(color), "l", 0.12),
                               prefix="ro")
            img = IMAGES[fs]
            ch.append(image_entry("shut", fs, (cx + sx, cy + 23 + sy, img.width, img.height), when=(0, nv, vp.handle)))
    if page.get("credit"):
        # in front of the neck, like the readouts
        cx, just = (SX + 16, "l") if FACE_LEFT else (SX + SW - 16, "r")
        b.cap(ch, cx, TOP + 470, "VOICE: CHIMERA", 12, QUIET, just, 0.1)
        b.cap(ch, cx, TOP + 490, "BY JAMESON HODGE (MIT)", 12, QUIET, just, 0.1)
    if page.get("left") == "steps":
        # the beat's name and tempo at the top right (the hit symbol and step need new parameters: step 2)
        b.label(ch, params["Beat"], (SX + SW - 300, TOP + 38, 284, 30), 22.0, AMBER, "right verticallyCentred", "Regular")
        b.label(ch, params["Tempo"], (SX + SW - 160, TOP + 70, 100, 26), 20.0, LABEL, "right verticallyCentred", "Regular")
        b.cap(ch, SX + SW - 54, TOP + 77, "BPM", 12, AMBER, "l", 0.1, 0.8)


def vowel_screen(b, ch):
    params = b.params
    screen(b, ch, SX, TOP, SW, BOT - TOP, "VOWELS", "F1 / F2")
    x, y, w, h = VOWEL_BOX

    b.frame(ch, x, y, w, h, FRAME)
    for ipa, f1, f2 in VOWEL_CHART:
        u = (math.log(F2_RANGE[1]) - math.log(f2)) / (math.log(F2_RANGE[1]) - math.log(F2_RANGE[0]))
        v = (math.log(f1) - math.log(F1_RANGE[0])) / (math.log(F1_RANGE[1]) - math.log(F1_RANGE[0]))
        f, dx, dy = baked(("vowel", ipa), 60, 60, lambda ink, m, s=ipa: ink.glyph(m + 30, m + 30, s, 34, BLUE, SEGOE), prefix="g")
        b.pic(ch, f, x + 18 + u * (w - 36) - 30 + dx, y + 18 + v * (h - 36) - 30 + dy)
    b.cap(ch, SX + SW / 2, y + h + 12, "LEFT: FRONT    DOWN: OPEN", 13, QUIET, "m", 0.1)
    # Open grows downward on the chart, and the pad's y grows upward: the skin shows it as is
    ch.append(xypad_entry("vowel pad", b.ref(params["Front"]).handle, b.ref(params["Open"]).handle, "puck.png", (x, y, w, h)))


def sounds_screen(b, ch):
    params = b.params
    screen(b, ch, SX, TOP, SW, BOT - TOP, "CONSONANTS", "BETWEEN TWO VOWELS")
    on, off = keycap_files("cons_cap", 104, 76, AMBER, 0.5)
    pos = [(SX + 18 + (k % 5) * 112, TOP + 32 + (k // 5) * 84) for k in range(len(CONSONANTS))]
    for k, (x, y) in enumerate(pos):
        f, dx, dy = baked(("cons", CONSONANTS[k]), 60, 60, lambda ink, m, s=CONSONANTS[k]: ink.glyph(m + 30, m + 30, s, 34, AMBER, SEGOE), prefix="g")
        b.pic(ch, f, x + 52 - 30 + dx, y + 38 - 30 + dy)
    for k, (x, y) in enumerate(pos):
        b.button(ch, params["Consonant"], on, off, k + 1, len(params["Consonant"].choices), x, y, momentary=True)
    b.cap(ch, SX + 18, TOP + 456, "THE VOWEL ON EACH SIDE", 12, QUIET, "l", 0.1)
    p = params["Pad Vowel"]
    for k, v in enumerate(PAD_VOWELS):
        con, coff = chip_files(v, 130, 44, SEGOE, 18)
        b.button(ch, p, con, coff, k, len(p.choices), SX + 18 + k * 140, TOP + 478)


def pads_screen(b, ch):
    params = b.params
    screen(b, ch, SX, TOP, SW, BOT - TOP, "PADS", "TAP TO HEAR ONE SOUND")
    on, off = keycap_files("pad_cap", 280, 88, AMBER, 0.55)
    pos = [(SX + 10 + (k % 2) * 288, TOP + 32 + (k // 2) * 96) for k in range(len(PADS))]
    for k, (x, y) in enumerate(pos):
        name, sym = PADS[k]

        def art(ink, m, name=name, sym=sym):
            ink.cap(m + 18, m + 34, name, 18, AMBER, "l", 0.14)
            ink.cap(m + 262, m + 24, sym, 34, core(AMBER), "r", 0.02)
        f, dx, dy = baked(("pad", name, sym), 280, 88, art, prefix="g")
        b.pic(ch, f, x + dx, y + dy)
    # the pad last heard (a kit pad, a drum key, or a step of the beat) lights for a moment: the
    # touch's own lit cap, shown while Hit names that pad
    hit = b.ref(params["Hit"])
    for k, (x, y) in enumerate(pos):
        ch.append(image_entry("hit %d" % (k + 1), on, (x, y, 280, 88), when=(k + 1, len(hit.choices), hit.handle)))
    p = params["Pad"]
    for k, (x, y) in enumerate(pos):
        b.button(ch, p, on, off, k + 1, len(p.choices), x, y, momentary=True)
    b.cap(ch, SX + 18, TOP + 516, "MPC PADS 1-16 PLAY THE BEATBOX SQUARES; OTHER DRUM KEYS: GM", 12, QUIET, "l", 0.1)
    b.cap(ch, SX + 18, TOP + 538, "C3 AND UP HUM TWO OCTAVES DOWN", 12, QUIET, "l", 0.1)


def steps_block(b, ch):
    """BEATBOX: the 16 steps laid out as the MPC's pads (1-4 on the bottom row, 13-16 on top), so
    pad n plays square n and Q-Link n (Q-Links sit as the pads do) turns it; a square fills while
    Step Lit names it (its pad hit, or the beat reaching it). Bank Direction Column."""
    params = b.params
    bounds = []

    def step_def():
        return G.component_definition(G.control_actions("menu overlay"), [
            focus_entry(AMBER, 160, 138),
            G.entry("value", "Label", label_data("Value", argb(AMBER), 60.0), rect_str(0, 32, 160, 80), "No", "Show", "Always")], "00000000")
    dn = b.need("chimStep", step_def)
    lit_param = b.ref(params["Step Lit"])
    if "step_lit.png" not in IMAGES:
        im = Image.new("RGBA", (160, 138), AMBER + (70,))
        ImageDraw.Draw(im).rectangle((0, 0, 159, 137), outline=core(AMBER) + (255,), width=2)
        put("step_lit.png", im)
    assigned = {}
    for k in range(16):
        c, r = k % 4, 3 - k // 4     # pad layout: step 1 bottom-left, 16 top-right
        x, y = GRID_X + c * 166, TOP + r * 144
        b.frame(ch, x, y, 160, 138, BLUE_DIM if c == 0 else FRAME)
        ch.append(image_entry("lit %d" % (k + 1), "step_lit.png", (x, y, 160, 138), when=(k + 1, len(lit_param.choices), lit_param.handle)))
        b.cap(ch, x + 10, y + 8, "%d" % (k + 1), 15, QUIET, "l", 0.1)
        p = b.ref(params["Step %d" % (k + 1)])
        ch.append(G.entry(p.name, dn, G.obj(("version", 1), ("handleName", "Data")), rect_str(x, y, 160, 138),
                          "Yes", "Hide", "Always", remap=[("Data", p.handle)]))
        assigned[13 + c - 4 * r] = p.index
    for c in range(4):
        bounds.append(rect_str(GRID_X + c * 166 - 2, TOP - 2, 164, 4 * 144 + 2))
    return bounds, assigned


def words_block(b, ch):
    """WORDS: the knob row (left), what to say (right), the keyboard (design.md B.5, mirrored)"""
    params = b.params
    b.frame(ch, 12, TOP, 550, 138)
    assigned = {}
    for i, name in enumerate(WORDS_ROW):
        x = 12 + round(i * 137.5)
        b.cell(ch, "compact", x, TOP + 2, 137, 134, i, name)
        assigned[13 + i] = params[name].index
    tx = screen(b, ch, 572, TOP, 696, 138, "WHAT TO SAY", focus=True)
    b.label(ch, params["Type Into"], (tx, TOP + 5, 58, 20), 14.0, QUIET, "left verticallyCentred", "Regular")
    b.cap(ch, tx + 58, TOP + 9, "/  SLOT", 12, QUIET, "l", 0.1)
    b.label(ch, params["Type Slot"], (tx + 120, TOP + 5, 16, 20), 14.0, QUIET, "horizontallyCentred verticallyCentred", "Regular")
    b.cap(ch, tx + 140, TOP + 9, "OF 8", 12, QUIET, "l", 0.1)
    b.label(ch, params["Typed"], (590, TOP + 36, 660, 56), 40.0, INK, "left verticallyCentred")
    b.cap(ch, 590, TOP + 108, "EACH LINE IS WHAT ONE NOTE SAYS.  LINE STARTS A NEW ONE, SHOWN AS /", 12, QUIET, "l", 0.1)
    # the keys: one shared cap pair for the letters, the letters baked under the buttons
    kp = params["Key"]
    n = len(kp.choices)
    on, off = keycap_files("key_cap", KEY_W, KEY_H, BLUE_MID)
    keys = []
    for r, row in enumerate(KEY_ROWS):
        for c, (lab, step) in enumerate(row):
            keys.append((lab, step, KEYS_X + c * KEY_PITCH_X, KEYS_Y + r * KEY_PITCH_Y, on, off))
    y = KEYS_Y + 3 * KEY_PITCH_Y
    for lab, step, x, w, color in WIDE_KEYS:
        if color is None:
            keys.append((lab, step, x, y, on, off))
        else:
            con, coff = keycap_files("key_%s" % slug(lab), w, KEY_H, BLUE_MID if color == LABEL else color, label=lab, label_px=20)
            keys.append((None, step, x, y, con, coff))
    for lab, step, x, y, con, coff in keys:
        if lab:
            f, dx, dy = baked(("key", lab), 60, 60, lambda ink, m, s=lab: ink.glyph(m + 30, m + 30, s, 42, INK, HERSHEY), prefix="g")
            b.pic(ch, f, x + KEY_W / 2 - 30 + dx, y + KEY_H / 2 - 30 + dy)
    for lab, step, x, y, con, coff in keys:
        b.button(ch, kp, con, coff, step, n, x, y, momentary=True, name="key %d" % step)
    return [rect_str(10, TOP - 2, 554, 142)], assigned


def build_tui(params, units, face):
    b = Builder(params, units)
    tabs, qmaps = [], []
    for page in PAGES:
        ch = []
        right = page.get("right")
        if right == "face":
            face_screen(b, ch, page, face)
        elif right == "vowel":
            vowel_screen(b, ch)
        elif right == "sounds":
            sounds_screen(b, ch)
        elif right == "pads":
            pads_screen(b, ch)
        if page.get("left") == "steps":
            bounds, assigned = steps_block(b, ch)
            direction = "Column"
        elif page.get("left") == "words":
            bounds, assigned = words_block(b, ch)
            direction = "RowTopDown"
        else:
            bounds = encoder_block(b, ch, page["groups"])
            assigned = {qlink_for(g, i): params[n].index for g, (_, names) in enumerate(page["groups"]) for i, n in enumerate(names) if n}
            direction = bank_direction()
        top_bar(b, ch, page)
        tabs.append(G.obj(("version", 1), ("tabName", page["name"]), ("fnKeyIndex", page["fn"]), ("fnKeySubIndex", page["sub"]),
                          ("componentDefinition", G.component_definition([], ch, ARGB_BG)),
                          ("qlinkBoundsData", bounds)))
        qmaps.append((page, direction, assigned))
    b.defs.append(("knob overlay", G.overlay_component("KnobOverlay", G.knob_overlay_data(False), ARGB_OVERLAY)))
    b.defs.append(("knob overlay bipolar", G.overlay_component("KnobOverlay", G.knob_overlay_data(True), ARGB_OVERLAY)))
    b.defs.append(("menu overlay", G.overlay_component("MenuOverlay", G.menu_overlay_data(), ARGB_OVERLAY)))
    replacements = [G.obj(("key", p.handle), ("value", list(p.choices))) for _, p in sorted(b.referenced.items()) if p.is_discrete and p.choices]
    page_data = G.obj(
        ("version", 1),
        ("componentDefinitions", G.obj(("version", 2), ("importFiles", []),
                                       ("localComponentDefinitions", [G.obj(("key", k), ("value", v)) for k, v in b.defs]))),
        ("replacementStrings", replacements),
        ("info", G.obj(("version", 1), ("type", "CompleteDescription"))),
        ("tabs", tabs))
    return G.obj(("pageData", page_data)), qmaps, b


def build_qlinks(params, qmaps):
    entries = []
    for page, direction, assigned in qmaps:
        qmap = G.Obj(("Q-Link %d" % q, assigned.get(q, -1)) for q in range(1, 17))
        entries.append(G.obj(("Tab", page["fn"] + 1), ("SubTab", page["sub"] + 1), ("Bank Direction", direction), ("Q-Links", qmap)))
    program = G.Obj(("Q-Link %d" % (q + 1), params[n].index) for q, n in enumerate(
        ["Breath", "Pressed", "Bright", "Glide", "Breathy", "Rough", "Fry", "Ring", "Vibrato", "Vib Rate", "Jaw", "Lips Open",
         "Rounding", "Soft Palate", "Larynx", "Gain"]))
    return G.obj(("version", 4),
                 ("info", G.obj(("version", 1), ("type", "CompleteDescription"))),
                 ("Screen Mode Q-Links", G.obj(("version", 4), ("map", entries))),
                 ("Program Mode Q-Links", program))


# --------------------------------------------------------------------------------------------
# Previews: each tab drawn from TUI.json's own entries, at the parameters' defaults
# --------------------------------------------------------------------------------------------
def oget(o, key, default=None):
    for k, v in o:
        if k == key:
            return v
    return default


class Previewer:
    def __init__(self, tui, params_by_index):
        pd = oget(tui, "pageData")
        self.defs = {oget(e, "key"): oget(e, "value") for e in oget(oget(pd, "componentDefinitions"), "localComponentDefinitions")}
        self.repl = {oget(e, "key"): oget(e, "value") for e in oget(pd, "replacementStrings")}
        self.tabs = oget(pd, "tabs")
        self.params = params_by_index

    def param(self, handle):
        if not handle or not handle.startswith(("Parameter ", "ResetOnGestureEnd/")):
            return None
        return self.params.get(int(handle.split(" ")[-1]))

    def value(self, handle):
        p = self.param(handle)
        return (p.default or 0.0) if p else 0.0

    def steps(self, handle):
        p = self.param(handle)
        base = handle.split("/")[-1]
        if base in self.repl:
            return len(self.repl[base])
        return len(p.choices) if p and p.choices else 0

    def text(self, handle):
        p = self.param(handle)
        if not p:
            return ""
        base = handle.split("/")[-1]
        n = self.steps(handle)
        if base in self.repl and n:
            return self.repl[base][int(round(self.value(handle) * (n - 1)))]
        return p.display or PREVIEW_SAMPLE.get(p.name, "")

    def enabled(self, h):
        """IndexedEnabling/<choice>/<n>/<handle> and RangedEnabling/<min>/<max>/<n>/<handle> at the
        parameters' defaults"""
        parts = h.split("/")
        if parts[0] == "IndexedEnabling":
            k, n, hd = int(parts[1]), int(parts[2]), "/".join(parts[3:])
            return int(round(self.value(hd) * (n - 1))) == k
        if parts[0] == "RangedEnabling":
            lo, hi, n, hd = int(parts[1]), int(parts[2]), int(parts[3]), "/".join(parts[4:])
            return lo <= int(round(self.value(hd) * (n - 1))) <= hi
        return True

    def draw(self, tab):
        img = Image.new("RGBA", (W, H), (0, 0, 0, 255))
        root = oget(tab, "componentDefinition")
        self.children(img, oget(root, "componentsData"), 0, 0, {})
        return img

    def children(self, img, entries, ox, oy, remap):
        for e in entries:
            cd = oget(e, "componentData")
            typ, data = oget(cd, "type"), oget(cd, "data")
            x, y, w, h = (float(v) for v in oget(oget(e, "bounds"), "bounds").split())
            x, y = x + ox, y + oy
            inv = oget(oget(e, "bounds"), "additionalInvalidatingHandles") or []
            if any(not self.enabled(h_) for h_ in inv):
                continue
            local = {oget(m, "key"): oget(m, "value") for m in oget(oget(e, "handle remapping"), "map")}
            rm = {k: remap.get(v, v) for k, v in local.items()}

            def handle(name):
                return rm.get(name, remap.get(name, name))
            if typ == "Image":
                src = IMAGES[oget(data, "image")]
                if oget(data, "imageType") == "FilmStrip":
                    n = oget(data, "numFrames")
                    fh = src.height // n
                    fr = int(round(self.value(handle("Data")) * (n - 1)))
                    src = src.crop((0, fr * fh, src.width, (fr + 1) * fh))
                if src.size != (int(w), int(h)):
                    src = src.resize((max(1, int(w)), max(1, int(h))), Image.LANCZOS)
                img.alpha_composite(src.convert("RGBA"), (int(x), int(y)))
            elif typ == "Knob":
                src = IMAGES[oget(data, "filmStrip")]
                n = oget(data, "numFrames")
                fh = src.height // n
                fr = int(round(self.value(handle("Data")) * (n - 1)))
                img.alpha_composite(src.crop((0, fr * fh, src.width, (fr + 1) * fh)), (int(x), int(y)))
            elif typ == "Button":
                hd = handle("Data")
                k, n = oget(data, "buttonId"), oget(data, "numButtonsInGroup")
                v = self.value(hd)
                on = v >= 0.5 if n <= 1 else int(round(v * (max(self.steps(hd), n) - 1))) == k
                img.alpha_composite(IMAGES[oget(data, "onImage" if on else "offImage")], (int(x), int(y)))
            elif typ == "Label":
                ts = oget(data, "textStyle")
                fnt = oget(ts, "font")
                kind = oget(data, "type")
                text = oget(data, "text", "") if kind == "Text" else self.text(handle("Data")) if kind == "Value" else (self.param(handle("Data")).name if self.param(handle("Data")) else "")
                if text:
                    path = ROBOTO_LIGHT if oget(fnt, "style") in ("Light", "Thin") else ROBOTO
                    f = font(path, oget(fnt, "height"))
                    col = oget(ts, "colour")
                    color = tuple(int(col[i:i + 2], 16) for i in (2, 4, 6))
                    j = oget(ts, "justification")
                    ha = "r" if "right" in j else "m" if "horizontallyCentred" in j else "l"
                    tx = x + w if ha == "r" else x + w / 2 if ha == "m" else x
                    ImageDraw.Draw(img).text((tx, y + h / 2), text, font=f, fill=color + (255,), anchor=ha + "m")
            elif typ == "XYPad":
                puck = IMAGES[oget(data, "image")]
                px = x + self.value(oget(data, "xPositionHandle")) * w
                py = y + (1 - self.value(oget(data, "yPositionHandle"))) * h
                img.alpha_composite(puck, (int(px - puck.width / 2), int(py - puck.height / 2)))
            elif typ in ("Focus",):
                pass
            elif typ in self.defs:
                sub = oget(self.defs[typ], "componentsData")
                self.children(img, sub, x, y, rm)


# --------------------------------------------------------------------------------------------
def main():
    ap = argparse.ArgumentParser(description="Draws the CHIMERA MPC skin.")
    ap.parse_args()
    for f in (PARAMS, FACE, HERSHEY):
        if not os.path.isfile(f):
            sys.exit("make_skin: %s not found" % f)
    plist = G.load_params(PARAMS)
    params = {p.name: p for p in plist}
    with open(PARAMS, encoding="utf-8") as f:
        raw = {p["name"]: p for p in json.load(f)["params"]}
    units = {n: UNIT_OVERRIDE.get(n, UNITS.get(raw[n].get("label", ""), raw[n].get("label", "").upper())) for n in raw}
    for page in PAGES:
        for _, names in page.get("groups", []):
            for n in names:
                if n and n not in params:
                    sys.exit('make_skin: page %s uses unknown parameter "%s"' % (page["name"], n))
    with open(FACE, encoding="utf-8") as f:
        face = json.load(f)
    if "head" not in face["rest"] or "lower" not in face:
        sys.exit("make_skin: build/face.json has no layer frames; rerun the engine's dump_face example")
    os.makedirs(SKINS, exist_ok=True)
    os.makedirs(PREVIEW, exist_ok=True)
    for f in os.listdir(SKINS):
        if f.lower().endswith((".png", ".json")):
            os.remove(os.path.join(SKINS, f))
    for f in os.listdir(PREVIEW):
        if f.lower().endswith(".png"):
            os.remove(os.path.join(PREVIEW, f))

    global DIAL_STRIPS
    DIAL_STRIPS = [dial_strip(c) for c in SLOTS]
    placements = make_face(face)
    placements["velum_frames"] = len(face["velum"])
    put("puck.png", _puck())
    tui, qmaps, b = build_tui(params, units, placements)

    # every parameter the skin must reach is on some page (Face, the old 33-pose strip, stays unused)
    shown = set(b.referenced)
    missing = [p.name for p in plist if p.index not in shown and p.name != "Face"]
    if missing:
        sys.exit("make_skin: not on any page: %s" % ", ".join(missing))

    # write the pictures the TUI uses, and check nothing is missing or unused
    used = G.referenced_images(tui, set())
    unknown = sorted(u for u in used if u not in IMAGES)
    if unknown:
        sys.exit("make_skin: TUI refers to pictures that were not drawn: %s" % ", ".join(unknown))
    unused = sorted(n for n in IMAGES if n not in used)
    for n in unused:
        del IMAGES[n]
    for name, im in IMAGES.items():
        im.save(os.path.join(SKINS, name), optimize=True)
    with open(os.path.join(SKINS, "TUI.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(G.dump_json(tui) + "\n")
    with open(os.path.join(SKINS, "Q-Links.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(G.dump_json(build_qlinks(params, qmaps)) + "\n")
    on_disk = {f for f in os.listdir(SKINS) if f.lower().endswith(".png")}
    if used - on_disk:
        sys.exit("make_skin: missing on disk: %s" % ", ".join(sorted(used - on_disk)))
    if on_disk - used:
        sys.exit("make_skin: unused on disk: %s" % ", ".join(sorted(on_disk - used)))

    postures = posture_previews(face, placements)
    pv = Previewer(tui, {p.index: p for p in plist})
    for pi, (page, tab) in enumerate(zip(PAGES, pv.tabs)):
        pv.draw(tab).convert("RGB").save(os.path.join(PREVIEW, "page_%d_%s.png" % (pi + 1, page["name"].lower())))

    decoded = 0
    for f in on_disk:
        with Image.open(os.path.join(SKINS, f)) as im:
            decoded += im.width * im.height * 4
    disk = sum(os.path.getsize(os.path.join(SKINS, f)) for f in os.listdir(SKINS))
    big = sorted(((IMAGES[n].width * IMAGES[n].height * 4, n) for n in IMAGES), reverse=True)[:5]
    print("make_skin: %d pictures, %.0f KB on disk, %.2f MB decoded (%s); grouping %s; previews in %s"
          % (len(on_disk), disk / 1024.0, decoded / 1e6, ", ".join("%s %.2f MB" % (n, s / 1e6) for s, n in big), GROUPING, PREVIEW))
    face_mb = sum(IMAGES[f].width * IMAGES[f].height * 4 for _, f, _, _, _, _ in placements["layers"]) / 1e6
    print("make_skin: the face: head %s, %d layer strips %.2f MB decoded (%s); fan %d lines (%d end short of the lowest tongue); "
          "posture previews face_<%s>.png"
          % (placements["head"], len(placements["layers"]), face_mb,
             ", ".join("%s %dx%dx%d" % (n, bd[2], bd[3], k) for n, _, k, _, bd, w in placements["layers"]),
             placements["fan_lines"], placements["fan_short"], "|".join(postures)))


def _puck():
    ink = Ink(40, 40)
    ink.circle(20, 20, 10, core(GREEN), 3.0)
    return ink.rgba()


DIAL_STRIPS = []

if __name__ == "__main__":
    main()
