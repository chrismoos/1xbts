/**********************************************************************
Each of the companies; Qualcomm, and Lucent (hereinafter
referred to individually as "Source" or collectively as "Sources") do
hereby state:

To the extent to which the Source(s) may legally and freely do so, the
Source(s), upon submission of a Contribution, grant(s) a free,
irrevocable, non-exclusive, license to the Third Generation Partnership
Project 2 (3GPP2) and its Organizational Partners: ARIB, CCSA, TIA, TTA,
and TTC, under the Source's copyright or copyright license rights in the
Contribution, to, in whole or in part, copy, make derivative works,
perform, display and distribute the Contribution and derivative works
thereof consistent with 3GPP2's and each Organizational Partner's
policies and procedures, with the right to (i) sublicense the foregoing
rights consistent with 3GPP2's and each Organizational Partner's  policies
and procedures and (ii) copyright and sell, if applicable) in 3GPP2's name
or each Organizational Partner's name any 3GPP2 or transposed Publication
even though this Publication may contain the Contribution or a derivative
work thereof.  The Contribution shall disclose any known limitations on
the Source's rights to license as herein provided.

When a Contribution is submitted by the Source(s) to assist the
formulating groups of 3GPP2 or any of its Organizational Partners, it
is proposed to the Committee as a basis for discussion and is not to
be construed as a binding proposal on the Source(s).  The Source(s)
specifically reserve(s) the right to amend or modify the material
contained in the Contribution. Nothing contained in the Contribution
shall, except as herein expressly provided, be construed as conferring
by implication, estoppel or otherwise, any license or right under (i)
any existing or later issuing patent, whether or not the use of
information in the document necessarily employs an invention of any
existing or later issued patent, (ii) any copyright, (iii) any
trademark, or (iv) any other intellectual property right.

With respect to the Software necessary for the practice of any or
all Normative portions of the QCELP-13 Variable Rate Speech Codec as
it exists on the date of submittal of this form, should the QCELP-13 be
approved as a Specification or Report by 3GPP2, or as a transposed
Standard by any of the 3GPP2's Organizational Partners, the Source(s)
state(s) that a worldwide license to reproduce, use and distribute the
Software, the license rights to which are held by the Source(s), will
be made available to applicants under terms and conditions that are
reasonable and non-discriminatory, which may include monetary compensation,
and only to the extent necessary for the practice of any or all of the
Normative portions of the QCELP-13 or the field of use of practice of the
QCELP-13 Specification, Report, or Standard.  The statement contained above
is irrevocable and shall be binding upon the Source(s).  In the event
the rights of the Source(s) in and to copyright or copyright license
rights subject to such commitment are assigned or transferred, the
Source(s) shall notify the assignee or transferee of the existence of
such commitments.
*******************************************************************/
/* celp.h - basic structures for celp coder   */



#include"filter.h"
#include"lpc.h"
#include"coder.h"
#include <stdio.h>

#define USE_CALLOC      0
#define PREV_LSP_FIX    1

#define LPC_CENTER LPCOFFSET+(LPCSIZE)/2

typedef int INTTYPE;

struct PITCHPARAMS {
    int    min_lag;
    int    max_lag;
    float  b;
    int    lag;
    int    frac;
    int    qcode_b;
    int    qcode_lag;
    int    qcode_frac;
    int    last;
};

struct DECODER_MEM {
    int    type;   /* To tell whether this is the Tx or Rx decoder */
    int    last_mode;
    float  pred_qlsp[LPCORDER];
    float  last_qlsp[LPCORDER];
    float  lastG[GORDER];
   /* these pitch variables are needed for the pitch post filter */
    float  pre_factor;
    float pw_speech_out[FSIZE];
    struct PITCHPARAMS pitch_post_params;
    int    post_lag[8];
    float  post_pgain[8];
    int    index;
    struct POLE_FILTER       lpc_filt;
    struct POLE_FILTER       wght_syn_filt;
    struct POLE_FILTER       jt_gain_opt;
    struct POLE_FILTER_1_TAP pitch_filt;
    struct POLE_FILTER_1_TAP pitch_filt_per;
    struct POLE_FILTER       post_filt_p;
    struct ZERO_FILTER       post_filt_z;
    struct ZERO_FILTER       pitch_sm;
    struct ZERO_FILTER       bpf_unv;
    struct POLE_FILTER       bright_filt;
    float  agc_factor;
    int    low_rate_cnt;
    int    last_lag;
    float  last_b;
    float  last_G;
    int    last_frac;
    int    err_cnt;
    int    erasure_count;

