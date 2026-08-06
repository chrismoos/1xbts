/* TIA-96 / IS-96A Service Option 1 FFI wrapper. */

#ifndef TIA96_FFI_H
#define TIA96_FFI_H

#include <stddef.h>
#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define TIA96_FRAME_SAMPLES 160
#define TIA96_MAX_PACKET_BYTES 23

void *tia96_encoder_init(void);
void tia96_encoder_uninit(void *ctx);
int tia96_encoder_encode_to_packet(void *ctx,
                                   const int16_t *speech,
                                   size_t samples,
                                   uint8_t *packet,
                                   size_t max_bytes);

void *tia96_decoder_init(void);
void tia96_decoder_uninit(void *ctx);
int tia96_decoder_decode_from_packet(void *ctx,
                                     const uint8_t *packet,
                                     size_t bytes,
                                     int16_t *speech,
                                     size_t max_samples);

#ifdef __cplusplus
}
#endif

#endif
