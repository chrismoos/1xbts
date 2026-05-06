#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "evrc_b_wb/struct.h"

namespace {

enum EvrcBwMode {
    EVRCBW_MODE_B = 1,
    EVRCBW_MODE_WB = 2,
};

struct EvrcBwContext {
    FGV_MEM codec;
    int mode;
};

void init_args(EvrcArgs *args, int mode, int encoder, int operating_point) {
    memset(args, 0, sizeof(*args));
    args->encode_only = encoder ? 1 : 0;
    args->decode_only = encoder ? 0 : 1;
    args->max_rate = 4;
    args->max_rate_default = 1;
    args->min_rate = 1;
    args->min_rate_default = 1;
    args->post_filter = 1;
    args->post_filter_default = 1;
    args->noise_suppression = 1;
    args->noise_suppression_default = 1;
    args->ibuf_len = encoder ? SPEECH_BUFFER_LEN : BITSTREAM_BUFFER_LEN - 1;
    args->obuf_len = encoder ? BITSTREAM_BUFFER_LEN - 1 : SPEECH_BUFFER_LEN;
    args->highpass_filter = 1;
    args->avg_rate_target = 5.3f;
    args->PPP_to_CELP_threshold = -150.0f;
    args->ratewin = 100;
    args->fullrate_coding_method = 0;
    if (operating_point >= 0) {
        args->operating_point = operating_point;
    } else {
        args->operating_point = (mode == EVRCBW_MODE_WB) ? 3 : 0;
    }
    strcpy(args->pattern, "QQF");
    args->verbose = NO;
    args->dtx = 0;
    args->lowband_decout = 0;
    args->Fsop = (mode == EVRCBW_MODE_WB) ? 16000 : 8000;
    args->Fsinp = (mode == EVRCBW_MODE_WB) ? 16000 : 8000;
}

void configure_codec(FGV_MEM *codec, int mode, int encoder, int operating_point) {
    init_args(&codec->args_storage, mode, encoder, operating_point);
    codec->args = &codec->args_storage;
    codec->ibuf_len = codec->args->ibuf_len;
    codec->obuf_len = codec->args->obuf_len;
    codec->write_accshift = 0;
}

void shift_encoder_lookahead(EvrcBwContext *ctx) {
    FGV_MEM *c = &ctx->codec;
    for (int k = 0; k < LOOKAHEAD_LEN; k++) {
        c->buf[k] = c->buf[k + FSIZE];
        c->buf_backup[k] = c->buf_backup[k + FSIZE];
    }

    if (ctx->mode != EVRCBW_MODE_WB) {
        return;
    }

    for (int k = 0; k < c->hb_ana_delay; k++) {
        c->buf_HB[k] = c->buf_HB[7 * c->ibuf_len / 8 + k];
    }
    for (int k = 0; k < LOOKAHEAD_LEN * 7 / 8; k++) {
        c->buf_HB[k + c->hb_ana_delay] = c->lookahead_UB[k];
    }
    for (int k = 0; k < LOOKAHEAD_LEN * 7 / 4 + c->hb_ana_delay * 2; k++) {
        c->buf_WB14[k] = c->buf_WB14[7 * c->ibuf_len / 4 + k];
    }
}

} // namespace

extern "C" {

void *evrcbw_encoder_init_with_operating_point(int mode, int operating_point);

void *evrcbw_encoder_init(int mode) {
    return evrcbw_encoder_init_with_operating_point(mode, -1);
}

void *evrcbw_encoder_init_with_operating_point(int mode, int operating_point) {
    if (mode != EVRCBW_MODE_B && mode != EVRCBW_MODE_WB) {
        return NULL;
    }
    if (mode == EVRCBW_MODE_B && (operating_point < -1 || operating_point > 2)) {
        return NULL;
    }
    if (mode == EVRCBW_MODE_WB && operating_point != -1 && operating_point != 3) {
        return NULL;
    }
    EvrcBwContext *ctx = new EvrcBwContext();
    ctx->mode = mode;
    configure_codec(&ctx->codec, mode, 1, operating_point);
    ctx->codec.InitEncoder();
    return ctx;
}

void evrcbw_encoder_uninit(void *handle) {
    delete static_cast<EvrcBwContext *>(handle);
}

int evrcbw_encoder_encode_to_words(
    void *handle,
    const int16_t *speech,
    size_t speech_samples,
    int16_t *rate,
    int16_t *words,
    size_t words_capacity
) {
    EvrcBwContext *ctx = static_cast<EvrcBwContext *>(handle);
    if (ctx == NULL || speech == NULL || rate == NULL || words == NULL) {
        return -1;
    }
    size_t expected_samples = (ctx->mode == EVRCBW_MODE_WB) ? 320 : 160;
    if (speech_samples != expected_samples || words_capacity < BITSTREAM_BUFFER_LEN - 1) {
        return -2;
    }

    FGV_MEM *c = &ctx->codec;
    memset(c->buf16, 0, sizeof(c->buf16));
    memcpy(c->buf16, speech, expected_samples * sizeof(int16_t));

    if (ctx->mode == EVRCBW_MODE_B) {
        for (c->buf16P = c->buf16, c->bufP = c->buf + LOOKAHEAD_LEN;
             c->buf16P < c->buf16 + SPEECH_BUFFER_LEN;
             c->buf16P++, c->bufP++) {
            *c->bufP = static_cast<float>(*c->buf16P);
        }
    }

    c->WB_encoder();

    *rate = c->data_packet.PACKET_RATE;
    memcpy(words, c->buf16, (BITSTREAM_BUFFER_LEN - 1) * sizeof(int16_t));
    shift_encoder_lookahead(ctx);
    return BITSTREAM_BUFFER_LEN - 1;
}

void *evrcbw_decoder_init(int mode) {
    if (mode != EVRCBW_MODE_B && mode != EVRCBW_MODE_WB) {
        return NULL;
    }
    EvrcBwContext *ctx = new EvrcBwContext();
    ctx->mode = mode;
    configure_codec(&ctx->codec, mode, 0, -1);
    ctx->codec.InitDecoder();
    return ctx;
}

void evrcbw_decoder_uninit(void *handle) {
    delete static_cast<EvrcBwContext *>(handle);
}

int evrcbw_decoder_decode_from_words(
    void *handle,
    int16_t rate,
    const int16_t *words,
    size_t words_count,
    int16_t *speech,
    size_t speech_max_samples
) {
    EvrcBwContext *ctx = static_cast<EvrcBwContext *>(handle);
    if (ctx == NULL || words == NULL || speech == NULL) {
        return -1;
    }
    if (words_count < BITSTREAM_BUFFER_LEN - 1 || speech_max_samples < 160) {
        return -2;
    }

    FGV_MEM *c = &ctx->codec;
    memset(c->buf16, 0, sizeof(c->buf16));
    memcpy(c->buf16, words, (BITSTREAM_BUFFER_LEN - 1) * sizeof(int16_t));
    c->rate = rate;
    c->data_packet.PACKET_RATE = rate;
    c->WB_decoder(1.0f);

    size_t produced = (c->data_packet.WB_MODE_BIT == 1 && !c->args->lowband_decout)
        ? 2 * c->obuf_len
        : c->obuf_len;
    if (produced > speech_max_samples) {
        return -3;
    }
    memcpy(speech, c->buf16, produced * sizeof(int16_t));
    return static_cast<int>(produced);
}

}
