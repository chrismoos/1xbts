/*
 * QCELP-13K (TIA/ANSI-733) FFI wrapper.
 *
 * Public C API consumed by Rust (crates/cdma-voice/src/qcelp13k.rs).
 * Every entry point manipulates a heap-allocated context; no
 * process-wide writable state.
 */

#ifndef QCELP13K_FFI_H
#define QCELP13K_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Speech frame is fixed at 160 samples @ 8 kHz = 20 ms. */
#define QCELP13K_FRAME_SAMPLES 160

/*
 * Maximum wire-packet size produced by qcelp13k_encoder_encode_to_packet():
 *   1 rate byte + 34 payload bytes (full rate). Callers should provide
 *   at least this many bytes.
 */
#define QCELP13K_MAX_PACKET_BYTES 35

/*
 * Encoder lifecycle.
 *
 * max_rate / min_rate use the TIA-733 mode numbering:
 *   1 = Eighth, 2 = Quarter, 3 = Half, 4 = Full
 * Passing max=4, min=1 enables the full variable-rate range.
 */
void *qcelp13k_encoder_init(int max_rate, int min_rate);
void  qcelp13k_encoder_uninit(void *ctx);

/*
 * Encode one 20 ms (160-sample) PCM frame. Returns the number of bytes
 * written to `packet` (1 rate byte + N payload bytes), or a negative
 * value on error.
 */
int   qcelp13k_encoder_encode_to_packet(void *ctx,
                                        const int16_t *speech,
                                        size_t samples,
                                        uint8_t *packet,
                                        size_t max_bytes);

/*
 * Decoder lifecycle.
 */
void *qcelp13k_decoder_init(void);
void  qcelp13k_decoder_uninit(void *ctx);

/*
 * Decode one wire packet (1 rate byte + N payload bytes) into a 20 ms
 * (160-sample) PCM frame. Returns the number of samples written
 * (always QCELP13K_FRAME_SAMPLES on success), or a negative value on
 * error.
 */
int   qcelp13k_decoder_decode_from_packet(void *ctx,
                                          const uint8_t *packet,
                                          size_t bytes,
                                          int16_t *speech,
                                          size_t max_samples);

#ifdef __cplusplus
}
#endif

#endif /* QCELP13K_FFI_H */
