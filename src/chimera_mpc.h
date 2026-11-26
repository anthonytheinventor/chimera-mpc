// chimera_mpc.h - the C interface of CHIMERA MPC's voice engine (engine/, Rust).
//
// The engine is CHIMERA by Jameson Hodge (MIT), with the settings that fit a 32-bit MPC.
// cm_new and cm_free allocate and free: call them outside the audio callback. Everything else is
// for the audio thread and allocates nothing. Every function catches a panic inside the engine;
// after one, the engine renders silence and cm_failed returns 1.

#ifndef CHIMERA_MPC_H
#define CHIMERA_MPC_H

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ChimeraMpc ChimeraMpc;

// content_dir: the plugin's Content folder (lyrics.txt, speech.txt, cmudict.txt.gz), or null.
// Starts the engine's own thread.
ChimeraMpc* cm_new(double sample_rate, const char* content_dir);
void cm_free(ChimeraMpc* engine);

// The engine's parameter defaults in parameter order (n values at most); returns its count.
int cm_param_defaults(float* out, int n);
// Every parameter at once, plain values in parameter order; applied at the next render.
void cm_set_params(ChimeraMpc* engine, const float* values, int n);

// The vowel pad's posture at (front, open), 0..1 each, for the body playing: 9 values (tongue
// fwd, tongue up, jaw, lips open, protrude, lip width, tongue root, larynx, tip curl).
void cm_vowel_posture(ChimeraMpc* engine, float front, float open, float* out9);

// Beat preset `index` (0..6) as the 16 step parameter values, its tempo and swing; 0 if none.
int cm_beat_preset(int index, float* steps16, float* bpm, float* swing);
// The host's transport for the block about to render (frames in `samples`).
void cm_transport(ChimeraMpc* engine, int playing, int bpm_valid, double bpm, int beats_valid, double beats, int samples);

// A raw MIDI message: status (with its channel) and two data bytes.
void cm_midi(ChimeraMpc* engine, int status, int d1, int d2);
void cm_note_on(ChimeraMpc* engine, int note, int velocity);
void cm_note_off(ChimeraMpc* engine, int note);
void cm_all_notes_off(ChimeraMpc* engine);
// -1..1, the wheel's +-2 semitones
void cm_pitch_bend(ChimeraMpc* engine, float bend);

// n mono samples into out.
void cm_render(ChimeraMpc* engine, float* out, int n);
// 1 once the lyrics and spoken lines are ready (Sing sings "ah" until then, Speak is silent).
int cm_lyrics_ready(const ChimeraMpc* engine);
// 1 when the voice playing is the body the parameters ask for (a Blend takes a moment).
int cm_body_ready(const ChimeraMpc* engine);
// What the screen shows now: out[0] the face strip's frame, out[1] the soft palate's frame.
void cm_display(ChimeraMpc* engine, int* out2);
int cm_failed(const ChimeraMpc* engine);

#ifdef __cplusplus
}
#endif

#endif
