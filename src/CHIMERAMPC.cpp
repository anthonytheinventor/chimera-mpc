// CHIMERA MPC - CHIMERA's physical-model voice on the MPC, by anthonytheinventor.
//
// CHIMERA (github.com/jamesondh/chimera-vocal-synth) is a voice synthesizer by Jameson Hodge,
// MIT licensed: every sound comes from a simulated airway (lungs, vocal folds, throat, mouth and
// nose), not from samples. Its Rust engine is built into this plugin as a static library
// (engine/, C interface in chimera_mpc.h) with the settings that fit a 32-bit MPC.
//
// Signal flow: MIDI notes -> the engine (Gain on the voice's level, before its limiter) -> mono
// voice -> both outputs.
//
// Modes:
//   Sing  each note sings the next syllable of the chosen lyrics (Content/lyrics.txt, one line a
//         slot), as CHIMERA's Sing mode does; Start Syl picks where the lyrics start.
//   Play  notes sing one held posture (legato: the last key held sets the pitch; the breath runs
//         while a key is held or Breathe is on). The articulators shape it, and the vowel pad
//         (Front, Open) blends CHIMERA's calibrated vowels and writes the result into them.
//   Speak each note says the next line of the chosen text (Content/speech.txt, blocks of lines),
//         its pitch moved by how far the key is from C3 (up to two octaves either way).
//   Beatbox the 16-step pattern (Beat picks a preset, which fills the steps), on its own tempo
//         with Play Beat, or following the MPC's transport with Follow Host; notes below C3 hit
//         drum sounds (General MIDI's drum map), notes from C3 up hum two octaves down. The
//         MPC's pads 1-16 play the BEATBOX squares: Pad 1 Note says which notes they send (a
//         chromatic run from it, the classic MPC map, or Learn: the next note is pad 1's), and
//         Last Note shows the note last played, so one tap shows what a pad sends. Each square
//         has a hum note (Note n: "-" is the hum's own, E1) and an Accent (1.3x), on the NOTES
//         page; picking a Beat fills them as it fills the steps.
// The screen shows what comes next: in Sing the syllable last sung (Now) and the one the next note
// sings (Next), in Speak the line last said and the next one (the engine's cm_now_next).
// Body: Male, Female, Child, or Blend (the engine's thread builds a voice for the mix; the
// previous body plays until it is ready). Restart: Sing and Speak start over at Start Syl and
// Start Line when the transport starts.
//
// Threads: the engine is created in prepare() (it allocates) and swapped in atomically; an old
// one is freed only once process() has let go of it. reset() only raises a flag that process()
// acts on, so the engine is only ever touched by one thread at a time. The lyrics are turned into
// syllables on the engine's own thread, which reads Content/.

#include "mpc_plugin.h"
#include "chimera_mpc.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <condition_variable>
#include <mutex>
#include <cmath>
#include <cstring>
#include <string>
#include <thread>
#include <vector>

