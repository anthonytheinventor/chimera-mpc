#!/usr/bin/env python3
"""make_skin.py - draws CHIMERA MPC's screens and writes its hand-made MPC skin (a first version).

    python "plugins/CHIMERA MPC/tools/make_skin.py"

Reads   build/params.json   (written by the build; run build_chimera.py first)
        build/face.json     (the tract's outlines: cargo run --release --example dump_face in engine/)
Writes  skins/TUI.json, skins/Q-Links.json and every PNG they use
        build/preview/page_N_<name>.png   (a mock-up of each page at the defaults)

The layout follows build-tmp/chimera-mpc/ui-design.md (the design report): French 36's four Q-Link
columns of four (Bank Direction "Column"), two each side of a central screen; each column is one of
CHIMERA's "pages of four", its encoders green, red, pink and white from top to bottom, as CHIMERA
colors its four control slots. The look is CHIMERA's vector display: glowing outlines on black,
blue for the body, amber for sound, captions in Hershey Simplex (OFL, CHIMERA's own font, baked
into the pictures; the MPC's live text is Roboto).

The center of the mode pages is the vocal tract seen from the side, from CHIMERA's own geometry:
the parts that stay put are baked into the background; the moving mouth (tongue, jaw, lips,
lower teeth, the floor and walls around them) is one opaque film strip bound to the read-only
"Face" parameter, whose frame the plugin picks as the posture nearest to what the voice is doing;
the soft palate is a transparent strip over it, bound to "Velum Pos". This is a probe: it tests
whether layered strips read well on the MPC before the full head (skull, skin, eye) is drawn.

Moving parts are reported by the plugin's helper thread (about 20 a second, quiet while a knob
moves). No Max[...] handles (they crashed the MPC One+ in French 36).
"""

import argparse
import json
import math
import os
import sys

from PIL import Image, ImageDraw, ImageFilter, ImageFont

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
# Hershey Simplex has no IPA: phonetic symbols are drawn in Segoe UI (Windows' own font)
IPA_FONT = os.path.join(os.environ.get("WINDIR", r"C:\Windows"), "Fonts", "segoeui.ttf")


def font_for(text):
    return HERSHEY if all(ord(ch) < 128 for ch in text) else IPA_FONT

# --------------------------------------------------------------------------------------------
# Geometry (the MPC's tab area is 1280 x 628)
# --------------------------------------------------------------------------------------------
W, H = 1280, 628
SS = 2                                  # supersampling for the drawings
COL_X = [24, 214, 884, 1074]
COL_W = 182
ROW_Y = [50, 189, 328, 467]
CELL_H = 139
COL_TOP, COL_BOT = 50, 606
CENTER = (404, 0, 876, 628)
KN = 64                                 # knob film-strip frame size
KNOB_FRAMES = 32
TOGGLE = (96, 44)
CHIP = (106, 34)                        # mode and body chips
FACE_SCALE = 26.0                       # px per cm on the face
FACE_ORIGIN = (571, 356)                # where body (0, 0) lands, px

# --------------------------------------------------------------------------------------------
# CHIMERA's palette (src/ui/colors.ts)
# --------------------------------------------------------------------------------------------
def hexrgb(h):
    h = h.lstrip("#")
    return tuple(int(h[i:i + 2], 16) for i in (0, 2, 4))


BLUE, BLUE_MID, BLUE_DIM, FRAME = hexrgb("4fb6f2"), hexrgb("3487c4"), hexrgb("1f5f8c"), hexrgb("133954")
GREEN, RED, PINK, WHITE, AMBER = hexrgb("5be381"), hexrgb("ff5d40"), hexrgb("ff73cf"), hexrgb("eef6f8"), hexrgb("ffb340")
SLOTS = [GREEN, RED, PINK, WHITE]
LABEL, QUIET = hexrgb("a9dcf8"), hexrgb("7cbfe6")
BG = (2, 4, 8)
ARGB_BG = "ff020408"
ARGB_OVERLAY = "ff060a10"


def argb(rgb, a=255):
    return "%02x%02x%02x%02x" % ((a,) + tuple(rgb))


def mix(a, b, u):
    return tuple(int(round(x * (1 - u) + y * u)) for x, y in zip(a, b))


def core(c):
    return mix(c, (255, 255, 255), 0.55)


