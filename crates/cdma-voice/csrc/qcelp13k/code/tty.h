/*
 * Stub tty.h for the vendored QCELP-13K reference.
 *
 * The upstream archive ships a tty/ subtree implementing TTY-over-QCELP.
 * v1 of this integration does NOT vendor tty/. The encoder() and
 * decoder() entry points reference a small set of TTY symbols
 * (tty_option, tty_enc_flag, tty_enc(), and constants TTY_NO_GAIN /
 * TTY_SILENCE). This header supplies const-zero definitions so every
 * TTY branch is statically dead and the floating-point reference
 * code compiles unchanged. See MODIFICATIONS.txt #14.
 */

#ifndef _QCELP_TTY_STUB_H_
#define _QCELP_TTY_STUB_H_

#include "tty_hdr.h"

/* TTY operating modes. tty_option is hard-coded to TTY_OFF (=0) and
 * TTY_NO_GAIN is defined to a non-zero value so the branch
 * `if (tty_option == TTY_NO_GAIN)` is always false. */
#define TTY_OFF          0
#define TTY_NO_GAIN      1
#define TTY_SILENCE      0x0001  /* matches tty_hdr.h */

/* tty_option / tty_enc_flag etc. are declared as plain ints so existing
 * encode.c / decode.c code that takes their address compiles unchanged.
 * The corresponding definitions in csrc/qcelp13k.c are __thread (TLS)
 * variables initialised to 0 and never written by the production
 * code path, so no process-wide writable state is introduced. */
#define QCELP_TTY_TLS __thread
extern QCELP_TTY_TLS int  tty_option;
extern QCELP_TTY_TLS int  tty_enc_flag;
extern QCELP_TTY_TLS int  tty_enc_char;
extern QCELP_TTY_TLS int  tty_enc_header;
extern QCELP_TTY_TLS int  tty_enc_baud_rate;
extern QCELP_TTY_TLS int  tty_debug_flag;
/* trans_fname is initialised once to NULL in csrc/qcelp13k.c and never
 * reassigned -- the FER-sim branch at decode.c:336 stays dead. The local
 * `extern char *trans_fname;` decl inside decoder() rebinds to the same
 * definition. */
extern char *const        trans_fname;
extern QCELP_TTY_TLS int  fer_sim_seed;

/* Mirror set used by code/decode.c. */
extern QCELP_TTY_TLS int  tty_dec_flag;
extern QCELP_TTY_TLS int  tty_dec_char;
extern QCELP_TTY_TLS int  tty_dec_header;
extern QCELP_TTY_TLS int  tty_dec_baud_rate;

/* tty_enc() / tty_dec() are referenced from code/encode.c and
 * code/decode.c respectively inside `if (tty_option == TTY_NO_GAIN)`
 * branches that are dead at run time because tty_option is 0. The
 * stubs return 0 ("not handled"). */
int tty_enc(int *tty_enc_char_p, int *tty_enc_header_p,
            int *tty_enc_baud_rate_p, float *in_buf, int len);

int tty_dec(short *out, int qcode_b, int header, int chr, int baud_rate,
            int fer_flag, int sf_idx, int sf_count, int sf_len);

/* fer_sim is referenced from code/decode.c inside an
 * `if (trans_fname != NULL)` branch; with trans_fname == NULL the
 * branch is dead. Provide a prototype only. */
void fer_sim(int *rate);

#endif /* _QCELP_TTY_STUB_H_ */