namespace {

// Parameter order: the engine (engine/src/lib.rs, module p) reads the same list. Append only.
enum ParamIndex
{
    kGain = 0,      // dB; parameter 0, harmless when the host nudges it
    kBreath,        // lung pressure, cmH2O
    kPressed,       // vocal-fold tension
    kBright,        // brightness of the source
    kGlide,         // pitch glide between notes, s (shown in ms)
    kBreathy,       // breathiness
    kRough,         // roughness
    kFry,           // vocal fry
    kRing,          // the singer's ring (epilarynx)
    kVibrato,       // depth, cents
    kVibRate,       // Hz
    kBody,          // Male / Female / Child / Blend
    kBodyFemale,    // Blend: how female
    kBodyChild,     // Blend: how child
    kJaw,           // cm open
    kLipsOpen,      // lip aperture, cm
    kProtrude,      // lip protrusion, cm
    kRounding,      // 1 - lip width
    kRoot,          // tongue root, cm forward (+) or back (-)
    kTipCurl,       // tongue tip curl
    kSoftPalate,    // velum port, cm2 (shut at 0 or below)
    kLarynx,        // larynx height, cm
    kTwang,         // epilarynx narrowing
    kFalseFolds,    // ventricular folds
    kTongueFwd,     // tongue body, front-back (template cm)
    kTongueUp,      // tongue body, height
    kTipFwd,        // tongue tip, front-back (at its rest values the tip follows the jaw)
    kTipUp,         // tongue tip, height
    kVowelFront,    // vowel pad: F2, back to front
    kVowelOpen,     // vowel pad: F1, close to open
    kWhisper,       // unvoiced breath
    kPitch,         // Play: the pitch without a key, Hz
    kMode,          // Sing / Play / Speak / Beatbox
    kBreathe,       // Play: breathe without a key
    kRestart,       // Sing, Speak: back to Start Syl / Start Line when the transport starts
    kMpe,           // MPE: channels 2-16 are note channels (bend, pressure, slide per note)
    kLyrics,        // Sing: lyrics slot 1..8
    kStartSyl,      // Sing: the syllable the lyrics start from
    kSpeech,        // Speak: text slot 1..8 (Content/speech.txt)
    kStartLine,     // Speak: the line it starts from
    kSpeechRate,    // Speak: speed, x
    kIpa,           // Speak: the text is IPA
    kSpeak,         // Speak: say the next line (on its rise)
    kConsonant,     // Speak: say a consonant between two vowels (on a change to one)
    kPadVowel,      // Speak: the vowel around it
    kBeat,          // Beatbox: a preset pattern, or Your Beat
    kTempo,         // Beatbox: bpm
    kSwing,         // Beatbox: %
    kMic,           // Beatbox: the close-mic sound
    kFollow,        // Beatbox: follow the host's transport
    kPlayBeat,      // Beatbox: run the pattern on its own tempo
    kStep1,         // Beatbox: steps 1..16 (rest or a sound)
    kStep16 = kStep1 + 15,
    kPad,           // Beatbox: hit a sound now (on its rise)
    kNumSettings,
    // read-only displays for the skin (not automatable; the helper thread writes them)
    kVisFace = kNumSettings,   // the old 33-pose face strip's frame: no longer shown or reported (ids are permanent)
    kVisVelum,                 // the soft palate strip's frame (0 shut, then open)
    // typing words on the MPC
    kTypeInto,                 // Speak / Sing: which texts the keys edit (Speak's lines or Sing's lyrics)
    kTypeSlot,                 // 1..8: which slot
    kKey,                      // one key (on a change to it): the letters, punctuation, Del, Line, Clear
    kTyped,                    // read-only: its text is the end of the slot being typed (formatValue)
    kHit,                      // read-only: the pad to light on the screen (0 none, 1..10 as Pad)
    // read-only: the face's moving layers, each a film strip (engine/src/face.rs picks the frames)
    kVisJaw,                   // the jaw's frame (jaw 0..2.4 cm)
    kVisTongue,                // the tongue's frame (the nearest of the outlines clustered offline)
    kVisLips,                  // the upper lip's frame (its edge's height x how far forward)
    kVisLowerLip,              // the lower lip's frame (opening x how far forward; a strip per pair of jaw frames)
    kVisLarynx,                // the larynx's frame (height -1.5..1.5 cm)
    kVisSound,                 // the sound fan's brightness: 0 silent .. 5 loudest
    kStepLit,                  // read-only: the BEATBOX square to light (0 none, 1..16: a pad hit, or the beat's step)
    kCursor,                   // WORDS: the typing cursor (the data wheel on the text box), kCursorEnd = at the end
    kTyped2,                   // read-only: the text box's second row (Typed is the first; formatValue)
    kPadNotes,                 // Beatbox: pad 1's note (0..112, a chromatic run from it), Classic (the classic MPC map) or Learn
    kLastNote,                 // read-only: the last note played (0 none, then 1 + the note), on the BEATBOX page
    kNow,                      // read-only: Sing's syllable last sung / Speak's line last said (formatValue)
    kNext,                     // read-only: the syllable / line the next note sings or says (formatValue)
    kStepNote1,                // Beatbox: each square's hum note ("-" the hum's own, then kNoteLow..kNoteHigh)
    kStepNote16 = kStepNote1 + 15,
    kStepAccent1,              // Beatbox: each square's accent (off / on: STEP_ACCENT, 1.3)
    kStepAccent16 = kStepAccent1 + 15,
    kPadMap1,                  // Beatbox: pad n's note for Pad 1 Note = Custom (Learn All fills them)
    kPadMap16 = kPadMap1 + 15,
    kNumParams
};

constexpr int kEngineParams = kPadNotes + 1;   // the engine reads 0..kPadNotes (engine/src/lib.rs, module p; it ignores the displays)
constexpr int kPadBaseMax = 112;   // Pad 1 Note's choices: 0..112 a note, then Classic, then Learn (engine PAD_BASE_MAX..)
constexpr int kPadLearn = kPadBaseMax + 2;
constexpr int kPadCustom = kPadBaseMax + 3;     // each pad's own note, from Pad Map 1-16
constexpr int kPadLearnAll = kPadBaseMax + 4;   // the next 16 Beatbox notes are pads 1 to 16
constexpr int kNoteLow = 24, kNoteHigh = 60;   // the steps' hum notes: C0..C3 as the MPC names them

bool isDisplay(int i) { return i == kVisFace || i == kVisVelum || i == kTyped || i == kHit || (i >= kVisJaw && i <= kVisSound) || i == kStepLit || i == kTyped2 || i == kLastNote || i == kNow || i == kNext; }

// A step's hum note: the Note choice (0 "-", then kNoteLow..) <-> the engine's MIDI note (0: the hum's own)
float stepNoteMidi(int choice) { return choice <= 0 ? 0.0f : (float)(kNoteLow + choice - 1); }
int stepNoteChoice(float midi) { return midi < 1.0f ? 0 : 1 + std::min(std::max((int)std::lround(midi), kNoteLow), kNoteHigh) - kNoteLow; }

// A note as the MPC names it (C-2 is note 0, C3 note 60): "61 C#3".
std::string noteName(int n)
{
    static const char* const names[12] = {"C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"};
    return std::to_string(n) + " " + names[n % 12] + std::to_string(n / 12 - 2);
}

// The momentary controls: a tap sets one and the MPC puts it back to 0 when the finger lifts, which
// with a quick tap can happen before the next block reads it. Their taps are caught as the host
// sets them (parameterChanged) and handed to the engine (cm_tap).
bool isTap(int i) { return i == kKey || i == kPad || i == kConsonant || i == kSpeak; }

constexpr int kVelumFrames = 8;    // engine/src/face.rs VELUM_FRAMES
constexpr int kFaceLayers = 6;     // cm_face: jaw, tongue, upper lip, lower lip, larynx, sound (kVisJaw.. in that order)
constexpr int kMaxFaceReports = 3; // the face's layers report at most this many frames a look
constexpr int kLookMs = 50;        // the helper thread looks this often
constexpr int kKnobLooks = 6;      // and stays quiet this many looks after a setting moves
constexpr int kHitLooks = 3;       // a hit's pad stays lit this many looks (about 150 ms)
// the typing cursor's steps: kCursorEnd at the end, each step down one character further back
constexpr int kCursorEnd = 600;   // engine/src/texts.rs TEXT_CAP (a slot's most characters)
constexpr int kResendEvery = 10;   // a new face frame is reported again every 10 looks (0.5 s)
constexpr int kResendLooks = 60;   // for 60 looks (3 s) after it changed

const std::vector<std::string> kSounds = {"B", "t", "Pf", "K", "^K", "!", "P!", "tss", "kch", "m"};

constexpr int kChunk = 256;   // frames rendered per engine call

// the vowel pad's output (cm_vowel_posture order) -> parameters
constexpr int kPadParams[9] = {kTongueFwd, kTongueUp, kJaw, kLipsOpen, kProtrude, kRounding, kRoot, kLarynx, kTipCurl};

} // namespace

