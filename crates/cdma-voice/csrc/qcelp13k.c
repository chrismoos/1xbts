/*
 * QCELP-13K (TIA/ANSI-733) FFI wrapper.
 *
 * Owns per-instance state for the floating-point reference encoder/
 * decoder in csrc/qcelp13k/code/. All call entry points operate on a
 * heap-allocated context; no process-wide writable state is touched
 * after one-time table initialisation (guarded by pthread_once).
 *
 * Wire packet format (see MODIFICATIONS.txt):
 *   byte 0 : rate (mode) in {1 = Eighth, 2 = Quarter, 3 = Half, 4 = Full}
 *   bytes 1..N : NUMBITS[mode] bits of TIA-733 frame data, MSB-first,
 *                zero-padded to a whole number of bytes.
 */

#include "qcelp13k.h"

#include <pthread.h>
#include <stdlib.h>
#include <string.h>

#include "qcelp13k/code/celp.h"
#include "qcelp13k/code/coder.h"
#include "qcelp13k/code/coderate.h"

/* -------------------------------------------------------------------
 * TTY / FER-sim stubs.
 *
 * The vendored encode.c / decode.c reference these symbols inside
 * runtime-dead branches (tty_option = 0, trans_fname = NULL). They
 * are declared in code/tty.h. Backing storage lives here.
 *
 * `__thread` storage classes prevent process-wide writable state.
 * ----------------------------------------------------------------- */
__thread int tty_option         = 0;
__thread int tty_enc_flag       = 0;
__thread int tty_enc_char       = 0;
__thread int tty_enc_header     = 0;
__thread int tty_enc_baud_rate  = 0;
__thread int tty_debug_flag     = 0;
__thread int tty_dec_flag       = 0;
__thread int tty_dec_char       = 0;
__thread int tty_dec_header     = 0;
__thread int tty_dec_baud_rate  = 0;
__thread int fer_sim_seed       = 0;

/* Frame-error simulation is not part of this embedded build. */
char *const trans_fname = (char *)0;

/* tty_enc / tty_dec / fer_sim stubs -- all dead-branch sinks. */
int tty_enc(int *c, int *h, int *b, float *in_buf, int len) {
    (void)c; (void)h; (void)b; (void)in_buf; (void)len;
    return 0;
}
int tty_dec(short *out, int qb, int hdr, int chr, int br,
            int fer, int sf, int sf_count, int sf_len) {
    (void)out; (void)qb; (void)hdr; (void)chr; (void)br;
    (void)fer; (void)sf; (void)sf_count; (void)sf_len;
    return 0;
}
void fer_sim(int *rate) { (void)rate; }

/* usage(struct CONTROL*) is the upstream CLI help printer; lpc.c
 * references it in a dead branch (unknown window type). The wrapper
 * always passes HAMMING, so this stub is never executed. */
void usage(struct CONTROL *control) {
    (void)control;
    abort();
}

/* -------------------------------------------------------------------
 * One-time table quantisation (init.c LSPVQ* / CODEBOOK* in-place
 * rewrite). Guard with pthread_once to keep idempotent rewrites
 * race-free across concurrent encoder_init() calls.
 * ----------------------------------------------------------------- */
static pthread_once_t qcelp13k_tables_once = PTHREAD_ONCE_INIT;

static void qcelp13k_init_tables_once(void) {
    initialize_static_codebooks();
}

/* -------------------------------------------------------------------
 * Contexts.
 * ----------------------------------------------------------------- */
typedef struct Qcelp13kEncoderContext {
    struct ENCODER_MEM e_mem;
    /* Upstream initialize_encoder_and_decoder() expects a separate
     * DECODER_MEM (the "Rx" decoder); celp13k.c main() passes its
     * decoder_memory. The encoder body itself only uses e_mem.dec
     * (the encoder-internal "Tx" decoder), so this slot is purely
     * for the init/free contract. Sharing one DECODER_MEM with e_mem.dec
     * causes free_encoder_and_decoder() to double-free their alias. */
    struct DECODER_MEM aux_d_mem;
    struct CONTROL     control;
    /* persistent input-speech ring (LPCSIZE-FSIZE+LPCOFFSET = 60
     * lookback samples + FSIZE = 160 fresh) per celp13k.c main(). */
    float              in_speech[LPCSIZE - FSIZE + LPCOFFSET + FSIZE];
    float              out_speech[FSIZE];
} Qcelp13kEncoderContext;