    int G_pred[GPRED_ORDER];

};

struct UNV_FEATURES {  /* features used for unvoiced classification etc */
    float  rate[FREQBANDS];
    float  nacf0;
    float  nacf1;
    float  nacf_frame;
    int    open_pitch;
    int    zero;
    /*float  res_energy_diff0;
    float  res_energy_diff1;
    float  last_res_energy;*/
    float  last_nacf;
    float  pred_gain;
    float  pred_gain_mean,pred_gain_var;
    float  pred_gain_diff;
    float  pred_gain_frame;
    float  last_pred_gain;
    float  last_pred_gain_frame;
    float  frame_energy_db;
    float  frame_energy_last;
    float  voiced_energy;
    float last_lsp[LPCORDER];
    float diff_lsp, last_diff_lsp;
    float band_energy[FREQBANDS];
    float band_snr_inst[FREQBANDS];
    float band_snr_inst_last[FREQBANDS];
};
struct ENCODER_MEM {
    struct UNV_FEATURES      features;
    struct DECODER_MEM       dec;

    struct ZERO_FILTER       unq_form_res_filt;
    struct ZERO_FILTER       form_res_filt;
    struct POLE_FILTER       spch_wght_syn_filt;
    struct POLE_FILTER       target_syn_filter;
    struct POLE_FILTER       hipass_p;
    struct ZERO_FILTER       hipass_z;
    struct ZERO_FILTER       decimate;
    float  last_k[LPCORDER];
    float  last_lsp[LPCORDER];

    float  band_noise_sm[FREQBANDS];
    int    last_rate;   /* rate decisision after 2nd stage select_mode2() */
    int    last_rate_1st_stage; /* rate decision after 1st stage */
    int    num_full_frames;
    int    hangover, hangover_in_progress;
    int    band_rate[FREQBANDS];
    float  band_power[4];

    float  signal_energy[FREQBANDS];
    float  frame_energy_sm[FREQBANDS];
    float  resid_mem[PMAX];

    int    adaptcount;
    int    pitchrun;
    float  band_power_last[FREQBANDS];
    int    snr_stat_once;
    int    snr_map[FREQBANDS];
    float  snr[FREQBANDS];
    float  r_filt[FREQBANDS][FILTERORDER];

    float pitch_target_energy[MAXSF];
    float pitch_target_energy_after[MAXSF];
    float codebook_target_energy[MAXSF];
    float codebook_target_energy_after[MAXSF];
    float target_snr,        /* target_snr for frame */
          target_snr_thr;    /* target snr threshold */
    float target_snr_mean,
          target_snr_var;
    float avg_rate,   /* average rate over STATWINDOW active frames */
          avg_rate_thr;

    int frame_num;
    /* these are the new variables needed for rate control */
    int block_cnt,full_cnt,full_force,half_cnt,half_force;

    /* total counts */
    int block_cnt_t,full_cnt_t,full_force_t,half_cnt_t,half_force_t,
                     quarter_cnt_t,total_speech_frames;
    int quarter_cnt /*, hist_full[4],hist_half[4]*/;
    int hist_above[4], hist_below[4];

    float pitch_gain[MAXPITSF]; /* Changed from [MAXSF/MAXCBPERPIT];*/
    float pitch_lag[MAXPITSF];  /* Changed from [MAXSF/MAXCBPERPIT];*/
    float target_after_save[FSIZE];