class CHIMERAMPC : public mpc::Plugin
{
public:
    CHIMERAMPC() : mpc::Plugin(makeInfo())
    {
        float d[kNumParams] = {};
        cm_param_defaults(d, kNumParams);
        auto pct = [&](const char* id, const char* name, int i) { addParam(mpc::floatParam(id, name, 0.0f, 100.0f, d[i] * 100.0f, "%", 0)); };

        addParam(mpc::floatParam("gain",         "Gain",        -30.0f, 12.0f, 0.0f, "dB", 1));
        addParam(mpc::floatParam("breath",       "Breath",      2.0f, 22.0f, d[kBreath], "cmH2O", 1));
        pct("pressed", "Pressed", kPressed);
        pct("bright",  "Bright",  kBright);
        addParam(mpc::logParam  ("glide",        "Glide",       5.0f, 600.0f, d[kGlide] * 1000.0f, "ms", 0));
        pct("breathy", "Breathy", kBreathy);
        pct("rough",   "Rough",   kRough);
        pct("fry",     "Fry",     kFry);
        pct("ring",    "Ring",    kRing);
        addParam(mpc::floatParam("vibrato",      "Vibrato",     0.0f, 120.0f, d[kVibrato], "ct", 0));
        addParam(mpc::floatParam("vibrato_rate", "Vib Rate",    3.5f, 7.5f, d[kVibRate], "Hz", 1));
        addParam(mpc::choiceParam("body",        "Body",        {"Male", "Female", "Child", "Blend"}, 0));
        pct("body_female", "Female", kBodyFemale);
        pct("body_child",  "Child",  kBodyChild);
        addParam(mpc::floatParam("jaw",          "Jaw",         0.0f, 2.4f, d[kJaw], "cm", 2));
        addParam(mpc::floatParam("lip_open",     "Lips Open",   -0.3f, 2.2f, d[kLipsOpen], "cm", 2));
        addParam(mpc::floatParam("lip_prot",     "Protrude",    -0.1f, 1.4f, d[kProtrude], "cm", 2));
        pct("rounding", "Rounding", kRounding);
        addParam(mpc::floatParam("root",         "Tongue Root", -0.9f, 0.9f, d[kRoot], "cm", 2));
        pct("tip_curl", "Tip Curl", kTipCurl);
        addParam(mpc::floatParam("velum",        "Soft Palate", -0.25f, 1.2f, d[kSoftPalate], "cm2", 2));
        addParam(mpc::floatParam("larynx",       "Larynx",      -1.5f, 1.5f, d[kLarynx], "cm", 2));
        pct("twang",       "Twang",       kTwang);
        pct("false_folds", "False Folds", kFalseFolds);
        // the tongue: the calibrated vowels' range with a margin (template cm)
        addParam(mpc::floatParam("tb_x",         "Tongue Fwd",  1.0f, 4.0f, d[kTongueFwd], "cm", 2));
        addParam(mpc::floatParam("tb_y",         "Tongue Up",   5.0f, 8.2f, d[kTongueUp], "cm", 2));
        addParam(mpc::floatParam("tt_x",         "Tip Fwd",     5.0f, 7.0f, d[kTipFwd], "cm", 2));
        addParam(mpc::floatParam("tt_y",         "Tip Up",      5.2f, 9.8f, d[kTipUp], "cm", 2));
        pct("vowel_front", "Front", kVowelFront);
        pct("vowel_open",  "Open",  kVowelOpen);
        addParam(mpc::boolParam ("whisper",      "Whisper",     false));
        addParam(mpc::logParam  ("pitch",        "Pitch",       40.0f, 1100.0f, d[kPitch], "Hz", 0));
        addParam(mpc::choiceParam("mode",        "Mode",        {"Sing", "Play", "Speak", "Beatbox"}, 0));
        addParam(mpc::boolParam ("breathe",      "Breathe",     false));
        addParam(mpc::boolParam ("restart",      "Restart",     true));
        addParam(mpc::boolParam ("mpe",          "MPE",         false));
        addParam(mpc::choiceParam("lyrics_slot", "Lyrics",      {"1", "2", "3", "4", "5", "6", "7", "8"}, 0));
        addParam(mpc::intParam  ("start_syl",    "Start Syl",   0, 255, 0));
        addParam(mpc::choiceParam("speech_slot", "Speech",      {"1", "2", "3", "4", "5", "6", "7", "8"}, 0));
        addParam(mpc::intParam  ("start_line",   "Start Line",  0, 63, 0));
        addParam(mpc::floatParam("speech_rate",  "Speech Rate", 0.5f, 1.8f, 1.0f, "x", 2));
        addParam(mpc::boolParam ("ipa",          "IPA",         false));
        addParam(mpc::boolParam ("speak",        "Speak",       false));
        // the consonant chart (IPA in the engine; the MPC's font has plain letters)
        addParam(mpc::choiceParam("consonant",   "Consonant",   {"-", "p", "b", "t", "d", "k", "g", "glottal", "m", "n", "ng", "trill r", "tap", "f", "v", "th", "dh", "s", "z", "sh", "zh", "x", "h", "r", "y", "l"}, 0));
        addParam(mpc::choiceParam("pad_vowel",   "Pad Vowel",   {"ah", "ee", "oo", "uh"}, 0));
        addParam(mpc::choiceParam("beat",        "Beat",        {"Boots+Cats", "4 on Floor", "Breakbeat", "Rim Shuffle", "Inward Snr", "Clicks+Pops", "Hum Bass", "Your Beat"}, 0));
        addParam(mpc::floatParam("tempo",        "Tempo",       60.0f, 240.0f, d[kTempo], "bpm", 0));
        addParam(mpc::floatParam("swing",        "Swing",       0.0f, 60.0f, d[kSwing] * 100.0f, "%", 0));
        addParam(mpc::boolParam ("mic",          "Mic FX",      true));
        addParam(mpc::boolParam ("follow",       "Follow Host", true));
        addParam(mpc::boolParam ("play_beat",    "Play Beat",   false));
        std::vector<std::string> stepChoices = {"."};
        for (const std::string& s : kSounds) stepChoices.push_back(s);
        for (int i = 0; i < 16; ++i)
        {
            const std::string id = "step_" + std::string(i < 9 ? "0" : "") + std::to_string(i + 1);
            const std::string name = "Step " + std::to_string(i + 1);
            addParam(mpc::choiceParam(id.c_str(), name.c_str(), stepChoices, (int)d[kStep1 + i]));
        }
        std::vector<std::string> padChoices = {"-"};
        for (const std::string& s : kSounds) padChoices.push_back(s);
        addParam(mpc::choiceParam("pad",         "Pad",         padChoices, 0));

        mpc::Param face = mpc::intParam("vis_face", "Face", 0, 32, 0);   // unused since the layered face
        mpc::Param velum = mpc::intParam("vis_velum", "Velum Pos", 0, kVelumFrames - 1, 0);
        face.automatable = false;
        velum.automatable = false;
        addParam(face);
        addParam(velum);

        addParam(mpc::choiceParam("type_into", "Type Into", {"Speak", "Sing"}, 0));   // named as the modes
        addParam(mpc::choiceParam("type_slot", "Type Slot", {"1", "2", "3", "4", "5", "6", "7", "8"}, 0));
        std::vector<std::string> keys = {"-"};
        for (char c = 'A'; c <= 'Z'; ++c) keys.push_back(std::string(1, c));
        for (const char* k : {"Space", ".", ",", "?", "!", "'", "Del", "Line", "Clear"}) keys.push_back(k);
        // typing is taps on the screen. Automatable, like every control the screen sets: the MPC
        // did not take the keyboard's taps while Key said it could not be automated.
        mpc::Param key = mpc::choiceParam("key", "Key", keys, 0);
        addParam(key);
        mpc::Param typed = mpc::intParam("typed", "Typed", 0, 1023, 0);
        typed.automatable = false;
        addParam(typed);
        mpc::Param hit = mpc::choiceParam("hit", "Hit", {"-", "Kick", "Hat", "Snare", "Rim", "Inward", "Click", "Pop", "Open Hat", "Crash", "Hum"}, 0);
        hit.automatable = false;
        addParam(hit);
        // the face's layers, read-only; at rest until the first report
        int rest[kFaceLayers] = {};
        cm_face_frames(faceFrames_);
        cm_face_rest(rest);
        const char* layerIds[kFaceLayers] = {"vis_jaw", "vis_tongue", "vis_lips", "vis_lower_lip", "vis_larynx", "vis_sound"};
        const char* layerNames[kFaceLayers] = {"Jaw Pos", "Tongue Pos", "Lips Pos", "Lower Lip", "Larynx Pos", "Sound Lvl"};
        for (int i = 0; i < kFaceLayers; ++i)
        {
            if (faceFrames_[i] < 2) faceFrames_[i] = 2;
            mpc::Param v = mpc::intParam(layerIds[i], layerNames[i], 0, faceFrames_[i] - 1, std::min(std::max(rest[i], 0), faceFrames_[i] - 1));
            v.automatable = false;
            addParam(v);
        }
        mpc::Param stepLit = mpc::choiceParam("step_lit", "Step Lit", {"-", "1", "2", "3", "4", "5", "6", "7", "8", "9", "10", "11", "12", "13", "14", "15", "16"}, 0);
        stepLit.automatable = false;
        addParam(stepLit);
        // the typing cursor: one step per character, so the data wheel moves it a character a click
        std::vector<std::string> cursorSteps;
        for (int i = 0; i <= kCursorEnd; ++i) cursorSteps.push_back(i == kCursorEnd ? "End" : "-" + std::to_string(kCursorEnd - i));
        addParam(mpc::choiceParam("cursor", "Cursor", cursorSteps, kCursorEnd));
        mpc::Param typed2 = mpc::intParam("typed2", "Typed 2", 0, 1023, 0);
        typed2.automatable = false;
        addParam(typed2);
        // the pads' notes in Beatbox (engine pad_square) and the last note played, to see them
        std::vector<std::string> padNotes, lastNote = {"-"};
        for (int n = 0; n <= kPadBaseMax; ++n) padNotes.push_back(noteName(n));
        padNotes.push_back("Classic");
        padNotes.push_back("Learn");
        padNotes.push_back("Custom");
        padNotes.push_back("Learn All");
        addParam(mpc::choiceParam("pad_notes", "Pad 1 Note", padNotes, (int)d[kPadNotes]));
        for (int n = 0; n < 128; ++n) lastNote.push_back(noteName(n));
        mpc::Param last = mpc::choiceParam("last_note", "Last Note", lastNote, 0);
        last.automatable = false;
        addParam(last);
        // what comes next on the SING and SPEAK pages: the text is formatValue's (the helper
        // thread moves the values so the host reads it again)
        mpc::Param now = mpc::intParam("now", "Now", 0, 1023, 0);
        mpc::Param next = mpc::intParam("next", "Next", 0, 1023, 0);
        now.automatable = false;
        next.automatable = false;
        addParam(now);
        addParam(next);
        // each square's hum note and accent (the NOTES page)
        std::vector<std::string> stepNotes = {"-"};
        for (int n = kNoteLow; n <= kNoteHigh; ++n) stepNotes.push_back(noteName(n));
        for (int i = 0; i < 16; ++i)
        {
            const std::string id = "step_note_" + std::string(i < 9 ? "0" : "") + std::to_string(i + 1);
            const std::string name = "Note " + std::to_string(i + 1);
            addParam(mpc::choiceParam(id.c_str(), name.c_str(), stepNotes, 0));
        }
        for (int i = 0; i < 16; ++i)
        {
            const std::string id = "step_accent_" + std::string(i < 9 ? "0" : "") + std::to_string(i + 1);
            const std::string name = "Accent " + std::to_string(i + 1);
            addParam(mpc::boolParam(id.c_str(), name.c_str(), false));
        }
        // Pad Map 1-16: each pad's note when Pad 1 Note says Custom (Learn All writes them)
        for (int i = 0; i < 16; ++i)
        {
            const std::string id = "pad_map_" + std::string(i < 9 ? "0" : "") + std::to_string(i + 1);
            const std::string name = "Pad Map " + std::to_string(i + 1);
            mpc::Param m = mpc::intParam(id.c_str(), name.c_str(), 0, 127, 36 + i);
            m.automatable = false;
            addParam(m);
        }
        texts_ = cm_texts_new();

        // Built-in programs: voices only. Each sets parameters 0-13 (Gain, the voice source, the
        // body) and leaves the mode, words, mouth posture and beat alone (a program's missing
        // values are left as they are). Gains level-match the presets; Gain comes after the
        // engine's limiter (ceiling 0.75): a boost turns the voice up into it, it never clips.
        // engine/examples/presets.rs renders and measures them (keep its list the same).
        //          Gain  Breath Press Bright Glide Brthy Rough Fry  Ring  Vib  Rate Body Fem  Child
        addProgram("Init", {0.0f, d[kBreath], d[kPressed] * 100.0f, d[kBright] * 100.0f, d[kGlide] * 1000.0f,
                            d[kBreathy] * 100.0f, d[kRough] * 100.0f, d[kFry] * 100.0f, 0.0f, d[kVibrato], d[kVibRate],
                            0.0f, 50.0f, 0.0f});
        addProgram("Natural", {-1.0f, 8.0f, 45.0f, 30.0f, 40.0f, 12.0f, 5.0f, 0.0f, 0.0f, 15.0f, 5.5f, 0.0f, 50.0f, 0.0f});
        addProgram("Breathy", { 2.0f, 6.0f, 20.0f, 15.0f, 80.0f, 60.0f, 0.0f, 0.0f, 0.0f, 25.0f, 5.5f, 1.0f, 50.0f, 0.0f});
        addProgram("Belt",    {-3.0f, 14.0f, 80.0f, 65.0f, 50.0f, 3.0f, 15.0f, 0.0f, 30.0f, 25.0f, 5.8f, 0.0f, 50.0f, 0.0f});
        addProgram("Opera",   {-6.0f, 12.0f, 60.0f, 45.0f, 90.0f, 5.0f, 0.0f, 0.0f, 80.0f, 70.0f, 5.8f, 0.0f, 50.0f, 0.0f});
        addProgram("Airy",    { 3.0f, 5.0f, 15.0f, 10.0f, 120.0f, 75.0f, 0.0f, 0.0f, 0.0f, 20.0f, 5.0f, 1.0f, 50.0f, 0.0f});
        addProgram("Rough",   {-4.0f, 11.0f, 70.0f, 40.0f, 40.0f, 10.0f, 70.0f, 20.0f, 0.0f, 10.0f, 5.5f, 0.0f, 50.0f, 0.0f});
        addProgram("Child",   { 2.0f, 7.0f, 45.0f, 40.0f, 50.0f, 20.0f, 0.0f, 0.0f, 0.0f, 20.0f, 6.0f, 2.0f, 50.0f, 0.0f});

        // The helper thread puts the face on the screen (see notifierMain). Without it the plugin
        // still plays; the face then stands still.
        try { notifier_ = std::thread(&CHIMERAMPC::notifierMain, this); }
        catch (...) { mpc::log("CHIMERA MPC: could not start the helper thread; the face will not move"); }
    }

