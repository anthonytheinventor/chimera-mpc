// PGO training run for CHIMERA MPC's engine (tools/pgo_profile.py): every mode, as the plugin
// drives it (44.1 kHz, 128-frame blocks, the per-block calls process() makes). Built for arm32 and
// run under qemu-arm; the instrumented engine writes its profile at exit (LLVM_PROFILE_FILE).
// Usage: train <Content folder> [raw float output, for comparing two builds]
#include <stdint.h>
#include <stdio.h>
#include <unistd.h>

typedef struct ChimeraMpc ChimeraMpc;
ChimeraMpc* cm_new(double sample_rate, const char* content_dir, const void* texts);
void cm_free(ChimeraMpc* e);
int cm_param_defaults(float* out, int n);
void cm_set_params(ChimeraMpc* e, const float* values, int n);
void cm_vowel_posture(ChimeraMpc* e, float front, float open, float* out9);
int cm_beat_preset(int index, float* steps16, float* bpm, float* swing);
int cm_beat_preset_notes(int index, float* notes16, float* accents16);
void cm_step_notes(ChimeraMpc* e, const float* notes16, const float* accents16);
uint32_t cm_now_next(ChimeraMpc* e, uint8_t* out, int cap);
uint32_t cm_text_preview(ChimeraMpc* e, uint8_t* out, int cap);
void cm_transport(ChimeraMpc* e, int playing, int bpm_valid, double bpm, int beats_valid, double beats, int samples);
void cm_note_on(ChimeraMpc* e, int note, int velocity);
void cm_note_off(ChimeraMpc* e, int note);
void cm_all_notes_off(ChimeraMpc* e);
void cm_render(ChimeraMpc* e, float* out, int n);
int cm_lyrics_ready(const ChimeraMpc* e);
int cm_body_ready(const ChimeraMpc* e);
void cm_display(ChimeraMpc* e, int* out4);
void cm_face(ChimeraMpc* e, int* out6);
void cm_step_lit(ChimeraMpc* e, int* out2);
void cm_pad_notes(ChimeraMpc* e, int* out3);
void cm_pad_learn(ChimeraMpc* e, int* out18);
void cm_text_cursor(ChimeraMpc* e, int from_end);
void cm_pad_map(ChimeraMpc* e, const int* map16);
int cm_failed(const ChimeraMpc* e);

enum { BODY = 11, BODY_FEMALE = 12, BODY_CHILD = 13, TONGUE_FWD = 24, TONGUE_UP = 25, JAW = 14, LIP_OPEN = 15,
       PROTRUDE = 16, ROUNDING = 17, ROOT = 18, LARYNX = 21, TIP_CURL = 19, VOWEL_FRONT = 28, VOWEL_OPEN = 29,
       MODE = 32, BEAT = 45, TEMPO = 46, SWING = 47, MIC = 48, FOLLOW = 49, PLAY_BEAT = 50, STEP_1 = 51, COUNT = 85 };

static ChimeraMpc* e;
static float v[COUNT];
static float notes[16], accents[16];
static double beats;
static int transport;
static double peak;
static FILE* dump;

static void block(void)
{
    float buf[128];
    uint8_t text[64];
    int out[18], map[16];
    for (int i = 0; i < 16; ++i) map[i] = 36 + i;
    cm_step_notes(e, notes, accents);
    cm_pad_map(e, map);
    cm_transport(e, transport, 1, 100.0, transport, beats, 128);
    if (transport) beats += 100.0 / 60.0 * 128.0 / 44100.0;
    cm_set_params(e, v, COUNT);
    cm_text_cursor(e, 0);
    cm_render(e, buf, 64);
    cm_render(e, buf + 64, 64);
    if (dump) fwrite(buf, sizeof(float), 128, dump);
    for (int i = 0; i < 128; ++i) { double a = buf[i] < 0 ? -buf[i] : buf[i]; if (a > peak) peak = a; }
    cm_text_preview(e, text, sizeof text);
    cm_now_next(e, text, sizeof text);
    cm_display(e, out);
    cm_face(e, out);
    cm_step_lit(e, out);
    cm_pad_learn(e, out);
    cm_pad_notes(e, out);
}

static void run(double seconds) { for (int n = (int)(seconds * 44100.0 / 128.0); n > 0; --n) block(); }