typedef struct Qcelp13kDecoderContext {
    struct DECODER_MEM d_mem;
    struct CONTROL     control;
} Qcelp13kDecoderContext;

/* -------------------------------------------------------------------
 * Mode <-> NUMBITS table (mirrors pack.h NUMBITS[NUMMODES]).
 * Index: mode (1..4). Mode 0 (BLANK) and 0xe (ERASURE) have no
 * payload bits.
 * ----------------------------------------------------------------- */
static int numbits_for_mode(int mode) {
    switch (mode) {
        case 1: return 20;  /* EIGHTH  */
        case 2: return 54;  /* QUARTER */
        case 3: return 124; /* HALF    */
        case 4: return 266; /* FULL    */
        default: return 0;  /* BLANK / ERASURE / unknown */
    }
}

/* Bytes occupied by the bit-packed payload (ceil(bits / 8)). */
static size_t payload_bytes_for_mode(int mode) {
    int bits = numbits_for_mode(mode);
    return (size_t)((bits + 7) / 8);
}

/* -------------------------------------------------------------------
 * Encoder.
 * ----------------------------------------------------------------- */
void *qcelp13k_encoder_init(int max_rate, int min_rate) {
    Qcelp13kEncoderContext *ctx =
        (Qcelp13kEncoderContext *)malloc(sizeof(Qcelp13kEncoderContext));
    if (ctx == NULL) {
        return NULL;
    }
    memset(ctx, 0, sizeof(*ctx));

    /* CONTROL defaults (mirrors parse_command_line() in celp13k.c). */
    ctx->control.min_rate            = (min_rate > 0 && min_rate <= 4) ? min_rate : 1;
    ctx->control.max_rate            = (max_rate > 0 && max_rate <= 4) ? max_rate : 4;
    ctx->control.avg_rate            = 9.0f;
    ctx->control.target_snr_thr      = 10.0f;
    ctx->control.per_wght            = PERCEPT_WGHT_FACTOR;
    ctx->control.pf_flag             = 1;     /* PF_ON / postfilter on */
    ctx->control.cb_out              = 0;
    ctx->control.pitch_out           = 0;
    ctx->control.num_frames          = -1;    /* UNLIMITED */
    ctx->control.print_packets       = 0;
    ctx->control.output_encoder_speech = 0;
    ctx->control.form_res_out        = 0;
    ctx->control.reduced_rate_flag   = 0;
    ctx->control.unvoiced_off        = 0;
    ctx->control.pitch_post          = 1;     /* YES */
    ctx->control.target_after_out    = 0;
    ctx->control.decode_only         = 0;
    ctx->control.encode_only         = 0;
    ctx->control.fractional_pitch    = 1;     /* YES */

    /* Drive the one-time table quantisation exactly once across all
     * encoders/decoders in this process. */
    pthread_once(&qcelp13k_tables_once, qcelp13k_init_tables_once);

    /* The initializer touches only memory owned by this context. */
    initialize_encoder_and_decoder(&ctx->e_mem, &ctx->aux_d_mem, &ctx->control);

    /* in_speech[] starts as zero-padded history (matches celp13k.c). */
    return ctx;
}

void qcelp13k_encoder_uninit(void *c) {
    Qcelp13kEncoderContext *ctx = (Qcelp13kEncoderContext *)c;
    if (ctx == NULL) return;
    /* free_encoder_and_decoder() frees both ENCODER_MEM filters and the
     * companion DECODER_MEM filters. Pass our aux_d_mem (the same one
     * we initialised) so we don't alias e_mem.dec into a double-free. */
    free_encoder_and_decoder(&ctx->e_mem, &ctx->aux_d_mem);
    free(ctx);
}