    ~CHIMERAMPC() override
    {
        {
            // under the lock, so the notifier cannot miss the wake-up between its look and its wait
            std::lock_guard<std::mutex> lock(quitMutex_);
            quit_.store(true);
        }
        quitCv_.notify_all();
        if (notifier_.joinable()) notifier_.join();
        cm_free(engine_.exchange(nullptr));
        cm_texts_free(texts_);
    }

    // A tap on a momentary control, as the host sets it (any thread): queued for process(). A rise
    // to a step, or a move to another step, is a tap; the same value sent again is not (automation
    // may resend it), and the return to 0 only re-arms it.
    void parameterChanged(int index, float plainValue) override
    {
        if (!isTap(index)) return;
        const int step = (int)std::lround(plainValue);
        const int was = tapLast_[tapSlot(index)].exchange(step, std::memory_order_acq_rel);
        if (step < 1 || step == was) return;
        const uint32_t w = tapWrite_.fetch_add(1, std::memory_order_acq_rel);
        // a full queue drops the tap rather than overwrite one not yet taken
        int32_t empty = 0;
        tapQueue_[w % kTapQueue].compare_exchange_strong(empty, (int32_t)((index << 8) | step), std::memory_order_release);
    }

    // The text box's two rows (the slot around its cursor), as Typed's and Typed 2's text, and the
    // screen's Now and Next (the host's thread).
    bool formatValue(int index, float plainValue, std::string& out) override
    {
        (void)plainValue;
        if (index == kNow || index == kNext)
        {
            // "NOW\nNEXT", each within the framework's 31 characters (the engine keeps them so)
            char buf[kTextBuf];
            readText(nowNextBuf_, nowNextSeq_, buf, sizeof buf);
            const char* nl = std::strchr(buf, '\n');
            if (index == kNow) out.assign(buf, nl ? (size_t)(nl - buf) : std::strlen(buf));
            else out = nl ? nl + 1 : "";
            return true;
        }
        if (index != kTyped && index != kTyped2) return false;
        // the preview is two rows with a line break between them: Typed shows the first, Typed 2
        // the second (each within the framework's 31 characters)
        char buf[64];
        readText(previewBuf_, previewSeq_, buf, sizeof buf);
        const char* nl = std::strchr(buf, '\n');
        if (index == kTyped) out.assign(buf, nl ? (size_t)(nl - buf) : std::strlen(buf));
        else out = nl ? nl + 1 : "";
        return true;
    }

