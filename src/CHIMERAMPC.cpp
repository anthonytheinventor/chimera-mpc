// CHIMERA MPC - CHIMERA's physical-model voice on the MPC, by anthonytheinventor.
//
// CHIMERA (github.com/jamesondh/chimera-vocal-synth) is a voice synthesizer by Jameson Hodge,
// MIT licensed: every sound comes from a simulated airway (lungs, vocal folds, throat, mouth and
// nose), not from samples. Its Rust engine is built into this plugin as a static library
// (engine/, C interface in chimera_mpc.h) with the settings that fit a 32-bit MPC.
//
// Signal flow: MIDI notes -> the engine (Play mode: the last key held sets the pitch, legato,
// and the breath runs while any key is held) -> mono voice -> Gain (smoothed) -> both outputs.
//
// This first version is Play mode only: notes sing one held posture, shaped by the voice
// parameters (breath, tension, brightness, ...) and the articulators (jaw, lips, tongue root,
// soft palate, larynx, ...). Sing, Speak and Beatbox, the tongue handles and the vowel pad come
// later, appended to this parameter list.
//
// Threads: the engine is created in prepare() (it allocates) and swapped in atomically; an old
// one is freed only once process() has let go of it. reset() only raises a flag that process()
// acts on, so the engine is only ever touched by one thread at a time.

#include "mpc_plugin.h"
#include "chimera_mpc.h"

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <thread>

namespace {

// Parameter order: the engine (engine/src/lib.rs, module p) reads the same list.
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
    kNumParams
};

constexpr int kChunk = 256;   // frames rendered per engine call

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
        ChimeraMpc* fresh = cm_new(sampleRate);
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

        // the parameters, in the engine's units (it applies only what changed)
        float v[kNumParams];
        for (int i = 0; i < kNumParams; ++i) v[i] = get(i);
        for (int i : {kPressed, kBright, kBreathy, kRough, kFry, kRing, kBodyFemale, kBodyChild, kRounding, kTipCurl, kTwang, kFalseFolds})
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

    void handleMidi(ChimeraMpc* e, const mpc::MidiEvent& m)
    {
        if (m.isNoteOn()) cm_note_on(e, m.note(), m.velocity());
        else if (m.isNoteOff()) cm_note_off(e, m.note());
        else if (m.isAllNotesOff()) cm_all_notes_off(e);
        else if (m.isPitchBend()) cm_pitch_bend(e, (float)m.pitchBend() / 8192.0f);
    }

    std::atomic<ChimeraMpc*> engine_{nullptr};
    std::atomic<bool> inProcess_{false};
    std::atomic<bool> allOff_{false};
    double engineRate_ = 0.0;
    float smoothedGain_ = 1.0f;
    float buf_[kChunk] = {};
};

MPC_PLUGIN_ENTRY(CHIMERAMPC)
