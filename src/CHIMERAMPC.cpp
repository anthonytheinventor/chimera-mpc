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
#include <condition_variable>
#include <mutex>
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
    kNumSettings,
    // read-only displays for the skin (not automatable; the helper thread writes them)
    kVisFace = kNumSettings,   // the face strip's frame (the nearest mouth posture)
    kVisVelum,                 // the soft palate strip's frame
    // typing words on the MPC
    kTypeInto,                 // Speech / Lyrics: which texts the keys edit
    kTypeSlot,                 // 1..8: which slot
    kKey,                      // one key (on a change to it): the letters, punctuation, Del, Line, Clear
    kTyped,                    // read-only: its text is the end of the slot being typed (formatValue)
    kHit,                      // read-only: the pad to light on the screen (0 none, 1..10 as Pad)
    kNumParams
};

constexpr int kEngineParams = kKey + 1;   // the engine reads 0..kKey (engine/src/lib.rs, module p)

bool isDisplay(int i) { return i == kVisFace || i == kVisVelum || i == kTyped || i == kHit; }

constexpr int kFaceFrames = 33;    // engine/src/lib.rs face_postures()
constexpr int kVelumFrames = 8;    // engine/src/lib.rs VELUM_FRAMES
constexpr int kLookMs = 50;        // the helper thread looks this often
constexpr int kKnobLooks = 6;      // and stays quiet this many looks after a setting moves
constexpr int kHitLooks = 3;       // a hit's pad stays lit this many looks (about 150 ms)

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

        mpc::Param face = mpc::intParam("vis_face", "Face", 0, kFaceFrames - 1, 0);
        mpc::Param velum = mpc::intParam("vis_velum", "Velum Pos", 0, kVelumFrames - 1, 0);
        face.automatable = false;
        velum.automatable = false;
        addParam(face);
        addParam(velum);

        addParam(mpc::choiceParam("type_into", "Type Into", {"Speech", "Lyrics"}, 0));
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
        texts_ = cm_texts_new();

        // Built-in programs: voices only. Each sets parameters 0-13 (Gain, the voice source, the
        // body) and leaves the mode, words, mouth posture and beat alone (a program's missing
        // values are left as they are). Gains level-match the presets; Gain comes after the
        // engine's limiter (ceiling 0.75), so a boost stays small enough to keep peaks under 1.
        // engine/examples/presets.rs renders and measures them (keep its list the same).
        //          Gain  Breath Press Bright Glide Brthy Rough Fry  Ring  Vib  Rate Body Fem  Child
        addProgram("Init", {0.0f, d[kBreath], d[kPressed] * 100.0f, d[kBright] * 100.0f, d[kGlide] * 1000.0f,
                            d[kBreathy] * 100.0f, d[kRough] * 100.0f, d[kFry] * 100.0f, 0.0f, d[kVibrato], d[kVibRate],
                            0.0f, 50.0f, 0.0f});
        addProgram("Natural", {-1.0f, 8.0f, 45.0f, 30.0f, 40.0f, 12.0f, 5.0f, 0.0f, 0.0f, 15.0f, 5.5f, 0.0f, 50.0f, 0.0f});
        addProgram("Breathy", { 2.0f, 6.0f, 20.0f, 15.0f, 80.0f, 60.0f, 0.0f, 0.0f, 0.0f, 25.0f, 5.5f, 1.0f, 50.0f, 0.0f});
        addProgram("Belt",    { 0.0f, 14.0f, 80.0f, 65.0f, 50.0f, 3.0f, 15.0f, 0.0f, 30.0f, 25.0f, 5.8f, 0.0f, 50.0f, 0.0f});
        addProgram("Opera",   {-2.0f, 12.0f, 60.0f, 45.0f, 90.0f, 5.0f, 0.0f, 0.0f, 80.0f, 70.0f, 5.8f, 0.0f, 50.0f, 0.0f});
        addProgram("Airy",    { 3.0f, 5.0f, 15.0f, 10.0f, 120.0f, 75.0f, 0.0f, 0.0f, 0.0f, 20.0f, 5.0f, 1.0f, 50.0f, 0.0f});
        addProgram("Rough",   { 0.0f, 11.0f, 70.0f, 40.0f, 40.0f, 10.0f, 70.0f, 20.0f, 0.0f, 10.0f, 5.5f, 0.0f, 50.0f, 0.0f});
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

    // The typed slot's end, as the Typed parameter's text (the host's thread).
    bool formatValue(int index, float plainValue, std::string& out) override
    {
        (void)plainValue;
        if (index != kTyped) return false;
        char buf[64];
        readPreview(buf, sizeof buf);
        out = buf;
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
        smoothedGain_ = dbToGain(get(kGain));
        padSeen_ = false;
        beatSeen_ = false;
        slotsSeen_ = false;
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
        const mpc::Transport& t = ctx.transport;
        cm_transport(e, t.playing ? 1 : 0, t.tempoValid ? 1 : 0, t.bpm, t.ppqValid ? 1 : 0, t.ppqPos, ctx.numFrames);

        // the parameters, in the engine's units (it applies only what changed)
        float v[kEngineParams];
        for (int i = 0; i < kEngineParams; ++i) v[i] = get(i);
        for (int i : {kPressed, kBright, kBreathy, kRough, kFry, kRing, kBodyFemale, kBodyChild, kRounding, kTipCurl, kTwang, kFalseFolds, kVowelFront, kVowelOpen, kSwing})
            v[i] *= 0.01f;
        v[kGlide] *= 0.001f;
        cm_set_params(e, v, kEngineParams);
        if (settle_.exchange(false, std::memory_order_acq_rel)) cm_settle(e);
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
        char preview[64];
        const uint32_t serial = cm_text_preview(e, (uint8_t*)preview, sizeof preview);
        if (serial != previewSerialSeen_)
        {
            previewSerialSeen_ = serial;
            writePreview(preview);
        }
        int shown[4] = {0, 0, 0, 0};
        cm_display(e, shown);
        faceFrame_.store(shown[0], std::memory_order_relaxed);
        velumFrame_.store(shown[1], std::memory_order_relaxed);
        hitPad_.store(shown[2], std::memory_order_relaxed);
        hitCount_.store(shown[3], std::memory_order_release);
        processed_.fetch_add(1, std::memory_order_relaxed);
        inProcess_.store(false, std::memory_order_release);
    }