    // The typed words go with the project (the host's thread).
    void saveExtraState(std::vector<uint8_t>& out) override
    {
        const int n = cm_texts_save(texts_, nullptr, 0);
        if (n <= 0) return;
        out.resize((size_t)n);
        cm_texts_save(texts_, out.data(), n);
    }

    void loadExtraState(const uint8_t* data, size_t size) override
    {
        cm_texts_load(texts_, data, (int)size);
    }

    // Allocates the engine (outside the audio callback). A new one only when the rate changes.
    void prepare(double sampleRate, int maxBlockSize) override
    {
        (void)maxBlockSize;
        if (engine_.load() != nullptr && sampleRate == engineRate_) return;
        const std::string content = mpc::contentFolder();
        ChimeraMpc* fresh = cm_new(sampleRate, content.empty() ? nullptr : content.c_str(), texts_);
        engineRate_ = sampleRate;
        ChimeraMpc* old = engine_.exchange(fresh);
        // free the old engine only once process() is not using it
        while (inProcess_.load())
            std::this_thread::sleep_for(std::chrono::microseconds(200));
        cm_free(old);
    }

    // On resume and after a state load: every note off (done by process(), on the audio thread).
    void reset() override
    {
        allOff_.store(true, std::memory_order_release);
        // a loaded state or program must not act as a tap on the momentary controls (process)
        settle_.store(true, std::memory_order_release);
        padSeen_ = false;
        beatSeen_ = false;
        slotsSeen_ = false;
        learnSeen_ = false;
        lastTyping_ = -1;   // a load's slot and cursor are taken as they are
        hitDark_.store(true, std::memory_order_release);   // the helper thread darkens the pads
        // a state load sets the momentary controls too (to whatever was saved): those are not taps.
        // reset() runs before the next block after a load (or on resume), so the queue is cleared
        // of them here
        tapRead_ = tapWrite_.load(std::memory_order_acquire);
        for (std::atomic<int32_t>& t : tapQueue_) t.store(0, std::memory_order_relaxed);
    }

    void process(mpc::ProcessContext& ctx) override
    {
        inProcess_.store(true);   // seq_cst: ordered against the engine load (prepare reads the two the other way round)
        ChimeraMpc* e = engine_.load();
        if (e == nullptr)
        {
            inProcess_.store(false, std::memory_order_release);
            return;   // outputs are already zeroed
        }
        if (allOff_.exchange(false, std::memory_order_acq_rel)) cm_all_notes_off(e);

        followVowelPad(e);
        followBeatPreset();
        followSlots();
        followLearn();
        // the squares' hum notes and accents (the engine applies them when they change)
        float stepNotes[16], stepAccents[16];
        for (int i = 0; i < 16; ++i)
        {
            stepNotes[i] = stepNoteMidi(getInt(kStepNote1 + i));
            stepAccents[i] = get(kStepAccent1 + i) >= 0.5f ? 1.0f : 0.0f;
        }
        cm_step_notes(e, stepNotes, stepAccents);
        int padMap[16];
        for (int i = 0; i < 16; ++i) padMap[i] = getInt(kPadMap1 + i);
        cm_pad_map(e, padMap);
        const mpc::Transport& t = ctx.transport;
        cm_transport(e, t.playing ? 1 : 0, t.tempoValid ? 1 : 0, t.bpm, t.ppqValid ? 1 : 0, t.ppqPos, ctx.numFrames);
        // with Follow Host on, Tempo is the MPC's (stopped too, when it gives one): Play Beat runs at
        // it and the screen shows it (the helper thread tells the host)
        if (getBool(kFollow) && t.tempoValid && t.bpm > 0.0)
        {
            const float bpm = (float)std::min(240.0, std::max(60.0, t.bpm));
            if (std::fabs(get(kTempo) - bpm) > 0.05f)
            {
                set(kTempo, bpm);
                tempoFollowed_.store(true, std::memory_order_release);
            }
        }

        // the parameters, in the engine's units (it applies only what changed)
        float v[kEngineParams];
        for (int i = 0; i < kEngineParams; ++i) v[i] = get(i);
        for (int i : {kPressed, kBright, kBreathy, kRough, kFry, kRing, kBodyFemale, kBodyChild, kRounding, kTipCurl, kTwang, kFalseFolds, kVowelFront, kVowelOpen, kSwing})
            v[i] *= 0.01f;
        v[kGlide] *= 0.001f;
        for (int i : {kKey, kPad, kConsonant, kSpeak}) v[i] = 0.0f;   // they come as taps (below)
        cm_set_params(e, v, kEngineParams);
        if (settle_.exchange(false, std::memory_order_acq_rel)) cm_settle(e);
        // the typing cursor: back to the end when another slot (or Sing / Speak) is picked
        const int typing = getInt(kTypeInto) * 16 + getInt(kTypeSlot);
        if (typing != lastTyping_) { if (lastTyping_ >= 0) set(kCursor, (float)kCursorEnd); lastTyping_ = typing; }
        cm_text_cursor(e, kCursorEnd - getInt(kCursor));
        // the taps caught since the last block, in order
        // (the reader follows the write count: a slot left empty, by a tap dropped from a full
        // queue, is passed over instead of holding up the ones after it)
        const uint32_t written = tapWrite_.load(std::memory_order_acquire);
        if (written - tapRead_ > (uint32_t)kTapQueue) tapRead_ = written - kTapQueue;
        while (tapRead_ != written)
        {
            const int32_t t = tapQueue_[tapRead_ % kTapQueue].exchange(0, std::memory_order_acquire);
            ++tapRead_;
            if (t != 0) cm_tap(e, t >> 8, t & 0xFF);
        }

        const int numFrames = ctx.numFrames;
        float* outL = ctx.outputs[0];
        float* outR = ctx.numOutputs > 1 ? ctx.outputs[1] : nullptr;
        int frame = 0, midiIndex = 0;
        while (frame < numFrames)
        {
            while (midiIndex < ctx.numMidiEvents && ctx.midi[midiIndex].sampleOffset <= frame)
                handleMidi(e, ctx.midi[midiIndex++]);
            int end = numFrames;
            if (midiIndex < ctx.numMidiEvents)
                end = mpc::clamp(ctx.midi[midiIndex].sampleOffset, frame + 1, numFrames);
            while (frame < end)
            {
                const int n = std::min(end - frame, kChunk);
                cm_render(e, buf_, n);
                for (int i = 0; i < n; ++i)
                {
                    outL[frame + i] = buf_[i];
                    if (outR) outR[frame + i] = buf_[i];
                }
                frame += n;
            }
        }
        char preview[64];
        const uint32_t serial = cm_text_preview(e, (uint8_t*)preview, sizeof preview);
        if (serial != previewSerialSeen_)
        {
            previewSerialSeen_ = serial;
            writeText(previewBuf_, previewSeq_, previewWrites_, preview);
        }
        char nowNext[kTextBuf];
        const uint32_t nowNextSerial = cm_now_next(e, (uint8_t*)nowNext, sizeof nowNext);
        if (nowNextSerial != nowNextSerialSeen_)
        {
            nowNextSerialSeen_ = nowNextSerial;
            writeText(nowNextBuf_, nowNextSeq_, nowNextWrites_, nowNext);
        }
        int shown[4] = {0, 0, 0, 0};
        cm_display(e, shown);
        velumFrame_.store(shown[1], std::memory_order_relaxed);
        int layers[kFaceLayers] = {};
        cm_face(e, layers);
        for (int i = 0; i < kFaceLayers; ++i) faceNow_[i].store(layers[i], std::memory_order_relaxed);
        hitPad_.store(shown[2], std::memory_order_relaxed);
        hitCount_.store(shown[3], std::memory_order_release);
        int step[2] = {0, 0};
        cm_step_lit(e, step);
        stepPad_.store(step[0], std::memory_order_relaxed);
        stepCount_.store(step[1], std::memory_order_release);
        // Learn All: the square to hit next, and when all 16 are in, the table goes into Pad Map and
        // Pad 1 Note says Custom (the helper thread tells the host)
        int learn[18] = {-1};
        cm_pad_learn(e, learn);
        learnNext_.store(learn[0], std::memory_order_relaxed);
        if (learn[0] == 16 && getInt(kPadNotes) == kPadLearnAll)
        {
            for (int i = 0; i < 16; ++i) set(kPadMap1 + i, (float)learn[2 + i]);
            set(kPadNotes, (float)kPadCustom);
            padLearned_.store(true, std::memory_order_release);
        }
        // Learn heard pad 1: its setting goes into Pad 1 Note (the helper thread tells the host)
        int notes[3] = {-1, 0, -1};
        cm_pad_notes(e, notes);
        lastNote_.store(notes[0], std::memory_order_relaxed);
        if (notes[2] >= 0 && getInt(kPadNotes) == kPadLearn)
        {
            set(kPadNotes, (float)notes[2]);
            padLearned_.store(true, std::memory_order_release);
        }
        processed_.fetch_add(1, std::memory_order_relaxed);
        inProcess_.store(false, std::memory_order_release);
    }

private:
    // A text (the WORDS preview, Now / Next) crosses from the audio thread to the host's: a
    // sequence lock (the writer never waits; a reader that catches a write in progress reads
    // again). `writes` counts the writes, for the helper thread to see a new text.
    static constexpr size_t kTextBuf = 64;
    using TextBuf = std::atomic<char>[kTextBuf];