/* Pack 16 bits of `packet.data[idx]` (low half-word) into the wire
 * buffer at byte offset `*byte_off`, big-endian. */
static void put16_be(uint8_t *out, size_t *off, int value) {
    uint16_t v = (uint16_t)(value & 0xFFFFu);
    out[(*off)++] = (uint8_t)((v >> 8) & 0xFFu);
    out[(*off)++] = (uint8_t)(v & 0xFFu);
}

static uint16_t get16_be(const uint8_t *in, size_t *off, size_t end) {
    uint16_t hi = (*off < end) ? in[(*off)++] : 0;
    uint16_t lo = (*off < end) ? in[(*off)++] : 0;
    return (uint16_t)((hi << 8) | lo);
}

int qcelp13k_encoder_encode_to_packet(void *c,
                                      const int16_t *speech,
                                      size_t samples,
                                      uint8_t *packet,
                                      size_t max_bytes) {
    Qcelp13kEncoderContext *ctx = (Qcelp13kEncoderContext *)c;
    struct PACKET frame_packet;
    int mode;
    size_t bits, payload_bytes;
    size_t off;
    size_t words_to_emit;
    size_t i;

    if (ctx == NULL || speech == NULL || packet == NULL) {
        return -1;
    }
    if (samples < QCELP13K_FRAME_SAMPLES) {
        return -1;
    }
    if (max_bytes < QCELP13K_MAX_PACKET_BYTES) {
        return -1;
    }

    /* Load the 160 fresh samples into the back of in_speech[]. */
    const size_t HEAD = LPCSIZE - FSIZE + LPCOFFSET; /* 60 */
    for (i = 0; i < FSIZE; ++i) {
        ctx->in_speech[HEAD + i] = (float)speech[i];
    }

    memset(&frame_packet, 0, sizeof(frame_packet));
    /* clear_packet_params() inside encoder() will zero per-field arrays. */

    encoder(ctx->in_speech, &frame_packet, &ctx->control,
            &ctx->e_mem, ctx->out_speech);

    /* Shift the in_speech history forward for the next call. */
    for (i = 0; i < HEAD; ++i) {
        ctx->in_speech[i] = ctx->in_speech[i + FSIZE];
    }

    /* frame_packet.data[0] is the mode (rate). */
    mode = frame_packet.data[0];
    bits = (size_t)numbits_for_mode(mode);
    payload_bytes = payload_bytes_for_mode(mode);

    if (1 + payload_bytes > max_bytes) {
        return -1;
    }

    packet[0] = (uint8_t)(mode & 0xFFu);

    /* pack.c always writes WORDS_PER_PACKET-1 = 17 16-bit words into
     * frame_packet.data[1..17], MSB-first. Emit only the words that
     * cover NUMBITS[mode] bits; the tail beyond `payload_bytes` is
     * spec-reserved and discarded on the wire. */
    words_to_emit = (bits + 15) / 16;  /* ceil(bits / 16) */
    off = 1;
    for (i = 0; i < words_to_emit; ++i) {
        put16_be(packet, &off, frame_packet.data[1 + i]);
    }
    /* `off` may now be one byte past `payload_bytes + 1` if NUMBITS is
     * not a multiple of 8 (Full=266 bits -> 17 words = 34 bytes, exact;
     * Half=124 -> 8 words = 16 bytes, exact; Quarter=54 -> 4 words = 8
     * bytes, payload_bytes_for_mode reports 7; Eighth=20 -> 2 words = 4
     * bytes, payload_bytes_for_mode reports 3). Clamp to spec size. */
    return (int)(1 + payload_bytes);
}

/* -------------------------------------------------------------------
 * Decoder.
 * ----------------------------------------------------------------- */