    /* MODIFICATION: relocated from file-scope statics in encode.c /
     * ratedec.c into per-instance state. Initialised to 0 by
     * initialize_encoder_and_decoder(). */
    int hysteresis;
    int last_solid_voiced;

};


struct LPCPARAMS {
    float  k[LPCORDER];
    int    qcode_k[LPCORDER];
    float  lsp[LPCORDER];
    int    qcode_lsp[LPCORDER];
};


struct CBPARAMS {
    float  G;
    int    i;
    int    qcode_G;
    int    qcode_i;
    int    qcode_Gsign;
    int    sd;
    int    seed;
};

struct PACKET {
    int    mode;
    int    data[20];
    int    lpc[LPCORDER];
    float  lsp[LPCORDER];
    int    min_lag;
    int    b[MAXSF], lag[MAXSF], G[MAXSF][MAXNUMCB], i[MAXSF][MAXNUMCB];
    int    sd_dec, sd_enc;
    int    Gsign[MAXSF][MAXNUMCB], frac[MAXSF];
};

struct SNR {
    int    num_frames[2];
    float  signal_energy[2];
    float  noise_energy[2];
    float  seg_snr[2];
};


struct CONTROL {
    int    min_rate;
    int    max_rate;
    float    avg_rate;
    float    target_snr_thr;
    float    per_wght; /* perceputal weights for weighed LPC's to get target */
    int    ratestuff;
    int    print_packets;
    int    num_frames;
    int    pf_flag;
    int    pitch_out;
    int    cb_out;
    int    form_res_out;
    int    target_after_out;
    int    output_encoder_speech;
    int    verbose;
    int    print_file;
    int    reduced_rate_flag;
    int    unvoiced_off;
    int    pitch_post;
    float  interp_filt[MAX_FR_RESOLUTION][FR_INTERP_FILTER_LENGTH];
    struct SNR snr;

    int encode_only;
    int decode_only;
    int fractional_pitch;
    int skip_pitch_prefilter;

};


/***********************************************************************
*
* Function Prototypes
*
************************************************************************/

extern double log_10(double x);

extern void adjust_rate_down(
    struct ENCODER_MEM  *e_mem
);

extern void adjust_rate_up(
    struct ENCODER_MEM  *e_mem
);

extern void agc(
    float *input,
    float *output,
    float *return_vals,
    int   length,
    float *factor
);

extern void agc_prefilter(
    float *input,
    float *output,
    float *return_vals,
    int   length
);

extern void alloc_mem_for_speech(
    float **in_speech,
    float **out_speech
);

extern void band_energy_fcn(
    float               *R,
    float               *energy,
    struct ENCODER_MEM  *e_mem
);

extern void check_lsp_stab(float *qlsp);

extern void clear_packet_params(
     struct PACKET *packet
);

extern void create_target_speech(
    int    mode,
    float  *input,
    float  *resid,
    float  *target,
    float  lpc[2][MAXSF][2][LPCORDER],
    struct ZERO_FILTER *err_filt,
    struct POLE_FILTER *wght_filt
);

extern float compute_autocorr(
    float   *signal,
    int     length,
    int     shift
);

extern void compute_cb(
     int                    mode,
     float                  *target,
     struct ENCODER_MEM     *e_mem,
     float                  *lpc,
     struct CBPARAMS        cb_params[MAXCBPERPIT][MAXNUMCB],
     int                    cbsf
);

extern void compute_cb_gain(
     int                    mode,
     struct ENCODER_MEM     *e_mem,
     struct CBPARAMS        cb_params[MAXCBPERPIT][MAXNUMCB],
     float                  *speech,
     int                    cbsf
);

extern void compute_features(
    float                *pitch_cor,
    int                  *pitch,
    float                *resid,
    float                *speech,
    struct UNV_FEATURES  *unv_features,
    float                *acf,
    float                *lpc,
    float                *lsp,
    float                *band_energy,
     struct ENCODER_MEM     *e_mem
);