    static void writeText(TextBuf& buf, std::atomic<uint32_t>& seq, std::atomic<uint32_t>& writes, const char* text)
    {
        seq.fetch_add(1, std::memory_order_acq_rel);   // odd: writing
        std::atomic_thread_fence(std::memory_order_release);
        for (size_t i = 0; i < kTextBuf; ++i)
        {
            buf[i].store(text[i], std::memory_order_relaxed);
            if (text[i] == 0) break;
        }
        std::atomic_thread_fence(std::memory_order_release);
        seq.fetch_add(1, std::memory_order_release);   // even: done
        writes.fetch_add(1, std::memory_order_relaxed);
    }

    static void readText(const TextBuf& buf, const std::atomic<uint32_t>& seq, char* out, size_t cap)
    {
        for (int tries = 0; tries < 100; ++tries)
        {
            const uint32_t a = seq.load(std::memory_order_acquire);
            if (a & 1u) { std::this_thread::yield(); continue; }
            size_t i = 0;
            for (; i + 1 < cap && i < kTextBuf; ++i)
            {
                out[i] = buf[i].load(std::memory_order_relaxed);
                if (out[i] == 0) break;
            }
            out[i < cap ? i : cap - 1] = 0;
            std::atomic_thread_fence(std::memory_order_acquire);
            if (seq.load(std::memory_order_relaxed) == a) return;
        }
        out[0] = 0;
    }