private:
    // The preview text crosses from the audio thread to the host's: a sequence lock (the writer
    // never waits; a reader that catches a write in progress reads again).
    void writePreview(const char* text)
    {
        previewSeq_.fetch_add(1, std::memory_order_acq_rel);   // odd: writing
        std::atomic_thread_fence(std::memory_order_release);
        for (size_t i = 0; i < sizeof previewBuf_; ++i)
        {
            previewBuf_[i].store(text[i], std::memory_order_relaxed);
            if (text[i] == 0) break;
        }
        std::atomic_thread_fence(std::memory_order_release);
        previewSeq_.fetch_add(1, std::memory_order_release);   // even: done
        previewWrites_.fetch_add(1, std::memory_order_relaxed);
    }

    void readPreview(char* out, size_t cap)
    {
        for (int tries = 0; tries < 100; ++tries)
        {
            const uint32_t a = previewSeq_.load(std::memory_order_acquire);
            if (a & 1u) { std::this_thread::yield(); continue; }
            size_t i = 0;
            for (; i + 1 < cap && i < sizeof previewBuf_; ++i)
            {
                out[i] = previewBuf_[i].load(std::memory_order_relaxed);
                if (out[i] == 0) break;
            }
            out[i < cap ? i : cap - 1] = 0;
            std::atomic_thread_fence(std::memory_order_acquire);
            if (previewSeq_.load(std::memory_order_relaxed) == a) return;
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
        int lastFace = -1, lastVelum = -1, lastTyped = -1, knobLooks = 0, idleLooks = 0;
        int lastHitCount = hitCount_.load(std::memory_order_acquire), shownHit = -1, hitLooks = 0;
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
            if (idleLooks > 3) continue;   // no audio for 200 ms: say nothing to the host

            bool moved = false;
            for (int i = 0; i < kNumParams; ++i)
            {
                // displays and the keyboard's taps are not knob sweeps
                if (isDisplay(i) || i == kKey) continue;
                const float x = get(i);
                if (x != seen[i]) { moved = true; seen[i] = x; }
            }
            if (moved) knobLooks = kKnobLooks;
            else if (knobLooks > 0) --knobLooks;

            auto show = [&](int index, int frame, int frames, int& last)
            {
                // held back while a setting moves, and shown once it rests: "last" is what the
                // screen has, so a frame that changed during a knob sweep is still sent (in Play
                // the mouth moves only with the knobs, and it never moved on the screen)
                if (frame == last || knobLooks > 0) return;
                last = frame;
                setNormalised(index, (float)frame / (float)(frames - 1));
                notifyHostParameterChanged(index);
            };
            show(kVisFace, faceFrame_.load(std::memory_order_relaxed), kFaceFrames, lastFace);
            show(kVisVelum, velumFrame_.load(std::memory_order_relaxed), kVelumFrames, lastVelum);
            // the typed text: its value moves so the host reads the new text (a report alone does
            // not refresh a text whose value stayed the same); reported even while knobs move, as
            // typing is taps
            const int typed = (int)(previewWrites_.load(std::memory_order_relaxed) % 1024u);
            if (typed != lastTyped)
            {
                lastTyped = typed;
                setNormalised(kTyped, (float)typed / 1023.0f);
                notifyHostParameterChanged(kTyped);
            }
            // a drum hit lights its pad for a moment (reported even while knobs move: hits are taps)
            const int hits = hitCount_.load(std::memory_order_acquire);
            int lit = shownHit < 0 ? 0 : shownHit;
            if (hits != lastHitCount)
            {
                lastHitCount = hits;
                lit = hitPad_.load(std::memory_order_relaxed);
                hitLooks = kHitLooks;
            }
            else if (hitLooks > 0 && --hitLooks == 0)
                lit = 0;
            if (lit != shownHit)
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

    // The WORDS page's Type Slot and the slot that plays (Speech, or Lyrics while typing lyrics)
    // are kept the same, so the text on the screen is the text the voice says: turning either one
    // moves the other, and switching Type Into shows the slot that target plays. After a load
    // (reset) the values are taken as they are, so a project saves back exactly as it loaded.
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
    std::atomic<int> faceFrame_{0}, velumFrame_{0}, hitPad_{0}, hitCount_{0};
    const void* texts_ = nullptr;
    std::atomic<char> previewBuf_[64] = {};
    std::atomic<uint32_t> previewSeq_{0}, previewWrites_{0};
    uint32_t previewSerialSeen_ = 0;
    std::atomic<uint32_t> processed_{0};
    std::thread notifier_;
    std::atomic<bool> quit_{false};
    std::mutex quitMutex_;
    std::condition_variable quitCv_;
    double engineRate_ = 0.0;
    float smoothedGain_ = 1.0f;
    bool padSeen_ = false;
    bool beatSeen_ = false;
    bool slotsSeen_ = false;
    int lastInto_ = 0, lastTypeSlot_ = 0, lastPlaying_ = 0;
    int lastBeat_ = 0;
    float padFront_ = 0.0f, padOpen_ = 0.0f;
    float buf_[kChunk] = {};
};

MPC_PLUGIN_ENTRY(CHIMERAMPC)
