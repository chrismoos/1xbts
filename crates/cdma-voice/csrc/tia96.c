/* TIA-96 / IS-96A Service Option 1 FFI wrapper. */

#include "tia96.h"

#include <stdlib.h>
#include <string.h>

#include "tia96/code/struct.h"

int initialize_encoder_and_decoder();
int encoder();
int decoder();
int dc_block();

typedef struct Tia96EncoderContext {
    struct ENCODER_MEM encoder_mem;
    struct DECODER_MEM decoder_mem;
    struct CONTROL control;
    float input[FSIZE + LPCOFFSET];
    int has_pending;
    int frame_num;
} Tia96EncoderContext;

typedef struct Tia96DecoderContext {
    struct ENCODER_MEM encoder_mem;
    struct DECODER_MEM decoder_mem;
    struct CONTROL control;
} Tia96DecoderContext;

static void init_control(struct CONTROL *control) {
    memset(control, 0, sizeof(*control));
    control->num_frames = UNLIMITED;
    control->max_rate = FULL;
    control->min_rate = EIGHTH;
    control->pf_flag = PF_ON;
}

static void free_pole(struct POLE_FILTER *filter) {
    free(filter->memory);
    free(filter->pole_coeff);
    memset(filter, 0, sizeof(*filter));
}

static void free_zero(struct ZERO_FILTER *filter) {
    free(filter->memory);
    free(filter->zero_coeff);
    memset(filter, 0, sizeof(*filter));
}

static void free_pole_zero(struct POLE_ZERO_FILTER *filter) {
    free(filter->memory);
    free(filter->pole_coeff);
    free(filter->zero_coeff);
    memset(filter, 0, sizeof(*filter));
}

static void free_decoder_mem(struct DECODER_MEM *mem) {
    free(mem->pitch_filt.memory);
    mem->pitch_filt.memory = NULL;
    free_pole(&mem->lpc_filt);
    if (mem->type == ENCODER) {
        free_pole_zero(&mem->wghting_filt);
    } else {
        free_zero(&mem->post_filt_z);
        free_pole(&mem->post_filt_p);
        free_pole_zero(&mem->bright_filt);
    }
}

static void free_encoder_mem(struct ENCODER_MEM *mem) {
    free_zero(&mem->form_res_filt);
    free_pole(&mem->wght_syn_filt);
    free_decoder_mem(&mem->dec);
}

static void free_codec_mem(struct ENCODER_MEM *encoder_mem,
                           struct DECODER_MEM *decoder_mem) {
    free_encoder_mem(encoder_mem);
    free_decoder_mem(decoder_mem);
}

static int encoder_mem_valid(const struct ENCODER_MEM *mem) {
    return mem->form_res_filt.memory != NULL &&
           mem->form_res_filt.zero_coeff != NULL &&
           mem->wght_syn_filt.memory != NULL &&
           mem->wght_syn_filt.pole_coeff != NULL &&
           mem->dec.pitch_filt.memory != NULL &&
           mem->dec.lpc_filt.memory != NULL &&
           mem->dec.lpc_filt.pole_coeff != NULL &&
           mem->dec.wghting_filt.memory != NULL &&
           mem->dec.wghting_filt.pole_coeff != NULL &&
           mem->dec.wghting_filt.zero_coeff != NULL;
}

static int decoder_mem_valid(const struct DECODER_MEM *mem) {
    return mem->pitch_filt.memory != NULL &&
           mem->lpc_filt.memory != NULL &&
           mem->lpc_filt.pole_coeff != NULL &&
           mem->post_filt_z.memory != NULL &&
           mem->post_filt_z.zero_coeff != NULL &&
           mem->post_filt_p.memory != NULL &&
           mem->post_filt_p.pole_coeff != NULL &&
           mem->bright_filt.memory != NULL &&
           mem->bright_filt.pole_coeff != NULL &&
           mem->bright_filt.zero_coeff != NULL;
}

static int bits_for_rate(int rate) {
    switch (rate) {
    case FULL: return 171;
    case HALF: return 80;
    case QUARTER: return 40;
    case EIGHTH: return 16;
    default: return 0;
    }
}

static size_t payload_bytes_for_rate(int rate) {
    int bits;
    bits = bits_for_rate(rate);
    return (size_t)((bits + 7) / 8);
}

static void put_word(uint8_t *out, size_t offset, size_t limit, int word) {
    out[offset] = (uint8_t)(((unsigned int)word >> 8) & 0xffu);
    if (offset + 1 < limit) {
        out[offset + 1] = (uint8_t)((unsigned int)word & 0xffu);
    }
}

static int get_word(const uint8_t *in, size_t bytes, size_t offset) {
    unsigned int hi;
    unsigned int lo;
    hi = offset < bytes ? in[offset] : 0;
    lo = offset + 1 < bytes ? in[offset + 1] : 0;
    return (int)((hi << 8) | lo);
}