    // The helper thread: every 50 ms it moves the face's display parameters to what the voice is
    // doing and reports the ones that changed, so the screen's film strips follow. While a
    // setting is moving (a Q-Link, the screen, the data wheel) it writes them without reporting
    // for about 300 ms: the house plugins found the MPC's Q-Link screen can lock up when reports
    // come in during a knob sweep. It also stays quiet while the audio is not running (the host
    // suspended or is closing the plugin), and stops at once when the plugin closes.
    void notifierMain()
    {
        float seen[kNumParams];
        for (int i = 0; i < kNumParams; ++i) seen[i] = get(i);
        // idle until the first block: nothing goes to the host while it is still opening the plugin
        int lastTyped = -1, knobLooks = 0, idleLooks = 4;
        // the face's strips: the soft palate, then cm_face's layers
        constexpr int kStrips = 1 + kFaceLayers;
        int lastShown[kStrips], again[kStrips] = {};
        for (int& l : lastShown) l = -1;
        int lastHitCount = hitCount_.load(std::memory_order_acquire), shownHit = -1, hitLooks = 0;
        int lastStepCount = stepCount_.load(std::memory_order_acquire), shownStep = -1, stepLooks = 0;
        int shownNote = -1;
        int lastNowNext = -1;
        uint32_t lastProcessed = 0;
        for (;;)
        {
            {
                std::unique_lock<std::mutex> lock(quitMutex_);
                if (quitCv_.wait_for(lock, std::chrono::milliseconds(kLookMs), [this] { return quit_.load(); })) break;
            }
            const uint32_t n = processed_.load(std::memory_order_relaxed);
            idleLooks = (n == lastProcessed) ? idleLooks + 1 : 0;
            lastProcessed = n;
            if (idleLooks > 3)
            {
                // no audio for 200 ms: say nothing to the host; a lit pad goes dark (written only,
                // shown when the screen next reads it)
                if (shownHit > 0) { shownHit = 0; set(kHit, 0.0f); }
                if (shownStep > 0) { shownStep = 0; set(kStepLit, 0.0f); }
                continue;
            }

            bool moved = false;
            for (int i = 0; i < kNumParams; ++i)
            {
                // displays and the keyboard's taps are not knob sweeps
                if (isDisplay(i) || isTap(i)) continue;
                const float x = get(i);
                if (x != seen[i]) { moved = true; seen[i] = x; }
            }
            if (moved) knobLooks = kKnobLooks;
            else if (knobLooks > 0) --knobLooks;

            // The face's strips: held back while a setting moves, and shown once it rests ("last" is
            // what the screen has, so a frame that changed during a knob sweep is still sent: in Play
            // the mouth moves only with the knobs). A new frame is reported again every half second
            // for a few seconds: in Play it is often the only report for a long time, and one the
            // screen misses (the Q-Link window over the face, say) would leave that layer stuck. At
            // most kMaxFaceReports reports a look, new frames first; one left over waits for the next.
            if (knobLooks == 0)
            {
                int budget = kMaxFaceReports;
                auto frameOf = [&](int s, int& index, int& frames)
                {
                    if (s == 0) { index = kVisVelum; frames = kVelumFrames; return velumFrame_.load(std::memory_order_relaxed); }
                    index = kVisJaw + s - 1;
                    frames = faceFrames_[s - 1];
                    return faceNow_[s - 1].load(std::memory_order_relaxed);
                };
                for (int k = 0; k < kStrips && budget > 0; ++k)
                {
                    const int s = (stripTurn_ + k) % kStrips;
                    int index = 0, frames = 2;
                    const int frame = std::min(std::max(frameOf(s, index, frames), 0), frames - 1);
                    if (frame == lastShown[s]) continue;
                    --budget;
                    lastShown[s] = frame;
                    again[s] = kResendLooks;
                    setNormalised(index, (float)frame / (float)(frames - 1));
                    notifyHostParameterChanged(index);
                }
                for (int s = 0; s < kStrips; ++s)
                {
                    if (again[s] <= 0 || --again[s] % kResendEvery != 0 || budget <= 0) continue;
                    --budget;
                    notifyHostParameterChanged(s == 0 ? kVisVelum : kVisJaw + s - 1);
                }
                stripTurn_ = (stripTurn_ + 1) % kStrips;
            }
            // the typed text: its value moves so the host reads the new text (a report alone does
            // not refresh a text whose value stayed the same); reported even while knobs move, as
            // typing is taps
            // Beatbox's and Sing's latency: while the sequence runs its hits land a fixed time after their point
            // on the timeline, and the MPC is told so it can line the voice up with the other
            // tracks; the other modes play at once (0). Told only while no setting is moving.
            // (Sing too: while the sequence runs its vowels land the same time after their notes,
            // the consonants ahead of them)
            const int mode = getInt(kMode);
            const int wantLatency = (mode == 3 || mode == 0) ? (int)std::lround(cm_beat_latency() * sampleRate()) : 0;
            if (wantLatency != latencyTold_ && knobLooks == 0)
            {
                latencyTold_ = wantLatency;
                setLatency(wantLatency);
            }
            const int typed = (int)(previewWrites_.load(std::memory_order_relaxed) % 1024u);
            if (typed != lastTyped)
            {
                lastTyped = typed;
                setNormalised(kTyped, (float)typed / 1023.0f);
                notifyHostParameterChanged(kTyped);
                setNormalised(kTyped2, (float)typed / 1023.0f);
                notifyHostParameterChanged(kTyped2);
            }
            // Now and Next (what comes next on SING and SPEAK): their values move so the host reads
            // the new texts; held back while a setting moves (notes would otherwise report several
            // times a second during a knob sweep, which can lock up the MPC's Q-Link screen), and
            // shown once it rests
            const int nowNext = (int)(nowNextWrites_.load(std::memory_order_relaxed) % 1024u);
            if (nowNext != lastNowNext && knobLooks == 0)
            {
                lastNowNext = nowNext;
                setNormalised(kNow, (float)nowNext / 1023.0f);
                notifyHostParameterChanged(kNow);
                setNormalised(kNext, (float)nowNext / 1023.0f);
                notifyHostParameterChanged(kNext);
            }
            // Learn's result, at once (it is not a knob sweep)
            if (padLearned_.exchange(false, std::memory_order_acq_rel)) notifyHostParameterChanged(kPadNotes);
            // a Learn button switched the mode to Beatbox: the mode chips show it at once
            if (modeSwitched_.exchange(false, std::memory_order_acq_rel)) notifyHostParameterChanged(kMode);
            // the MPC's tempo came into Tempo (Follow Host): shown once settings rest
            if (knobLooks == 0 && tempoFollowed_.exchange(false, std::memory_order_acq_rel)) notifyHostParameterChanged(kTempo);
            // the last note played, in Beatbox (BEATBOX shows it): reported when it changes (the
            // same pad again changes nothing), held while a setting moves, as the pads are
            const int note = lastNote_.load(std::memory_order_relaxed);
            if (mode == 3 && note >= 0 && note != shownNote && knobLooks == 0)
            {
                shownNote = note;
                set(kLastNote, (float)(note + 1));
                notifyHostParameterChanged(kLastNote);
            }
            // a drum hit lights its pad for a moment
            const int hits = hitCount_.load(std::memory_order_acquire);
            int lit = shownHit < 0 ? 0 : shownHit;
            // after a load or reset: the pads and squares go dark, whatever the project saved
            const bool dark = hitDark_.exchange(false, std::memory_order_acq_rel);
            if (dark) shownNote = -1;   // a load may have brought another Last Note: the next note is shown again
            // the BEATBOX square of a pad hit or of the beat's step, lit for a moment as the pads are
            const int steps = stepCount_.load(std::memory_order_acquire);
            int litStep = shownStep < 0 ? 0 : shownStep;
            if (dark)
            {
                lastStepCount = steps;
                stepLooks = 0;
                litStep = 0;
                shownStep = -1;
            }
            else if (steps != lastStepCount)
            {
                lastStepCount = steps;
                litStep = stepPad_.load(std::memory_order_relaxed);
                stepLooks = kHitLooks;
            }
            else if (stepLooks > 0 && --stepLooks == 0)
                litStep = 0;
            // Learn All: the square whose pad to hit next stays lit
            const int learnNext = learnNext_.load(std::memory_order_relaxed);
            if (learnNext >= 0 && learnNext < 16) litStep = learnNext + 1;
            // (held while a setting moves: the beat's steps would report several times a second
            // during a knob sweep on BEATBOX, which can lock up the MPC's Q-Link screen)
            if (litStep != shownStep && knobLooks == 0)
            {
                shownStep = litStep;
                set(kStepLit, (float)litStep);
                notifyHostParameterChanged(kStepLit);
            }
            if (dark)
            {
                lastHitCount = hits;
                hitLooks = 0;
                lit = 0;
                shownHit = -1;
            }
            else if (hits != lastHitCount)
            {
                lastHitCount = hits;
                lit = hitPad_.load(std::memory_order_relaxed);
                hitLooks = kHitLooks;
            }
            else if (hitLooks > 0 && --hitLooks == 0)
                lit = 0;
            // (held while a setting moves: the beat's hits come several times a second, which during a
            // knob sweep can lock up the MPC's Q-Link screen)
            if (lit != shownHit && knobLooks == 0)
            {
                shownHit = lit;
                set(kHit, (float)lit);
                notifyHostParameterChanged(kHit);
            }
        }
    }

    static mpc::PluginInfo makeInfo()
    {
        mpc::PluginInfo info;
        info.name         = "CHIMERA MPC";
        info.vendor       = "anthonytheinventor";   // must match plugin.json "manufacturer"
        info.uniqueId     = MPC_FOURCC(kUid[0], kUid[1], kUid[2], kUid[3]);   // must match plugin.json "uid"
        info.version      = 1000;
        info.isInstrument = true;
        info.numInputs    = 0;
        info.numOutputs   = 2;
        info.tailSamples  = 0;
        return info;
    }

    static constexpr char kUid[5] = "Am6u";

