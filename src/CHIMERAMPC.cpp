// CHIMERA MPC - CHIMERA's physical-model voice on the MPC, by anthonytheinventor.
//
// CHIMERA (github.com/jamesondh/chimera-vocal-synth) is a voice synthesizer by Jameson Hodge,
// MIT licensed: every sound comes from a simulated airway (lungs, vocal folds, throat, mouth and
// nose), not from samples. Its Rust engine is built into this plugin as a static library
// (engine/, C interface in chimera_mpc.h) with the settings that fit a 32-bit MPC.
//
// Signal flow: MIDI notes -> the engine -> mono voice -> Gain (smoothed) -> both outputs.
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
//         drum sounds (General MIDI's drum map), notes from C3 up hum two octaves down.
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
#include <cmath>
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
    kNumParams
};

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
    }

    ~CHIMERAMPC() override
    {
        cm_free(engine_.exchange(nullptr));
    }

    // Allocates the engine (outside the audio callback). A new one only when the rate changes.
    void prepare(double sampleRate, int maxBlockSize) override
    {
        (void)maxBlockSize;
        if (engine_.load() != nullptr && sampleRate == engineRate_) return;
        const std::string content = mpc::contentFolder();
        ChimeraMpc* fresh = cm_new(sampleRate, content.empty() ? nullptr : content.c_str());
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
        smoothedGain_ = dbToGain(get(kGain));
        padSeen_ = false;
        beatSeen_ = false;
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
        const mpc::Transport& t = ctx.transport;
        cm_transport(e, t.playing ? 1 : 0, t.tempoValid ? 1 : 0, t.bpm, t.ppqValid ? 1 : 0, t.ppqPos, ctx.numFrames);

        // the parameters, in the engine's units (it applies only what changed)
        float v[kNumParams];
        for (int i = 0; i < kNumParams; ++i) v[i] = get(i);
        for (int i : {kPressed, kBright, kBreathy, kRough, kFry, kRing, kBodyFemale, kBodyChild, kRounding, kTipCurl, kTwang, kFalseFolds, kVowelFront, kVowelOpen, kSwing})
            v[i] *= 0.01f;
        v[kGlide] *= 0.001f;
        cm_set_params(e, v, kNumParams);
        const float targetGain = dbToGain(get(kGain));

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
                    smoothedGain_ += 0.002f * (targetGain - smoothedGain_);
                    const float y = buf_[i] * smoothedGain_;
                    outL[frame + i] = y;
                    if (outR) outR[frame + i] = y;
                }
                frame += n;
            }
        }
        inProcess_.store(false, std::memory_order_release);
    }

private:
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

    static float dbToGain(float dB) { return std::pow(10.0f, dB / 20.0f); }

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
            return;
        }
        if (front == padFront_ && open == padOpen_) return;
        padFront_ = front; padOpen_ = open;
        float a[9];
        cm_vowel_posture(e, front * 0.01f, open * 0.01f, a);
        a[5] = 1.0f - a[5];   // lip width -> Rounding
        for (int k = 0; k < 9; ++k)
        {
            float x = a[k];
            if (kPadParams[k] == kRounding || kPadParams[k] == kTipCurl) x *= 100.0f;   // percent parameters
            set(kPadParams[k], x);
        }
    }

    // A beat preset was picked: its steps, tempo and swing go into their parameters (as the vowel
    // pad's posture does). Picking Your Beat, or editing a step, changes nothing else.
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
    double engineRate_ = 0.0;
    float smoothedGain_ = 1.0f;
    bool padSeen_ = false;
    bool beatSeen_ = false;
    int lastBeat_ = 0;
    float padFront_ = 0.0f, padOpen_ = 0.0f;
    float buf_[kChunk] = {};
};

MPC_PLUGIN_ENTRY(CHIMERAMPC)