# --------------------------------------------------------------------------------------------
# Pages (the design report, 3.4). A column is (title, [four parameter names or None]).
# --------------------------------------------------------------------------------------------
PAGES = [
    {"name": "SING", "fn": 0, "sub": 0, "center": "face", "mode": 0,
     "cols": [("SONG", ["Vibrato", "Vib Rate", "Breathy", "Ring"]), ("VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
              ("TONE", ["Rough", "Fry", "Twang", "False Folds"]), ("LYRICS", ["Lyrics", "Start Syl", "Restart", "Gain"])]},
    {"name": "PLAY", "fn": 1, "sub": 0, "center": "face", "mode": 1,
     "cols": [("VOICE", ["Pitch", "Breath", "Pressed", "Vibrato"]), ("TONE", ["Bright", "Breathy", "Rough", "Fry"]),
              ("MOUTH", ["Rounding", "Lips Open", "Protrude", "Jaw"]), ("THROAT", ["Soft Palate", "Larynx", "Twang", "False Folds"])]},
    {"name": "VOWEL", "fn": 1, "sub": 1, "center": "vowel", "mode": 1,
     "cols": [("VOWEL", ["Front", "Open", "Soft Palate", "Rounding"]), ("TONGUE", ["Tongue Fwd", "Tongue Up", "Tip Fwd", "Tip Up"]),
              ("MORE", ["Tongue Root", "Tip Curl", "Glide", "Vib Rate"]), ("VOICE", ["Pitch", "Breath", "Pressed", "Bright"])]},
    {"name": "SPEAK", "fn": 2, "sub": 0, "center": "face", "mode": 2,
     "cols": [("SPEAK", ["Speech Rate", "Start Line", "Speech", "IPA"]), ("VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
              ("TONE", ["Breathy", "Rough", "Fry", "Ring"]), ("SHAPE", ["Jaw", "Lips Open", "Larynx", "Soft Palate"])]},
    {"name": "SOUNDS", "fn": 2, "sub": 1, "center": "sounds", "mode": 2,
     "cols": [("SPEAK", ["Speech Rate", "Start Line", "Speech", "IPA"]), ("VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
              ("TONE", ["Breathy", "Rough", "Fry", "Ring"]), ("SHAPE", ["Jaw", "Lips Open", "Larynx", "Soft Palate"])]},
    {"name": "BEATBOX", "fn": 3, "sub": 0, "center": "steps", "mode": 3, "cols": None},
    {"name": "KIT", "fn": 3, "sub": 1, "center": "pads", "mode": 3,
     "cols": [("BEAT", ["Beat", "Tempo", "Swing", "Mic FX"]), ("HUM VOICE", ["Breath", "Pressed", "Bright", "Glide"]),
              ("TRANSPORT", ["Follow Host", "Play Beat", "Gain", None]), ("", [None, None, None, None])]},
    {"name": "BODY", "fn": 4, "sub": 0, "center": "body", "mode": None,
     "cols": [("BODY", ["Body", "Female", "Child", "Gain"]), ("SETUP", ["MPE", "Restart", "Whisper", "Breathe"]),
              ("", [None, None, None, None]), ("", [None, None, None, None])]},
]
BIPOLAR = {"Larynx", "Tongue Root"}
MODES = ["SING", "PLAY", "SPEAK", "BEATBOX"]
BODIES = ["MALE", "FEMALE", "CHILD", "BLEND"]
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
VOWEL_BOX = (424, 70, 432, 360)


# --------------------------------------------------------------------------------------------
# Drawing helpers
# --------------------------------------------------------------------------------------------
_fonts = {}


def hershey(px, text=""):
    path = font_for(text)
    key = (path, px)
    if key not in _fonts:
        _fonts[key] = ImageFont.truetype(path, px)
    return _fonts[key]


def roboto(px):
    key = ("r", px)
    if key not in _fonts:
        for name in ("Roboto-Bold.ttf", "arialbd.ttf", "DejaVuSans-Bold.ttf"):
            try:
                _fonts[key] = ImageFont.truetype(name, px)
                break
            except OSError:
                continue
        else:
            _fonts[key] = ImageFont.load_default()
    return _fonts[key]


def glow_layer(size, draw_fn, color, blur, strength=1.0):
    """draw_fn(draw, s) on a supersampled transparent layer; a blurred copy under it makes the glow"""
    w, h = size
    big = Image.new("RGBA", (w * SS, h * SS), (0, 0, 0, 0))
    draw_fn(ImageDraw.Draw(big), SS)
    small = big.resize((w, h), Image.LANCZOS)
    halo = small.filter(ImageFilter.GaussianBlur(blur))
    if strength != 1.0:
        a = halo.getchannel("A").point(lambda v: min(255, int(v * strength)))
        halo.putalpha(a)
    out = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    out.alpha_composite(halo)
    out.alpha_composite(halo)
    out.alpha_composite(small)
    return out


def text_layer(size, xy, text, font, color, anchor="la", blur=2.0):
    def fn(d, s):
        f = ImageFont.truetype(font.path, int(font.size * s))
        d.text((xy[0] * s, xy[1] * s), text, font=f, fill=color + (255,), anchor=anchor)
    return glow_layer(size, fn, color, blur, 0.8)


def frame_rect(d, box, color, width=1, s=1):
    x0, y0, x1, y1 = box
    d.rectangle((x0 * s, y0 * s, x1 * s, y1 * s), outline=color + (255,), width=width * s)


# --------------------------------------------------------------------------------------------
# The face
# --------------------------------------------------------------------------------------------
def face_px(p, ox=0, oy=0):
    return (FACE_ORIGIN[0] + p[0] * FACE_SCALE - ox, FACE_ORIGIN[1] - p[1] * FACE_SCALE - oy)


def chaikin(pts, it=2):
    for _ in range(it):
        if len(pts) < 3:
            return pts
        out = [pts[0]]
        for a, b in zip(pts, pts[1:]):
            out += [(0.75 * a[0] + 0.25 * b[0], 0.75 * a[1] + 0.25 * b[1]), (0.25 * a[0] + 0.75 * b[0], 0.25 * a[1] + 0.75 * b[1])]
        out.append(pts[-1])
        pts = out
    return pts


# (part, color, width, closed, smooth): the moving mouth, in drawing order
MOUTH_PARTS = [("inner", BLUE, 1.4, False, 0), ("epiglottis", BLUE, 2.0, False, 0), ("lower_teeth", BLUE_MID, 1.2, False, 0),
               ("jaw_bone", RED, 1.6, False, 0), ("tongue", GREEN, 1.8, False, 1), ("upper_lip", PINK, 1.6, False, 2),
               ("lower_lip", PINK, 1.6, False, 2), ("outer", BLUE, 1.5, False, 0)]
STATIC_PARTS = [("nasal_cavity", BLUE_MID, 1.1, True, 0), ("upper_teeth", BLUE_MID, 1.2, True, 0)]


def draw_parts(size, frame, parts, ox=0, oy=0, skip_near=None):
    """the parts of one tract frame on a transparent layer of this size, offset by (ox, oy) px"""
    layers = Image.new("RGBA", size, (0, 0, 0, 0))
    for name, color, width, closed, smooth in parts:
        pts = [tuple(p) for p in frame[name]]
        if skip_near is not None:
            # the outer wall along the soft palate: the palate's own strip draws that stretch
            pts = [p for p in pts if min(math.hypot(p[0] - q[0], p[1] - q[1]) for q in skip_near) > 0.35]
        if len(pts) < 2:
            continue
        if smooth:
            pts = chaikin(pts, smooth)
        if closed:
            pts = pts + [pts[0]]
        xy = [face_px(p, ox, oy) for p in pts]

        def fn(d, s, xy=xy, color=color, width=width):
            d.line([(x * s, y * s) for x, y in xy], fill=core(color) + (255,), width=max(1, int(width * s)), joint="curve")
        layers.alpha_composite(glow_layer(size, fn, color, 2.5, 1.0))
    return layers


def bbox_of(frames, parts, margin=10):
    xs, ys = [], []
    for f in frames:
        for name, *_ in parts:
            for p in f[name]:
                x, y = face_px(p)
                xs.append(x)
                ys.append(y)
    return (int(min(xs)) - margin, int(min(ys)) - margin, int(max(xs)) + margin, int(max(ys)) + margin)


# --------------------------------------------------------------------------------------------
# Controls
# --------------------------------------------------------------------------------------------
def knob_frame(t, color, bipolar):
    """one dial: a dim ring with ticks, the value arc from the minimum (or the middle), a pointer"""
    def fn(d, s):
        c = KN * s / 2
        r = (KN / 2 - 9) * s
        a0, a1 = 135, 405
        d.arc((c - r, c - r, c + r, c + r), a0, a1, fill=BLUE_DIM + (255,), width=int(2 * s))
        for k in range(11):
            a = math.radians(a0 + (a1 - a0) * k / 10)
            d.line((c + math.cos(a) * (r + 3 * s), c + math.sin(a) * (r + 3 * s), c + math.cos(a) * (r + 6 * s), c + math.sin(a) * (r + 6 * s)),
                   fill=BLUE_DIM + (255,), width=int(1 * s))
        at = a0 + (a1 - a0) * t
        start = (a0 + a1) / 2 if bipolar else a0
        lo, hi = min(start, at), max(start, at)
        if hi - lo > 0.5:
            d.arc((c - r, c - r, c + r, c + r), lo, hi, fill=core(color) + (255,), width=int(3 * s))
        a = math.radians(at)
        d.line((c + math.cos(a) * r * 0.25, c + math.sin(a) * r * 0.25, c + math.cos(a) * r * 0.92, c + math.sin(a) * r * 0.92),
               fill=core(color) + (255,), width=int(3 * s))
    return glow_layer((KN, KN), fn, color, 2.5, 1.0)


def knob_strip(color, bipolar):
    frames = [knob_frame(k / (KNOB_FRAMES - 1), color, bipolar) for k in range(KNOB_FRAMES)]
    out = Image.new("RGBA", (KN, KN * KNOB_FRAMES), (0, 0, 0, 0))
    for k, f in enumerate(frames):
        out.alpha_composite(f, (0, k * KN))
    return out


def keycap(size, label, color, on, font_px=17):
    w, h = size

    def fn(d, s):
        fill = color + (70,) if on else (0, 0, 0, 0)
        d.rounded_rectangle((2 * s, 2 * s, (w - 2) * s, (h - 2) * s), radius=6 * s, fill=fill,
                            outline=(core(color) if on else mix(color, BG, 0.55)) + (255,), width=int((2 if on else 1) * s))
        f = ImageFont.truetype(font_for(label), int(font_px * s))
        d.text((w * s / 2, h * s / 2), label, font=f, fill=(core(color) if on else mix(color, BG, 0.35)) + (255,), anchor="mm")
    return glow_layer(size, fn, color, 2.0 if on else 1.0, 1.0 if on else 0.4)


# --------------------------------------------------------------------------------------------
# Pages
# --------------------------------------------------------------------------------------------
def cell_box(c, r):
    return (COL_X[c], ROW_Y[r], COL_W, CELL_H)


def chips(names, x0, y, size, gap=6):
    return [(x0 + i * (size[0] + gap), y) for i in range(len(names))]


def caption_image(text, px, color, blur=1.0):
    f = hershey(px)
    w = int(f.getlength(text)) + 16
    h = px + 14
    return text_layer((w, h), (8, 4), text, f, color, blur=blur)


def make_overlays(page, face, mouth_box):
    """The page's pictures, as (file, image, (x, y)): shared files are drawn once and reused."""
    out = []
    kind = page["center"]
    if page["cols"]:
        for c, (title, names) in enumerate(page["cols"]):
            if any(names):
                out.append(("col_frame.png", None, (COL_X[c] - 4, COL_TOP - 6)))
    if kind == "face":
        out.append(("face_rest.png", None, None))
        out.append(("credit.png", None, None))
    if kind == "vowel":
        out.append(("vowel_chart.png", None, (VOWEL_BOX[0], VOWEL_BOX[1])))
    if kind == "steps":
        for k in range(16):
            r, c = divmod(k, 4)
            out.append(("step_frame.png", None, (456 + c * 204, 64 + r * 140)))
    if kind == "sounds":
        out.append(("cap_sounds.png", None, (CENTER[0] + 70, 46)))
    if kind == "pads":
        out.append(("cap_pads1.png", None, (CENTER[0] + 70, 530)))
        out.append(("cap_pads2.png", None, (CENTER[0] + 50, 556)))
    if kind == "body":
        out.append(("cap_title.png", None, (CENTER[0] + 110, 260)))
        out.append(("cap_body1.png", None, (CENTER[0] + 40, 330)))
        out.append(("cap_body2.png", None, (CENTER[0] + 90, 356)))
    return out


def make_shared_images(face, mouth_box):
    """Every picture make_overlays names, drawn once."""
    im = {}
    w, h = COL_W + 8, COL_BOT - COL_TOP + 6
    col = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    frame_rect(ImageDraw.Draw(col), (0, 0, w - 1, h - 1), FRAME)
    im["col_frame.png"] = col
    sf = Image.new("RGBA", (196, 132), (0, 0, 0, 0))
    frame_rect(ImageDraw.Draw(sf), (0, 0, 195, 131), FRAME)
    im["step_frame.png"] = sf
    # the tract at rest: the parts that stay put, and the moving ones outside the mouth's box
    rest = face["rest"]
    full = Image.new("RGBA", (W, H), (0, 0, 0, 0))
    full.alpha_composite(draw_parts((W, H), rest, STATIC_PARTS))
    moving = draw_parts((W, H), rest, MOUTH_PARTS, skip_near=rest["velum"])
    bx0, by0, bx1, by1 = mouth_box
    ImageDraw.Draw(moving).rectangle((bx0, by0, bx1 - 1, by1 - 1), fill=(0, 0, 0, 0))
    full.alpha_composite(moving)
    box = full.getbbox()
    im["face_rest.png"] = full.crop(box)
    im["face_rest.png"].info["at"] = box[:2]
    credit = caption_image("VOICE: CHIMERA BY JAMESON HODGE", 13, QUIET)
    credit.info["at"] = (CENTER[0] + (CENTER[2] - CENTER[0] - credit.width) // 2, 400)
    im["credit.png"] = credit
    x, y, w, h = VOWEL_BOX
    chart = Image.new("RGBA", (w, h + 40), (0, 0, 0, 0))
    frame_rect(ImageDraw.Draw(chart), (0, 0, w - 1, h - 1), FRAME)
    for ipa, f1, f2 in VOWEL_CHART:
        u = (math.log(F2_RANGE[1]) - math.log(f2)) / (math.log(F2_RANGE[1]) - math.log(F2_RANGE[0]))
        v = (math.log(f1) - math.log(F1_RANGE[0])) / (math.log(F1_RANGE[1]) - math.log(F1_RANGE[0]))
        chart.alpha_composite(text_layer((w, h + 40), (12 + u * (w - 24), 12 + v * (h - 24)), ipa, hershey(26, ipa), BLUE, anchor="mm"))
    chart.alpha_composite(text_layer((w, h + 40), (w / 2, h + 20), "LEFT: FRONT    DOWN: OPEN", hershey(14), QUIET, anchor="mm", blur=1.0))
    im["vowel_chart.png"] = chart
    im["cap_sounds.png"] = caption_image("A CONSONANT BETWEEN TWO VOWELS", 15, LABEL)
    im["cap_pads1.png"] = caption_image("PADS: GM DRUM NOTES 35-59", 14, QUIET)
    im["cap_pads2.png"] = caption_image("C3 AND UP HUM TWO OCTAVES DOWN", 14, QUIET)
    im["cap_title.png"] = caption_image("CHIMERA MPC", 40, BLUE, blur=2.0)
    im["cap_body1.png"] = caption_image("VOICE: CHIMERA BY JAMESON HODGE (MIT)", 14, LABEL)
    im["cap_body2.png"] = caption_image("A SIMULATED AIRWAY, NOT SAMPLES", 14, QUIET)
    return im


# --------------------------------------------------------------------------------------------
# TUI.json
# --------------------------------------------------------------------------------------------
def label_data(label_type, color, height, justification="left verticallyCentred", style="Bold"):
    return G.obj(("version", 1),
                 ("textStyle", G.obj(("version", 1),
                                     ("font", G.obj(("version", 1), ("name", "Roboto"), ("style", style), ("height", float(height)))),
                                     ("colour", color), ("justification", justification), ("case", "Original"))),
                 ("type", label_type), ("handleName", "Data"))


def focus_entry(color, w, h):
    data = G.obj(("version", 1), ("backgroundColour", argb(color, 0x16)), ("outlineColour", argb(color)),
                 ("backgroundInset", 2.0), ("outlineThickness", 2.0))
    return G.entry("focus", "Focus", data, "0 0 %d %d" % (w, h), "No", "Show", "WhenFocussed")


def name_entry(row):
    return G.entry("name", "Label", label_data("Name", argb(SLOTS[row]), 19.0, "left verticallyCentred"), "8 4 170 26", "No", "Show", "Always")


def text_entry(name, text, color, bounds, height=18.0, justification="centred"):
    data = label_data("Text", argb(color), height, justification)
    data = G.Obj(list(data.items()) + [("text", text)]) if hasattr(data, "items") else data
    return G.entry(name, "Label", data, "%d %d %d %d" % bounds, "No", "Show", "Always")


def knob_def(row, bipolar, discrete):
    color = SLOTS[row]
    children = [focus_entry(color, COL_W, CELL_H), name_entry(row),
                G.entry("knob", "Knob", G.knob_filmstrip_data(knob_file(row, bipolar), KNOB_FRAMES),
                        "8 40 %d %d" % (KN, KN), "No", "Show", "Always"),
                G.entry("value", "Label", label_data("Value", argb(core(color)), 26.0),
                        "78 46 100 52", "No", "Show", "Always")]
    overlay = "menu overlay" if discrete else ("knob overlay bipolar" if bipolar else "knob overlay")
    return G.component_definition(G.control_actions(overlay), children, "00000000")


def toggle_def(row):
    children = [focus_entry(SLOTS[row], COL_W, CELL_H), name_entry(row),
                G.entry("toggle", "Button", G.button_data(toggle_file(row, True), toggle_file(row, False), 1, 1),
                        "10 50 %d %d" % TOGGLE, "No", "Show", "Always")]
    return G.component_definition(G.control_actions("menu overlay", double_click=False), children, "00000000")


def knob_file(row, bipolar):
    return "knob_%s%s.png" % (["green", "red", "pink", "white"][row], "_bi" if bipolar else "")


def toggle_file(row, on):
    return "toggle_%s_%s.png" % (["green", "red", "pink", "white"][row], "on" if on else "off")


def image_entry(name, image, bounds, invalidating=None):
    data = G.obj(("version", 2), ("imageType", "Regular"), ("colour", "0"), ("image", image))
    return G.entry(name, "Image", data, "%d %d %d %d" % bounds, "No", "Show", "Always", invalidating=invalidating)


def film_entry(name, image, frames, handle, bounds):
    data = G.obj(("version", 2), ("imageType", "FilmStrip"), ("colour", "0"), ("image", image),
                 ("numFrames", frames), ("handleName", handle))
    return G.entry(name, "Image", data, "%d %d %d %d" % bounds, "No", "Show", "Always")


def radio_def(image_on, image_off, k, n):
    children = [G.entry("b", "Button", G.button_data(image_on, image_off, k, n), "0 0 %d %d" % Image.open(os.path.join(SKINS, image_on)).size,
                        "No", "Show", "Always")]
    return G.component_definition([], children, "00000000")


def xypad_entry(name, hx, hy, puck, bounds):
    data = G.obj(("version", 1), ("xPositionHandle", hx), ("yPositionHandle", hy), ("image", puck))
    return G.entry(name, "XYPad", data, "%d %d %d %d" % bounds, "No", "Show", "Always")


def build_tui(params, face_frames, mouth_box, velum_box, n_velum):
    defs = []

    def need(name, make):
        if not any(k == name for k, _ in defs):
            defs.append((name, make()))
        return name
    referenced = {}
    tabs = []

    def control(children, p, defname, bounds):
        referenced[p.index] = p
        children.append(G.entry(p.name, defname, G.obj(("version", 1), ("handleName", "Data")), bounds,
                                "Yes", "Hide", "Always", remap=[("Data", p.handle)]))

    def radio_group(children, p, files, positions, momentary=False, first=0):
        """buttons for the steps first.. of p (p's choices are the group's steps)"""
        referenced[p.index] = p
        n = len(p.choices) if p.choices else len(files)
        for i, ((on, off), (x, y)) in enumerate(zip(files, positions)):
            k = first + i
            name = need("chimRadio %s %d" % (p.name, k), lambda on=on, off=off, k=k: radio_def(on, off, k, max(n, 2)))
            size = Image.open(os.path.join(SKINS, on)).size
            handle = ("ResetOnGestureEnd/" + p.handle) if momentary else p.handle
            children.append(G.entry("%s %d" % (p.name, k), name, G.obj(("version", 1), ("handleName", "Data")),
                                    "%d %d %d %d" % (x, y, size[0], size[1]), "No", "Hide", "Always", remap=[("Data", handle)]))

    face_p, velum_p = params["Face"], params["Velum Pos"]
    for page in PAGES:
        children = []
        for fname, _, at in make_overlays(page, None, mouth_box):
            img = SHARED[fname]
            x, y = at if at is not None else img.info["at"]
            children.append(image_entry(fname, fname, (x, y, img.width, img.height)))
        for c, (title, names) in enumerate(page["cols"] or []):
            if title:
                children.append(text_entry("title %d" % c, title, LABEL, (COL_X[c], 10, COL_W, 30), 20.0))
        if page["center"] == "steps":
            for k in range(16):
                r, c = divmod(k, 4)
                children.append(text_entry("step %d" % k, "STEP %d" % (k + 1), SLOTS[r], (456 + c * 204 + 6, 64 + r * 140 + 104, 120, 24), 16.0, "left verticallyCentred"))
        kind = page["center"]
        if kind == "face":
            mx0, my0, mx1, my1 = mouth_box
            vx0, vy0, vx1, vy1 = velum_box
            children.append(film_entry("face", "face_mouth.png", face_frames, face_p.handle, (mx0, my0, mx1 - mx0, my1 - my0)))
            children.append(film_entry("velum", "face_velum.png", n_velum, velum_p.handle, (vx0, vy0, vx1 - vx0, vy1 - vy0)))
        if page["mode"] is not None:
            radio_group(children, params["Mode"], [("chip_mode_%d_on.png" % k, "chip_mode_%d_off.png" % k) for k in range(4)],
                        chips(MODES, CENTER[0] + 16, 8, CHIP))
        if kind == "vowel":
            x, y, w, h = VOWEL_BOX
            # Open grows downward on the chart, and the pad's y grows upward: the skin shows it as is
            children.append(xypad_entry("vowel pad", params["Front"].handle, params["Open"].handle, "puck.png", (x, y, w, h)))
        if kind == "sounds":
            pos = [(CENTER[0] + 22 + (k % 5) * 88, 84 + (k // 5) * 62) for k in range(len(CONSONANTS))]
            radio_group(children, params["Consonant"], [("cons_%d_on.png" % k, "cons_%d_off.png" % k) for k in range(1, len(CONSONANTS) + 1)],
                        pos, momentary=True, first=1)
            radio_group(children, params["Pad Vowel"], [("pv_%d_on.png" % k, "pv_%d_off.png" % k) for k in range(4)],
                        chips(PAD_VOWELS, CENTER[0] + 16, 420, (106, 40)))
        if kind == "pads":
            pos = [(CENTER[0] + 22 + (k % 2) * 220, 60 + (k // 2) * 92) for k in range(len(PADS))]
            radio_group(children, params["Pad"], [("pad_%d_on.png" % k, "pad_%d_off.png" % k) for k in range(1, len(PADS) + 1)],
                        pos, momentary=True, first=1)
        if kind == "body":
            radio_group(children, params["Body"], [("chip_body_%d_on.png" % k, "chip_body_%d_off.png" % k) for k in range(4)],
                        chips(BODIES, CENTER[0] + 16, 8, CHIP))
        groups = []
        if page["cols"]:
            for c, (title, names) in enumerate(page["cols"]):
                groups.append("%d %d %d %d" % (COL_X[c] - 2, COL_TOP, COL_W + 4, COL_BOT - COL_TOP) if any(names) else "0 0 0 0")
                for r, name in enumerate(names):
                    if not name:
                        continue
                    p = params[name]
                    if p.kind == "bool":
                        dn = need("chimToggle %d" % r, lambda r=r: toggle_def(r))
                    else:
                        bi = name in BIPOLAR
                        dn = need("chimKnob %d %d %d" % (r, bi, p.is_discrete), lambda r=r, bi=bi, ds=p.is_discrete: knob_def(r, bi, ds))
                    control(children, p, dn, "%d %d %d %d" % cell_box(c, r))
        else:
            # BEATBOX: the 16 steps in rows of four (Bank Direction RowTopDown)
            for k in range(16):
                r, c = divmod(k, 4)
                p = params["Step %d" % (k + 1)]
                dn = need("chimKnob %d 0 1" % r, lambda r=r: knob_def(r, False, True))
                control(children, p, dn, "%d %d %d %d" % (456 + c * 204 + 6, 64 + r * 140 - 4, COL_W, CELL_H - 30))
            for r in range(4):
                groups.append("452 %d 820 %d" % (60 + r * 140, 140))
        tabs.append(G.obj(("version", 1), ("tabName", page["name"]), ("fnKeyIndex", page["fn"]), ("fnKeySubIndex", page["sub"]),
                          ("componentDefinition", G.component_definition([], children, ARGB_BG)),
                          ("qlinkBoundsData", groups)))
    defs.append(("knob overlay", G.overlay_component("KnobOverlay", G.knob_overlay_data(False), ARGB_OVERLAY)))
    defs.append(("knob overlay bipolar", G.overlay_component("KnobOverlay", G.knob_overlay_data(True), ARGB_OVERLAY)))
    defs.append(("menu overlay", G.overlay_component("MenuOverlay", G.menu_overlay_data(), ARGB_OVERLAY)))
    replacements = [G.obj(("key", p.handle), ("value", list(p.choices))) for _, p in sorted(referenced.items()) if p.is_discrete and p.choices]
    page_data = G.obj(
        ("version", 1),
        ("componentDefinitions", G.obj(("version", 2), ("importFiles", []),
                                       ("localComponentDefinitions", [G.obj(("key", k), ("value", v)) for k, v in defs]))),
        ("replacementStrings", replacements),
        ("info", G.obj(("version", 1), ("type", "CompleteDescription"))),
        ("tabs", tabs))
    return G.obj(("pageData", page_data))


def build_qlinks(params):
    entries = []
    for ti, page in enumerate(PAGES):
        assigned = {}
        if page["cols"]:
            for c, (title, names) in enumerate(page["cols"]):
                for r, name in enumerate(names):
                    if name:
                        assigned[13 + c - 4 * r] = params[name].index
            direction = "Column"
        else:
            for k in range(16):
                r, c = divmod(k, 4)
                assigned[13 + c - 4 * r] = params["Step %d" % (k + 1)].index
            direction = "RowTopDown"
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
def main():
    ap = argparse.ArgumentParser(description="Draws the CHIMERA MPC skin.")
    ap.parse_args()
    for f in (PARAMS, FACE, HERSHEY):
        if not os.path.isfile(f):
            sys.exit("make_skin: %s not found" % f)
    params = {p.name: p for p in G.load_params(PARAMS)}
    for page in PAGES:
        for _, names in page["cols"] or []:
            for n in names:
                if n and n not in params:
                    sys.exit('make_skin: page %s uses unknown parameter "%s"' % (page["name"], n))
    face = json.load(open(FACE, encoding="utf-8"))
    os.makedirs(SKINS, exist_ok=True)
    os.makedirs(PREVIEW, exist_ok=True)
    for f in os.listdir(SKINS):
        if f.lower().endswith((".png", ".json")):
            os.remove(os.path.join(SKINS, f))
    images = {}

    # the face: the mouth strip (opaque: the static parts inside its box are drawn in every frame)
    frames = face["face"]
    mouth_box = bbox_of(frames + [face["rest"]], [("tongue",), ("jaw_bone",), ("lower_teeth",), ("lower_lip",), ("upper_lip",)], 10)
    mx0, my0, mx1, my1 = mouth_box
    mw, mh = mx1 - mx0, my1 - my0
    strip = Image.new("RGBA", (mw, mh * len(frames)), BG + (255,))
    for k, fr in enumerate(frames):
        cell = Image.new("RGBA", (mw, mh), BG + (255,))
        cell.alpha_composite(draw_parts((mw, mh), face["rest"], STATIC_PARTS, mx0, my0))
        cell.alpha_composite(draw_parts((mw, mh), fr, MOUTH_PARTS, mx0, my0, skip_near=fr["velum"]))
        strip.alpha_composite(cell, (0, k * mh))
    images["face_mouth.png"] = strip.convert("RGB")
    vel = face["velum"]
    velum_box = bbox_of(vel, [("velum",)], 8)
    vx0, vy0, vx1, vy1 = velum_box
    vw, vh = vx1 - vx0, vy1 - vy0
    vstrip = Image.new("RGBA", (vw, vh * len(vel)), (0, 0, 0, 0))
    for k, fr in enumerate(vel):
        vstrip.alpha_composite(draw_parts((vw, vh), fr, [("velum", WHITE, 1.4, False, 3)], vx0, vy0), (0, k * vh))
    images["face_velum.png"] = vstrip

    # knobs, toggles, chips, pads, consonants
    bipolar_rows = {r for page in PAGES for _, names in (page["cols"] or []) for r, n in enumerate(names) if n in BIPOLAR}
    for row in range(4):
        images[knob_file(row, False)] = knob_strip(SLOTS[row], False)
        if row in bipolar_rows:
            images[knob_file(row, True)] = knob_strip(SLOTS[row], True)
        for on in (True, False):
            images[toggle_file(row, on)] = keycap(TOGGLE, "ON" if on else "OFF", SLOTS[row], on)
    for k, m in enumerate(MODES):
        for on in (True, False):
            images["chip_mode_%d_%s.png" % (k, "on" if on else "off")] = keycap(CHIP, m, SLOTS[k], on, 16)
    for k, b in enumerate(BODIES):
        for on in (True, False):
            images["chip_body_%d_%s.png" % (k, "on" if on else "off")] = keycap(CHIP, b, SLOTS[k], on, 16)
    for prefix, labels, size, color in (("cons", CONSONANTS, (80, 54), AMBER), ("pad", ["%s  %s" % p for p in PADS], (212, 82), AMBER)):
        for k, lab in enumerate(labels):
            for on in (True, False):
                images["%s_%d_%s.png" % (prefix, k + 1, "on" if on else "off")] = keycap(size, lab, color, on, 26 if prefix == "cons" else 18)
    for k, v in enumerate(PAD_VOWELS):
        for on in (True, False):
            images["pv_%d_%s.png" % (k, "on" if on else "off")] = keycap((106, 40), v, SLOTS[k], on, 22)
    puck = glow_layer((40, 40), lambda d, s: d.ellipse((10 * s, 10 * s, 30 * s, 30 * s), outline=core(GREEN) + (255,), width=int(3 * s)), GREEN, 3.0)
    images["puck.png"] = puck
    SHARED.update(make_shared_images(face, mouth_box))
    images.update(SHARED)
    for name, im in images.items():
        im.save(os.path.join(SKINS, name), optimize=True)

    with open(os.path.join(SKINS, "TUI.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(G.dump_json(build_tui(params, len(frames), mouth_box, velum_box, len(vel))) + "\n")
    with open(os.path.join(SKINS, "Q-Links.json"), "w", encoding="utf-8", newline="\n") as f:
        f.write(G.dump_json(build_qlinks(params)) + "\n")

    # previews: each page at its defaults (the face at rest, knobs at their defaults)
    for pi, page in enumerate(PAGES):
        img = Image.new("RGBA", (W, H), BG + (255,))
        for fname, _, at in make_overlays(page, None, mouth_box):
            sh = SHARED[fname]
            img.alpha_composite(sh, at if at is not None else sh.info["at"])
        for c, (title, names) in enumerate(page["cols"] or []):
            if title:
                tw = ImageDraw.Draw(img).textlength(title, font=roboto(20))
                ImageDraw.Draw(img).text((COL_X[c] + (COL_W - tw) / 2, 14), title, font=roboto(20), fill=LABEL + (255,))
            for r, name in enumerate(names):
                if name:
                    ImageDraw.Draw(img).text((COL_X[c] + 8, ROW_Y[r] + 6), name, font=roboto(18), fill=SLOTS[r] + (255,))
        if page["center"] == "steps":
            for k in range(16):
                r, c = divmod(k, 4)
                p = params["Step %d" % (k + 1)]
                x, y = 456 + c * 204 + 6, 64 + r * 140 - 4
                t = p.default if p.default is not None else 0.0
                fr = int(round(t * (KNOB_FRAMES - 1)))
                img.alpha_composite(images[knob_file(r, False)].crop((0, fr * KN, KN, (fr + 1) * KN)), (x + 8, y + 40))
                ImageDraw.Draw(img).text((x + 80, y + 60), p.display or "", font=roboto(22), fill=core(SLOTS[r]) + (255,))
                ImageDraw.Draw(img).text((x + 8, y + 6), p.name, font=roboto(18), fill=SLOTS[r] + (255,))
        if page["center"] == "face":
            img.alpha_composite(strip.crop((0, 0, mw, mh)), (mx0, my0))
            img.alpha_composite(vstrip.crop((0, 0, vw, vh)), (vx0, vy0))
        if page["center"] == "sounds":
            for k in range(len(CONSONANTS)):
                img.alpha_composite(images["cons_%d_off.png" % (k + 1)], (CENTER[0] + 22 + (k % 5) * 88, 84 + (k // 5) * 62))
            for k, (x, y) in enumerate(chips(PAD_VOWELS, CENTER[0] + 16, 420, (106, 40))):
                img.alpha_composite(images["pv_%d_%s.png" % (k, "on" if k == 0 else "off")], (x, y))
        if page["center"] == "pads":
            for k in range(len(PADS)):
                img.alpha_composite(images["pad_%d_off.png" % (k + 1)], (CENTER[0] + 22 + (k % 2) * 220, 60 + (k // 2) * 92))
        if page["center"] == "body":
            for k, (x, y) in enumerate(chips(BODIES, CENTER[0] + 16, 8, CHIP)):
                img.alpha_composite(images["chip_body_%d_%s.png" % (k, "on" if k == 0 else "off")], (x, y))
        if page["mode"] is not None:
            for k, (x, y) in enumerate(chips(MODES, CENTER[0] + 16, 8, CHIP)):
                img.alpha_composite(images["chip_mode_%d_%s.png" % (k, "on" if k == page["mode"] else "off")], (x, y))
        for c, (title, names) in enumerate(page["cols"] or []):
            for r, name in enumerate(names):
                if not name:
                    continue
                p = params[name]
                x, y, _, _ = cell_box(c, r)
                if p.kind == "bool":
                    img.alpha_composite(images[toggle_file(r, (p.default or 0) >= 0.5)], (x + 10, y + 56))
                else:
                    t = p.default if p.default is not None else 0.0
                    fr = int(round(t * (KNOB_FRAMES - 1)))
                    ks = images[knob_file(r, name in BIPOLAR)]
                    img.alpha_composite(ks.crop((0, fr * KN, KN, (fr + 1) * KN)), (x + 8, y + 40))
                    ImageDraw.Draw(img).text((x + 80, y + 60), p.display or "", font=roboto(22), fill=core(SLOTS[r]) + (255,))
        img.convert("RGB").save(os.path.join(PREVIEW, "page_%d_%s.png" % (pi + 1, page["name"].lower())))
    total = sum(os.path.getsize(os.path.join(SKINS, f)) for f in os.listdir(SKINS))
    decoded = sum(im.width * im.height * 4 for im in images.values())
    print("make_skin: %d files, %.0f KB on disk, about %.1f MB decoded; previews in %s" % (len(os.listdir(SKINS)), total / 1024.0, decoded / 1e6, PREVIEW))


SHARED = {}

if __name__ == "__main__":
    main()