    // The vowel pad moved: its posture goes into the articulator parameters (the parameters stay
    // the one record of the posture, so a saved project sounds the same). Written as values
    // only: the screen shows them when it next reads them.
    void followVowelPad(ChimeraMpc* e)
    {
        const float front = get(kVowelFront), open = get(kVowelOpen);
        if (!padSeen_)
        {
            // first block after a load: the articulators already hold the posture
            padSeen_ = true;
            padFront_ = front; padOpen_ = open;
            rememberArtic();
            return;
        }
        bool articMoved = false;
        for (int k = 0; k < 9; ++k)
            if (get(kPadParams[k]) != padArtic_[k]) articMoved = true;
        if (front == padFront_ && open == padOpen_)
        {
            if (articMoved) rememberArtic();
            return;
        }
        padFront_ = front; padOpen_ = open;
        if (articMoved)
        {
            // the articulators changed in the same block: a project or program loading
            // (parameter by parameter, while audio runs), not a drag on the pad
            rememberArtic();
            return;
        }
        float a[9];
        cm_vowel_posture(e, front * 0.01f, open * 0.01f, a);
        a[5] = 1.0f - a[5];   // lip width -> Rounding
        for (int k = 0; k < 9; ++k)
        {
            float x = a[k];
            if (kPadParams[k] == kRounding || kPadParams[k] == kTipCurl) x *= 100.0f;   // percent parameters
            set(kPadParams[k], x);
        }
        rememberArtic();
    }

    void rememberArtic()
    {
        for (int k = 0; k < 9; ++k) padArtic_[k] = get(kPadParams[k]);
    }

    // A beat preset was picked: its steps, hum notes, accents, tempo and swing go into their
    // parameters (as the vowel pad's posture does). Picking Your Beat, or editing a step, changes
    // nothing else.
    void followBeatPreset()
    {
        const int beat = getInt(kBeat);
        if (!beatSeen_ || beat == lastBeat_) { beatSeen_ = true; lastBeat_ = beat; return; }
        lastBeat_ = beat;
        float steps[16], bpm = 0.0f, swing = 0.0f;
        if (!cm_beat_preset(beat, steps, &bpm, &swing)) return;
        for (int i = 0; i < 16; ++i) set(kStep1 + i, steps[i]);
        set(kTempo, bpm);
        set(kSwing, swing * 100.0f);
        // its hum notes and accents too (a preset without them puts them back to "-" and off)
        float notes[16] = {}, accents[16] = {};
        cm_beat_preset_notes(beat, notes, accents);
        for (int i = 0; i < 16; ++i)
        {
            set(kStepNote1 + i, (float)stepNoteChoice(notes[i]));
            set(kStepAccent1 + i, accents[i] >= 0.5f ? 1.0f : 0.0f);
        }
    }

    // The WORDS page's Type Slot and the slot that plays (Speech, or Lyrics while typing lyrics)
    // are kept the same, so the text on the screen is the text the voice says: turning either one
    // moves the other, and switching Type Into shows the slot that target plays. After a load
    // (reset) the values are taken as they are, so a project saves back exactly as it loaded.
    // Tapping LEARN PAD 1 or LEARN ALL 16 (Pad 1 Note moving to Learn or Learn All) switches the
    // mode to Beatbox: the pads only play the squares, and learn, there. A load is taken as it is.
    void followLearn()
    {
        const int pads = getInt(kPadNotes);
        const bool learning = pads == kPadLearn || pads == kPadLearnAll;
        if (learnSeen_ && learning && pads != lastPadNotes_ && getInt(kMode) != 3)
        {
            set(kMode, 3.0f);
            modeSwitched_.store(true, std::memory_order_release);   // the helper thread tells the host
        }
        learnSeen_ = true;
        lastPadNotes_ = pads;
    }

    void followSlots()
    {
        const int into = getInt(kTypeInto);
        const int target = into == 1 ? kLyrics : kSpeech;
        const int slot = getInt(kTypeSlot), playing = getInt(target);
        if (!slotsSeen_) {}
        else if (into != lastInto_) set(kTypeSlot, (float)playing);
        else if (slot != lastTypeSlot_) set(target, (float)slot);
        else if (playing != lastPlaying_) set(kTypeSlot, (float)playing);
        slotsSeen_ = true;
        lastInto_ = into;
        lastTypeSlot_ = getInt(kTypeSlot);
        lastPlaying_ = getInt(target);
    }

    // Every channel message goes to the engine as it came: notes, pitch bend, channel and key
    // pressure, CC 74 (MPE slide) and all-notes-off (with MPE, channels 2-16 are note channels).
    void handleMidi(ChimeraMpc* e, const mpc::MidiEvent& m)
    {
        if (m.size >= 2 && (m.data[0] & 0xF0) >= 0x80 && (m.data[0] & 0xF0) <= 0xE0)
            cm_midi(e, m.data[0], m.data[1], m.size >= 3 ? m.data[2] : 0);
    }

    std::atomic<ChimeraMpc*> engine_{nullptr};
    std::atomic<bool> inProcess_{false};
    std::atomic<bool> allOff_{false};
    std::atomic<bool> settle_{false};
    std::atomic<bool> hitDark_{false};
    int latencyTold_ = 0;   // the helper thread's
    int lastTyping_ = -1;   // the audio thread's: Type Into and Type Slot as last seen
    static constexpr int kTapQueue = 64;
    static int tapSlot(int index) { return index == kKey ? 0 : index == kPad ? 1 : index == kConsonant ? 2 : 3; }
    std::atomic<int32_t> tapQueue_[kTapQueue] = {};
    std::atomic<uint32_t> tapWrite_{0};
    uint32_t tapRead_ = 0;   // the audio thread's
    std::atomic<int> tapLast_[4] = {};
    std::atomic<int> velumFrame_{0}, hitPad_{0}, hitCount_{0}, stepPad_{0}, stepCount_{0}, lastNote_{-1};
    std::atomic<bool> padLearned_{false};
    std::atomic<int> learnNext_{-1};
    std::atomic<bool> modeSwitched_{false};
    std::atomic<bool> tempoFollowed_{false};
    std::atomic<int> faceNow_[kFaceLayers] = {};
    int faceFrames_[kFaceLayers] = {10, 28, 18, 12, 7, 6};   // cm_face_frames
    int stripTurn_ = 0;                                 // the helper thread's
    const void* texts_ = nullptr;
    TextBuf previewBuf_ = {};
    std::atomic<uint32_t> previewSeq_{0}, previewWrites_{0};
    uint32_t previewSerialSeen_ = 0;
    TextBuf nowNextBuf_ = {};
    std::atomic<uint32_t> nowNextSeq_{0}, nowNextWrites_{0};
    uint32_t nowNextSerialSeen_ = 0;   // the audio thread's
    std::atomic<uint32_t> processed_{0};
    std::thread notifier_;
    std::atomic<bool> quit_{false};
    std::mutex quitMutex_;
    std::condition_variable quitCv_;
    double engineRate_ = 0.0;
    bool padSeen_ = false;
    bool beatSeen_ = false;
    bool slotsSeen_ = false;
    bool learnSeen_ = false;
    int lastPadNotes_ = 0;
    int lastInto_ = 0, lastTypeSlot_ = 0, lastPlaying_ = 0;
    int lastBeat_ = 0;
    float padFront_ = 0.0f, padOpen_ = 0.0f;
    float padArtic_[9] = {};
    float buf_[kChunk] = {};
};

MPC_PLUGIN_ENTRY(CHIMERAMPC)
