/* Stub tty.h -- TTY support is not included in this build.
 * All TTY variables are defined as globals set to 0 (disabled),
 * so the if(tty_option == TTY_NO_GAIN) branches are never taken.
 */
#ifndef _TTY_H_
#define _TTY_H_

#include "evrc_state.h"

#define TTY_DISABLED    0
#define TTY_NO_GAIN     1

#define TTY_DEBUG_PRINT 0x01
#define TTY_DEBUG_DUMP  0x02

#define tty_option           (evrc_current_context()->tty.tty_option)
#define tty_enc_flag         (evrc_current_context()->tty.tty_enc_flag)
#define tty_enc_char         (evrc_current_context()->tty.tty_enc_char)
#define tty_enc_header       (evrc_current_context()->tty.tty_enc_header)
#define tty_enc_baud_rate    (evrc_current_context()->tty.tty_enc_baud_rate)
#define tty_dec_flag         (evrc_current_context()->tty.tty_dec_flag)
#define tty_dec_char         (evrc_current_context()->tty.tty_dec_char)
#define tty_dec_header       (evrc_current_context()->tty.tty_dec_header)
#define tty_dec_baud_rate    (evrc_current_context()->tty.tty_dec_baud_rate)
#define tty_debug_print_flag (evrc_current_context()->tty.tty_debug_print_flag)
#define tty_debug_flag       (evrc_current_context()->tty.tty_debug_flag)

/* Stub function declarations (never called when tty_option == 0) */
static inline void init_tty_enc(short *c, short *h, short *b) { (void)c; (void)h; (void)b; }
static inline void init_tty_dec(void) {}
static inline void tty_debug(void) {}
static inline short tty_enc(short *c, short *h, short *b, short *buf, short len) {
    (void)c; (void)h; (void)b; (void)buf; (void)len; return 0;
}
static inline short tty_dec(short *out, short rate, short hdr, short ch, short baud, short fer) {
    (void)out; (void)rate; (void)hdr; (void)ch; (void)baud; (void)fer; return 0;
}

#endif