static void vowel(float front, float open)
{
    static const int keys[9] = {TONGUE_FWD, TONGUE_UP, JAW, LIP_OPEN, PROTRUDE, ROUNDING, ROOT, LARYNX, TIP_CURL};
    float a[9];
    v[VOWEL_FRONT] = front;
    v[VOWEL_OPEN] = open;
    cm_vowel_posture(e, front, open, a);
    for (int k = 0; k < 9; ++k) v[keys[k]] = keys[k] == ROUNDING ? 1.0f - a[k] : a[k];
}

static void tune(const int* ns, int count, double on, double off)
{
    for (int i = 0; i < count; ++i) { cm_note_on(e, ns[i], 70 + (i * 17) % 50); run(on); cm_note_off(e, ns[i]); run(off); }
}

static void beat(int preset)
{
    float steps[16], bpm, swing;
    cm_beat_preset(preset, steps, &bpm, &swing);
    cm_beat_preset_notes(preset, notes, accents);
    for (int i = 0; i < 16; ++i) v[STEP_1 + i] = steps[i];
    v[TEMPO] = bpm;
    v[SWING] = swing;
    v[BEAT] = (float)preset;
}

int main(int argc, char** argv)
{
    const char* content = argc > 1 ? argv[1] : "Content";
    if (argc > 2) dump = fopen(argv[2], "wb");
    e = cm_new(44100.0, content, 0);
    if (!e) { fprintf(stderr, "cm_new failed\n"); return 1; }
    cm_param_defaults(v, COUNT);
    block(); sleep(20); block();
    if (!cm_lyrics_ready(e)) { fputs("lyrics not ready", stderr); return 2; }

    // Sing: the three bodies
    static const int melody[] = {60, 62, 64, 65, 67, 65, 64, 62, 60, 57, 59, 60};
    v[MODE] = 0;
    for (int b = 0; b < 3; ++b) { v[BODY] = (float)b; tune(melody, 12, 0.32, 0.08); }
    // Speak
    v[MODE] = 2; v[BODY] = 0;
    static const int speak[] = {60, 55, 64, 60, 52};
    tune(speak, 5, 1.6, 0.2);
    v[BODY] = 1;
    tune(speak, 3, 1.6, 0.2);
    // Play: held notes, the vowel pad moving, legato
    v[MODE] = 1; v[BODY] = 0;
    cm_note_on(e, 57, 100);
    for (int k = 0; k < 40; ++k) { vowel((float)((k * 7) % 10) / 10.0f, (float)((k * 3) % 10) / 10.0f); run(0.1); }
    cm_note_on(e, 60, 90); run(0.8); cm_note_off(e, 60); run(0.5); cm_note_off(e, 57); run(0.4);
    // Play on a Blend
    v[BODY] = 3; v[BODY_FEMALE] = 0.5f; v[BODY_CHILD] = 0.3f;
    block(); sleep(10); block();
    if (!cm_body_ready(e)) { fputs("blend not ready", stderr); return 2; }
    cm_note_on(e, 55, 100); run(1.5); cm_note_off(e, 55); run(0.3);
    v[BODY] = 0;
    // Beatbox: the sequencer with Mic FX, presets, pads and hums
    v[MODE] = 3; v[MIC] = 1; v[FOLLOW] = 0; v[PLAY_BEAT] = 1;
    for (int p = 0; p < 7; ++p) { beat(p); run(1.6); }
    beat(0);
    for (int i = 0; i < 16; ++i) { cm_note_on(e, 36 + i, 110); run(0.12); cm_note_off(e, 36 + i); }
    cm_note_on(e, 64, 100); run(1.0); cm_note_off(e, 64); run(0.3);
    v[MIC] = 0; run(2.0);
    // following the host's transport
    v[FOLLOW] = 1; v[PLAY_BEAT] = 0; transport = 1; run(3.0); transport = 0; run(0.3);
    v[PLAY_BEAT] = 0; v[MIC] = 1;
    cm_all_notes_off(e);
    // idle
    v[MODE] = 0; run(1.0);

    fprintf(stderr, "done: failed=%d peak=%.3f\n", cm_failed(e), peak);
    cm_free(e);
    if (dump) fclose(dump);
    return 0;
}