extern void compute_lpc(
    float            *speech,
    int              windowsize,
    int              windowtype,
    int              order,
    float            *lpc,
    float            *R,
    struct  CONTROL  *control
);

extern void compute_pitch(
    int                   mode,
    float                 *target,
    struct CONTROL        *control,
    struct ENCODER_MEM    *e_mem,
    float                 *lpc,
    struct PITCHPARAMS    *pitch_params
);

extern void compute_snr(
     struct SNR     *snr,
     struct CONTROL *control
);

extern void compute_sns(
    float *R,
    float *lpc,
    float *lsp,
    float *sns
);

extern void compute_target_snr(
    int                 mode,
    struct ENCODER_MEM  *e_mem
);

extern void convolve(
    float *in1,
    int   l1,
    float *in2,
    int   l2,
    float *out,
    int   *lout
);

extern void convolve_odd(
    float *in1,
    int   l1,
    float *in2,
    int   l2,
    float *out,
    int   *lout
);

extern void decoder(
    float *out_speech,
    struct PACKET      *packet,
    struct CONTROL     *control,
    struct DECODER_MEM *d_mem
);

extern void do_pole_filter(
    float                *input,
    float                *output,
    int                  numsamples,
    struct POLE_FILTER   *filter,
    int                  update_flag
);

extern void debug_do_pole_filter(
    float                *input,
    float                *output,
    int                  numsamples,
    struct POLE_FILTER   *filter,
    int                  update_flag
);

extern void do_zero_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
);

#if 0
extern void debug_do_zero_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
);
#endif

extern void do_fir_linear_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
);

extern void do_pole_filter_1_tap_interp(
    float                     *input,
    float                     *output,
    int                       numsamples,
    struct POLE_FILTER_1_TAP  *filter,
    int                       update_flag
);

extern void durbin(
    float *R,
    float *lpc,
    int   order
);

extern void encoder(
    float                   *in_buffer,
    struct  PACKET          *packet,
    struct  CONTROL         *control,
    struct  ENCODER_MEM     *e_mem,
    float                   *out_buffer
);

extern void free_encoder_and_decoder(
    struct  ENCODER_MEM *e_mem,
    struct  DECODER_MEM *d_mem
);

extern void free_pole_filter(
    struct  POLE_FILTER  *filter
);

extern void free_zero_filter(
    struct ZERO_FILTER  *filter
);

extern void front_end_filter(
    float               *speech,
    struct ENCODER_MEM  *e_mem
);

extern int G_erasure_check(
    struct CBPARAMS    cb_params[MAXCBPERPIT][MAXNUMCB]
);

extern void get_zero_input_response_pole(
    float               *response,
    int                 length,
    struct POLE_FILTER  *filter
);

extern void get_impulse_response_pole(
    float               *response,
    int                 length,
    struct POLE_FILTER  *filter
);

extern void get_zero_input_response_pole_1_tap_interp(
    float                     *response,
    int                       length,
    struct POLE_FILTER_1_TAP  *filter
);

extern int getbit(
    struct PACKET *packet,
    int           type,
    int           number,
    int           loc
);

extern float hammsinc(float input);

extern void HAMMINGwindow(
    float *input,
    float *output,
    int   length
);

extern void initialize_decoder(
    struct  DECODER_MEM *d_mem
);

extern void initialize_static_codebooks(void);

extern void initialize_encoder_and_decoder(
    struct  ENCODER_MEM *e_mem,
    struct  DECODER_MEM *d_mem,
    struct  CONTROL *control
);

extern void initialize_pole_1_tap_filter(
    struct POLE_FILTER_1_TAP  *filter,
    int                       max_order
);

extern void initialize_pole_filter(
    struct  POLE_FILTER     *filter,
    int                     order
);

#if 0
void initialize_pole_zero_filter();
#endif

extern void initial_recursive_conv(
    float *resid,
    int   length,
    float *impulse_response
);

extern void initialize_zero_filter(
    struct ZERO_FILTER  *filter,
    int                 order
);