void *qcelp13k_decoder_init(void) {
    Qcelp13kDecoderContext *ctx =
        (Qcelp13kDecoderContext *)malloc(sizeof(Qcelp13kDecoderContext));
    if (ctx == NULL) {
        return NULL;
    }
    memset(ctx, 0, sizeof(*ctx));

    ctx->control.min_rate            = 1;
    ctx->control.max_rate            = 4;
    ctx->control.avg_rate            = 9.0f;
    ctx->control.target_snr_thr      = 10.0f;
    ctx->control.per_wght            = PERCEPT_WGHT_FACTOR;
    ctx->control.pf_flag             = 1;
    ctx->control.pitch_post          = 1;
    ctx->control.fractional_pitch    = 1;

    pthread_once(&qcelp13k_tables_once, qcelp13k_init_tables_once);

    /* initialize_decoder() (init.c:63) sets up most of the decoder-only
     * state but does NOT initialise the bpf_unv FIR filter that
     * decode.c uses for EIGHTH / QUARTERRATE_UNVOICED frames. The
     * full bpf_unv init lives in initialize_encoder_and_decoder()
     * (init.c:186). Replicate the relevant piece here. */
    initialize_decoder(&ctx->d_mem);
    {
        extern const float unv_filter[];
        int i;
        initialize_zero_filter(&ctx->d_mem.bpf_unv, FIR_UNV_LEN);
        for (i = 0; i < FIR_UNV_LEN; ++i) {
            ctx->d_mem.bpf_unv.zero_coeff[i] = unv_filter[i];
        }
    }
    return ctx;
}

void qcelp13k_decoder_uninit(void *c) {
    Qcelp13kDecoderContext *ctx = (Qcelp13kDecoderContext *)c;
    if (ctx == NULL) return;
    /* initialize_decoder() calloc's only pitch_filt / pitch_filt_per
     * memories and the pole/zero filter coefficients; free them. */
    free((char *)ctx->d_mem.pitch_filt.memory);
    free((char *)ctx->d_mem.pitch_filt_per.memory);
    free_pole_filter(&ctx->d_mem.lpc_filt);
    free_pole_filter(&ctx->d_mem.post_filt_p);
    free_zero_filter(&ctx->d_mem.post_filt_z);
    free_zero_filter(&ctx->d_mem.pitch_sm);
    free_pole_filter(&ctx->d_mem.bright_filt);
    free_zero_filter(&ctx->d_mem.bpf_unv);
    free(ctx);
}

int qcelp13k_decoder_decode_from_packet(void *c,
                                        const uint8_t *packet,
                                        size_t bytes,
                                        int16_t *speech,
                                        size_t max_samples) {
    Qcelp13kDecoderContext *ctx = (Qcelp13kDecoderContext *)c;
    struct PACKET frame_packet;
    float out_speech[FSIZE];
    int mode;
    size_t bits, payload_bytes, words_in_payload, words_in_packet;
    size_t off;
    size_t i;

    if (ctx == NULL || packet == NULL || speech == NULL) {
        return -1;
    }
    if (bytes < 1 || max_samples < FSIZE) {
        return -1;
    }

    mode = packet[0];
    bits = (size_t)numbits_for_mode(mode);
    payload_bytes = payload_bytes_for_mode(mode);

    if (bits == 0 || bytes < 1 + payload_bytes) {
        return -1;
    }

    memset(&frame_packet, 0, sizeof(frame_packet));
    frame_packet.data[0] = mode;

    /* unpack_frame() walks all WORDS_PER_PACKET-1 = 17 words; emit each
     * 16-bit word from the wire payload, zero-padding the tail past
     * NUMBITS[mode] / past `payload_bytes`. */
    words_in_payload = (bits + 15) / 16;
    words_in_packet = (size_t)(WORDS_PER_PACKET - 1);
    off = 1;
    for (i = 0; i < words_in_payload && i < words_in_packet; ++i) {
        frame_packet.data[1 + i] = (int)get16_be(packet, &off, bytes);
    }
    for (; i < words_in_packet; ++i) {
        frame_packet.data[1 + i] = 0;
    }

    decoder(out_speech, &frame_packet, &ctx->control, &ctx->d_mem);

    /* Convert float -> int16 with saturation. */
    for (i = 0; i < FSIZE; ++i) {
        float s = out_speech[i];
        if (s > 32767.0f) s = 32767.0f;
        else if (s < -32768.0f) s = -32768.0f;
        speech[i] = (int16_t)s;
    }
    return (int)FSIZE;
}
