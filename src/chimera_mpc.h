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

ChimeraMpc* cm_new(double sample_rate);
void cm_free(ChimeraMpc* engine);

// The engine's parameter defaults in parameter order (n values at most); returns its count.
int cm_param_defaults(float* out, int n);
// Every parameter at once, plain values in parameter order; applied at the next render.
void cm_set_params(ChimeraMpc* engine, const float* values, int n);

void cm_note_on(ChimeraMpc* engine, int note, int velocity);
void cm_note_off(ChimeraMpc* engine, int note);
void cm_all_notes_off(ChimeraMpc* engine);
// -1..1, the wheel's +-2 semitones
void cm_pitch_bend(ChimeraMpc* engine, float bend);

// n mono samples into out.
void cm_render(ChimeraMpc* engine, float* out, int n);
int cm_failed(const ChimeraMpc* engine);

#ifdef __cplusplus
}
#endif

#endif