void *tia96_encoder_init(void) {
    Tia96EncoderContext *ctx;
    ctx = (Tia96EncoderContext *)calloc(1, sizeof(*ctx));
    if (ctx == NULL) {
        return NULL;
    }
    init_control(&ctx->control);
    initialize_encoder_and_decoder(&ctx->encoder_mem, &ctx->decoder_mem,
                                   &ctx->control);
    if (!encoder_mem_valid(&ctx->encoder_mem) ||
        !decoder_mem_valid(&ctx->decoder_mem)) {
        free_codec_mem(&ctx->encoder_mem, &ctx->decoder_mem);
        free(ctx);
        return NULL;
    }
    return ctx;
}

void tia96_encoder_uninit(void *opaque) {
    Tia96EncoderContext *ctx;
    ctx = (Tia96EncoderContext *)opaque;
    if (ctx == NULL) {
        return;
    }
    free_codec_mem(&ctx->encoder_mem, &ctx->decoder_mem);
    free(ctx);
}

int tia96_encoder_encode_to_packet(void *opaque,
                                   const int16_t *speech,
                                   size_t samples,
                                   uint8_t *packet,
                                   size_t max_bytes) {
    Tia96EncoderContext *ctx;
    struct PACKET encoded;
    struct SIGNAL_DATA *signal;
    float output[FSIZE];
    size_t payload_bytes;
    size_t words;
    size_t i;
    int rate;

    ctx = (Tia96EncoderContext *)opaque;
    if (ctx == NULL || speech == NULL || packet == NULL ||
        samples != TIA96_FRAME_SAMPLES || max_bytes < 3) {
        return -1;
    }

    if (!ctx->has_pending) {
        for (i = 0; i < FSIZE; ++i) {
            ctx->input[i] = (float)speech[i] / 4.0f;
        }
        dc_block(ctx->input, &ctx->encoder_mem.dc_block_mem, LPCOFFSET,
                 &ctx->control);
        ctx->has_pending = 1;
        packet[0] = EIGHTH;
        packet[1] = 0;
        packet[2] = 0;
        return 3;
    }

    for (i = 0; i < LPCOFFSET; ++i) {
        ctx->input[FSIZE + i] = (float)speech[i] / 4.0f;
    }

    memset(&encoded, 0, sizeof(encoded));
    signal = NULL;
    encoder(ctx->input, &encoded, &ctx->control, &signal, &ctx->encoder_mem,
            output, ctx->frame_num++);
    for (i = LPCOFFSET; i < FSIZE; ++i) {
        ctx->input[i] = (float)speech[i] / 4.0f;
    }

    rate = encoded.data[0];
    payload_bytes = payload_bytes_for_rate(rate);
    if (payload_bytes == 0 || 1 + payload_bytes > max_bytes) {
        return -1;
    }
    packet[0] = (uint8_t)rate;
    words = (payload_bytes + 1) / 2;
    for (i = 0; i < words; ++i) {
        put_word(packet, 1 + i * 2, 1 + payload_bytes,
                 encoded.data[1 + i]);
    }
    return (int)(1 + payload_bytes);
}

void *tia96_decoder_init(void) {
    Tia96DecoderContext *ctx;
    ctx = (Tia96DecoderContext *)calloc(1, sizeof(*ctx));
    if (ctx == NULL) {
        return NULL;
    }
    init_control(&ctx->control);
    initialize_encoder_and_decoder(&ctx->encoder_mem, &ctx->decoder_mem,
                                   &ctx->control);
    if (!encoder_mem_valid(&ctx->encoder_mem) ||
        !decoder_mem_valid(&ctx->decoder_mem)) {
        free_codec_mem(&ctx->encoder_mem, &ctx->decoder_mem);
        free(ctx);
        return NULL;
    }
    return ctx;
}

void tia96_decoder_uninit(void *opaque) {
    Tia96DecoderContext *ctx;
    ctx = (Tia96DecoderContext *)opaque;
    if (ctx == NULL) {
        return;
    }
    free_codec_mem(&ctx->encoder_mem, &ctx->decoder_mem);
    free(ctx);
}

int tia96_decoder_decode_from_packet(void *opaque,
                                     const uint8_t *packet,
                                     size_t bytes,
                                     int16_t *speech,
                                     size_t max_samples) {
    Tia96DecoderContext *ctx;
    struct PACKET encoded;
    float output[FSIZE];
    size_t payload_bytes;
    size_t words;
    size_t i;
    float sample;
    int rate;

    ctx = (Tia96DecoderContext *)opaque;
    if (ctx == NULL || packet == NULL || speech == NULL || bytes < 1 ||
        max_samples < FSIZE) {
        return -1;
    }
    rate = packet[0];
    payload_bytes = payload_bytes_for_rate(rate);
    if (payload_bytes == 0 || bytes != 1 + payload_bytes) {
        return -1;
    }

    memset(&encoded, 0, sizeof(encoded));
    encoded.data[0] = rate;
    words = (payload_bytes + 1) / 2;
    for (i = 0; i < words; ++i) {
        encoded.data[1 + i] = get_word(packet, bytes, 1 + i * 2);
    }
    decoder(output, &encoded, &ctx->control, &ctx->decoder_mem);
    for (i = 0; i < FSIZE; ++i) {
        sample = output[i] * 4.0f;
        if (sample > 32767.0f) {
            sample = 32767.0f;
        } else if (sample < -32768.0f) {
            sample = -32768.0f;
        }
        speech[i] = (int16_t)sample;
    }
    return FSIZE;
}
