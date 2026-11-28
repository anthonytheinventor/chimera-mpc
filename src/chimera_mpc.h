// chimera_mpc.h - the C interface of CHIMERA MPC's voice engine (engine/, Rust).
//
// The engine is CHIMERA by Jameson Hodge (MIT), with the settings that fit a 32-bit MPC.
// cm_new and cm_free allocate and free: call them outside the audio callback. Everything else is
// for the audio thread and allocates nothing. Every function catches a panic inside the engine;
// after one, the engine renders silence and cm_failed returns 1.

#ifndef CHIMERA_MPC_H
#define CHIMERA_MPC_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

typedef struct ChimeraMpc ChimeraMpc;

// content_dir: the plugin's Content folder (lyrics.txt, speech.txt, cmudict.txt.gz), or null;
// texts: the plugin's text bank (cm_texts_new), or null. Starts the engine's own thread.
ChimeraMpc* cm_new(double sample_rate, const char* content_dir, const void* texts);

// The text bank for the words typed on the MPC: one per plugin, given to every engine it makes;
// freed after the last engine. Save and load are for the host's thread (they may wait).
const void* cm_texts_new(void);
void cm_texts_free(const void* texts);
int cm_texts_save(const void* texts, uint8_t* out, int cap);
int cm_texts_load(const void* texts, const uint8_t* data, int len);
// The slots' size (characters) of the build that saved these texts: 600 before it was recorded,
// 0 if they are not ours (the Cursor parameter's steps follow it).
int cm_texts_saved_cap(const uint8_t* data, int len);
// The typed slot's end ("_" for the cursor) into out (NUL-terminated); returns a serial that
// moves whenever it changes. The audio thread.
uint32_t cm_text_preview(ChimeraMpc* engine, uint8_t* out, int cap);
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
// Beat preset `index` (0..6)'s hum notes (16 values: a MIDI note, or 0 for the hum's own note) and
// accents (16 values, 0 or 1); 0 if none.
int cm_beat_preset_notes(int index, float* notes16, float* accents16);
// The steps' hum notes (16 MIDI notes, 0 = the hum's own note) and accents (16 values, 0 or 1), as
// the sequencer and the pads play the squares. Once a block, on the audio thread (applied when
// they change).
void cm_step_notes(ChimeraMpc* engine, const float* notes16, const float* accents16);
// The screen's NOW and NEXT as "NOW\nNEXT" into out (UTF-8, NUL-terminated; each part at most 31
// bytes): Sing's syllable last sung and the one the next note sings, Speak's line last said and the
// one the next note says, empty in the other modes. Returns a serial that moves whenever either
// changes. Once a block, on the audio thread.
uint32_t cm_now_next(ChimeraMpc* engine, uint8_t* out, int cap);
// The host's transport for the block about to render (frames in `samples`).
void cm_transport(ChimeraMpc* engine, int playing, int bpm_valid, double bpm, int beats_valid, double beats, int samples);

// A raw MIDI message: status (with its channel) and two data bytes.
void cm_midi(ChimeraMpc* engine, int status, int d1, int d2);
// After a state load: the momentary controls' values (Speak, Consonant, Pad, Key) as they now
// stand do not act; only a later change does.
void cm_settle(ChimeraMpc* engine);
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
// What the screen shows now: out[0] 0 (it was the old face strip's frame), out[1] the soft
// palate's frame (0 shut, then open), out[2] the last drum sound heard (0 none, 1..10 as the Pad
// parameter), out[3] a count that moves at every hit.
void cm_display(ChimeraMpc* engine, int* out4);
// The face's layers now: out[0] the jaw's frame, out[1] the tongue's, out[2] the upper lip's,
// out[3] the lower lip's (in the strip of the jaw shown), out[4] the larynx's, out[5] the sound
// level (0 silent .. 5), from the output since the last call. Once a block, on the audio thread;
// allocates nothing.
void cm_face(ChimeraMpc* engine, int* out6);
// How many frames each of cm_face's layers has, in the same order (any thread).
void cm_face_frames(int* out6);
// The frames cm_face reports for the rest posture, silent (the display parameters' defaults).
// Allocates (call it outside the audio callback).
void cm_face_rest(int* out6);
// A tap on a momentary control: parameter index (Key, Pad, Consonant or Speak) and the step it was
// set to. The audio thread; those parameters then go to cm_set_params as 0.
void cm_tap(ChimeraMpc* engine, int index, int value);
// The BEATBOX square last heard into out[0] (0 none, 1..16: a pad hit, as Pad 1 Note maps the
// notes, or the beat reaching that step), and a count that moves at every one into out[1].
void cm_step_lit(ChimeraMpc* engine, int* out2);
// The notes: out[0] the last note played (0..127, -1 none), out[1] a count that moves at every
// note, out[2] the Pad 1 Note setting Learn learned while the parameter says Learn (else -1: the
// plugin writes it into the parameter). Once a block.
void cm_pad_notes(ChimeraMpc* engine, int* out3);
// Beatbox's latency while the sequence runs, in seconds (engine/src/lib.rs BEAT_LATENCY): the
// plugin reports it in Beatbox mode.
double cm_beat_latency(void);
// The typing cursor: characters before the end of the slot being typed (0: at the end). The audio
// thread, once a block.
void cm_text_cursor(ChimeraMpc* engine, int from_end);
// Custom's table of the pads' notes (16 values, Pad Map 1-16). The audio thread, once a block.
void cm_pad_map(ChimeraMpc* engine, const int* map16);
// Learn All's progress: out[0] the next pad to hit (0..15), 16 when done, -1 when not learning;
// out[2..18] the notes learned. The audio thread.
void cm_pad_learn(ChimeraMpc* engine, int* out18);
int cm_failed(const ChimeraMpc* engine);

#ifdef __cplusplus
}
#endif

#endif