extern void lin_quant(
    int   *qcode,
    float min,
    float max,
    int   num_levels,
    float input
);

extern void lin_unquant(
    float *output,
    float min,
    float max,
    int   num_levels,
    int   qcode
);

extern void lpc2lsp(
    float *lpc,
    float *lsp,
#if PREV_LSP_FIX
    float *prev_lsp,
#endif
    int   order
);

extern void lpc2pq(
    float *lpc,
    float *p,
    float *q,
    int   order
);

extern int lsp_erasure_check(
    int    mode,
    int    *qcode
);

extern void lsp2lpc(
    float *lsp,
    float *lpc,
    int   order
);

extern void lsp2pq(
    float *lsp,
    float *p,
    float *q,
    int   order
);

extern float lspdct(
    float *r,
    int   order,
    float indx
);

extern void interp_lpcs(
    int              mode,
    float            *prev_lsp,
    float            *curr_lsp,
    float            lpc[2][MAXSF][2][LPCORDER],
    int              type,
    struct  CONTROL  *control
);

extern void pack_cb(
    int             mode,
    struct CBPARAMS cb_params[MAXCBPERPIT][MAXNUMCB],
    struct PACKET   *packet,
    int             psf, /* pitch sub-frame */
    int             cbsf  /* codebook sub-frame */
);

extern void pack_pitch(
    struct PITCHPARAMS *pitch_params,
    struct PACKET      *packet,
    int                sf
);

extern void pack_lpc(
    int              mode,
    struct LPCPARAMS *lpc_params,
    struct PACKET    *packet
);

extern void pack_frame(
     int           mode,
     struct PACKET *packet
);

extern void postfilt(
    float               *out_buffer,
    float               *lpc,
    struct DECODER_MEM  *d_mem,
    int                 length
);

extern void pq2lpc(
    float *p,
    float *q,
    float *lpc,
    int   order
);

extern void pq2lsp(
    float *p,
    float *q,
    float *lsp,
#if PREV_LSP_FIX
    float *prev_lsp,
#endif
    int   order
);

extern void putbit(
    struct PACKET *packet,
    int           type,
    int           number,
    int           loc,
    int           bit
);

extern void quantize_b(
    float unq_b,
    float *q_b,
    int   *qcode_b
);

extern void quantize_lag(
    int   *lag,
    int   *qcode_lag,
    int   *frac,
    int   *qcode_frac
);

extern void quantize_lpc(
    int                 mode,
    float               lpc[LPCORDER],
    float               *lsp,
    float               *qlsp,
    float               *R,
    struct LPCPARAMS    *lpc_params,
    struct ENCODER_MEM  *e_mem
);

extern void quantize_min_lag(
    int  min_lag,
    int  *qcode_min_lag
);

extern void quantize_G(
    int   mode,
    float unq_G,
    float *q_G,
    int   *qcode_G,
    int   *qcode_Gsign,
    float *lastG, /* only needed for FULLRATE every fourth CB subframe */
    int   cbsf,
    int   *G_pred
);

extern void quantize_G_8th(
    float     unq_G,            /* input Gain value before quantization  */
    float     *q_G,             /* recontructed CB Gain */
    INTTYPE   *qcode_G,         /* quant. code for CB Gain */
    int       *G_pred           /* prediction for CB Gain */
);

extern void quantize_i(
    int   *i,
    int   *qcode_i
);

extern void recursive_conv(
    float *resid,
    float *impulse_response,
    int   length
);

float rnd1617();

extern float rsearch(
    float *r,
    int   sub_order,
    float lo,
    float hi,
    int   num_bins
);

extern void run_decoder(
    int                     mode,
    struct  DECODER_MEM     *d_mem,
    float                   lpc[2][LPCORDER],
    struct  PITCHPARAMS     *pitch_params,
    struct  CBPARAMS        cb_params[MAXCBPERPIT][MAXNUMCB],
    float                   *out_buffer,
    int                     length,
    struct  CONTROL         *control,
    int                     numcbsf,
    int                     numcb
);

extern void save_pitch(
     struct PITCHPARAMS  *pitch_params,
     float               *pitch_lag,
     float               *pitch_gain
);

extern void save_target(
     float *target,
     float *gt_target,
     int   length
);

extern void select_mode1(
    int                 *rate,
    float               *acf,
    struct CONTROL      *control,
    struct ENCODER_MEM  *e_mem
);

extern void select_mode2(
    int                 *rate,
    float               *form_resid,
    float               *acf,
    struct CONTROL      *control,
    struct ENCODER_MEM  *e_mem,
    float               *speech,
    float               *lpc,
    float               *lsp
);

extern void set_lag_range(
    struct PITCHPARAMS  *pitch_params,
    int                 *qcode_min_lag
);

extern float small_sinc(float val);

extern void target_reduction(
     float *after_search,
     float *before_search,
     float *energy_before,
     float *energy_after,
     int   length
);

extern int truefalse(
    int word,
    int bitloc
);

void unpack_cb();

extern void unpack_frame(
     struct PACKET *packet
);

extern void unpack_lpc(
    int              mode,
    struct LPCPARAMS *lpc_params,
    struct PACKET    *packet
);

extern void unpack_pitch(
     struct PITCHPARAMS *pitch_params,
     struct PACKET      *packet,
     int                sf
);

extern void unquantize_b(
    float *q_b,
    int   *qcode_b,
    int   lag
);

extern void unquantize_G(
    int   mode,
    float *q_G,
    int   *qcode_G,
    int   *qcode_Gsign,
    float *lastG,
    int   cbsf,
    int   *G_pred
);

extern void unquantize_G_8th(
    float     *q_G,          /* reconstructed G value */
    INTTYPE   *qcode_G,      /* quantization code for Gain */
    int       *G_pred        /* predictor */
);

extern void unquantize_i(
    int   *i,
    int   *qcode_i
);

extern void unquantize_lag(
    int   *lag,
    int   *qcode_lag,
    int   *frac,
    int   *qcode_frac
);

extern void unquantize_lpc(
    int                 mode,
    float               *qlsp,           /* output unquantized lsp */
    struct DECODER_MEM  *d_mem,
    struct LPCPARAMS    *lpc_params
);

extern void unquantize_lsp(
    int                 mode,
    float               *qlsp,          /* output unquantized lsp's */
    float               *last_qlsp,     /* input - unquantized last lsp */
    int                 *qcode,
    struct DECODER_MEM  *d_mem
);

void unquantize_min_lag();

extern void update_form_resid_mems(
    float               *input,
    struct ZERO_FILTER  *err_filt
);

extern void update_hist_cnt(
    struct ENCODER_MEM  *e_mem,
    int                 rate
);

extern void update_snr(
     int         type,
     float       *signal,
     float       *sig_and_noise,
     struct SNR  *snr
);

extern void update_target_cb(
    int                 mode,
    float               *pw_speech,
    float               *target,
    float               *lpc,
    struct PITCHPARAMS  *pitch_params,
    struct DECODER_MEM  *d_mem
);

extern void update_target_pitch(
    int                mode,
    float              *pw_speech,
    float              *target,
    float              *lpc,
    float              *targ_ene,
    struct DECODER_MEM *d_mem
);

extern void usage();


/*
void exit();

char *calloc();
char *malloc();

*/

/* Added on 5/7/96 */
int round_flt(float x);

extern void open_binary_input_file(
    FILE **fin,
    char *filename
);

extern void open_binary_output_file(
    FILE **fout,
    char *filename
);

extern int read_samples(
    FILE  *fin,
    float *inbuf,
    int   insamples
);

extern int write_samples(
    FILE  *fout,
    float *outbuf,
    int   outsamples
);

extern int read_packet(
    FILE  *fin,
    int   *inbuf,
    int   insamples
);

extern int write_packet(
    FILE  *fout,
    int   *outbuf,
    int   outsamples
);
