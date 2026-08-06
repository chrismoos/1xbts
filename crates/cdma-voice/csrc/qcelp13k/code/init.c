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
/* init.c - initialization of all coder stuff. defaults here.  */

#include <stdlib.h>
/* MODIFICATION: drop <malloc.h>, stdlib.h provides malloc()/free() on macOS. */
#include <math.h>
#include "celp.h"
#include "coderate.h"
#include "quantize.h"
#include "cb.h"

void initialize_decoder(
    struct  DECODER_MEM *d_mem
)
{
    int i;

    /* initialize the decoder filters */

    d_mem->type=DECODER;
    initialize_pole_1_tap_filter(&(d_mem->pitch_filt),
				 MAXLAG+FR_INTERP_FILTER_LENGTH/2+3);
    initialize_pole_1_tap_filter(&(d_mem->pitch_filt_per),
				 MAXLAG+FR_INTERP_FILTER_LENGTH/2+3);
    initialize_pole_filter(&(d_mem->lpc_filt), LPCORDER);
    initialize_pole_filter(&(d_mem->post_filt_p), LPCORDER);
    initialize_zero_filter(&(d_mem->post_filt_z), LPCORDER);
    initialize_zero_filter(&(d_mem->pitch_sm), PSMORDER);

    for (i=0; i<PSMORDER; i++) {
	d_mem->pitch_sm.zero_coeff[i] = hammsinc((float)(MAX_FR_RESOLUTION*
                   (i-PSMORDER/2+.5)));
    }
    initialize_pole_filter(&(d_mem->bright_filt), 1);
    d_mem->bright_filt.pole_coeff[0]= -BRIGHT_COEFF;
    for (i=0; i<LPCORDER; i++) {
	d_mem->last_qlsp[i]= (float)(i+1)/(float)(LPCORDER+1);
    }
    for (i=0; i<LPCORDER; i++) {
	d_mem->pred_qlsp[i]= 0.0;
    }

    d_mem->low_rate_cnt = 0;
    d_mem->last_G = 0.0;


    d_mem->agc_factor = 1.0;
    d_mem->pre_factor = 1.0;

    /*d_mem->pitch_post_params.lag = MAXLAG;*/
    /*d_mem->pitch_post_params.b = 0.0;*/

    d_mem->err_cnt = 0;
    d_mem->erasure_count = 0;
    d_mem->last_b = 0.0;
    d_mem->last_lag = 0;
    d_mem->last_frac = 0;

    d_mem->G_pred[0] = d_mem->G_pred[1] = 0;

} /* end initialize_decoder() */

void initialize_static_codebooks(void)
{
    int i;

#if !DOS
    /* Change LSP VQ Codebook from floating-point to fixed-point precision */
    for (i=0; i<LSPVQSIZE[0]; i++) {
        LSPVQ0[i][0] = round_flt(LSPVQ0[i][0]*16384)/16384.0;
        LSPVQ0[i][1] = round_flt(LSPVQ0[i][1]*16384)/16384.0;
    }
    for (i=0; i<LSPVQSIZE[1]; i++) {
        LSPVQ1[i][0] = round_flt(LSPVQ1[i][0]*16384)/16384.0;
        LSPVQ1[i][1] = round_flt(LSPVQ1[i][1]*16384)/16384.0;
    }
    for (i=0; i<LSPVQSIZE[2]; i++) {
        LSPVQ2[i][0] = round_flt(LSPVQ2[i][0]*16384)/16384.0;
        LSPVQ2[i][1] = round_flt(LSPVQ2[i][1]*16384)/16384.0;
    }
    for (i=0; i<LSPVQSIZE[3]; i++) {
        LSPVQ3[i][0] = round_flt(LSPVQ3[i][0]*16384)/16384.0;
        LSPVQ3[i][1] = round_flt(LSPVQ3[i][1]*16384)/16384.0;
    }
    for (i=0; i<LSPVQSIZE[4]; i++) {
        LSPVQ4[i][0] = round_flt(LSPVQ4[i][0]*16384)/16384.0;
        LSPVQ4[i][1] = round_flt(LSPVQ4[i][1]*16384)/16384.0;
    }

    /* Change circular codebook from floating-point to fixed-point precision */
    for (i=0; i<CBLENGTH; i++) {
        CODEBOOK_HALF[i] = round_flt(CODEBOOK_HALF[i]*8192)/8192.0;
        CODEBOOK[i] = round_flt(CODEBOOK[i]*8192)/8192.0;
    }
#endif
}

void initialize_encoder_and_decoder(
    struct  ENCODER_MEM *e_mem,
    struct  DECODER_MEM *d_mem,
    struct  CONTROL *control
)
{
    int i, j, k;

    /* initialize the decoder filters */

#if 1
    initialize_decoder( d_mem );
#else
    d_mem->type=DECODER;
    initialize_pole_1_tap_filter(&(d_mem->pitch_filt),
				 MAXLAG+FR_INTERP_FILTER_LENGTH/2+3);
    initialize_pole_1_tap_filter(&(d_mem->pitch_filt_per),
				 MAXLAG+FR_INTERP_FILTER_LENGTH/2+3);
    initialize_pole_filter(&(d_mem->lpc_filt), LPCORDER);
    initialize_pole_filter(&(d_mem->post_filt_p), LPCORDER);
    initialize_zero_filter(&(d_mem->post_filt_z), LPCORDER);
    initialize_zero_filter(&(d_mem->pitch_sm), PSMORDER);

    for (i=0; i<PSMORDER; i++) {
	d_mem->pitch_sm.zero_coeff[i] = hammsinc((float)(MAX_FR_RESOLUTION*
                   (i-PSMORDER/2+.5)));
    }
    initialize_pole_filter(&(d_mem->bright_filt), 1);
    d_mem->bright_filt.pole_coeff[0]= -BRIGHT_COEFF;
    for (i=0; i<LPCORDER; i++) {
	d_mem->last_qlsp[i]= (float)(i+1)/(float)(LPCORDER+1);
    }
    for (i=0; i<LPCORDER; i++) {
	d_mem->pred_qlsp[i]= 0.0;
    }

    d_mem->low_rate_cnt = 0;
    d_mem->last_G = 0.0;


    d_mem->agc_factor = 1.0;
    d_mem->pre_factor = 1.0;

    /*d_mem->pitch_post_params.lag = MAXLAG;*/
    /*d_mem->pitch_post_params.b = 0.0;*/

    d_mem->err_cnt = 0;
    d_mem->last_b = 0.0;
    d_mem->last_lag = 0;
    d_mem->last_frac = 0;

    d_mem->G_pred[0] = d_mem->G_pred[1] = 0;
#endif


    /* initialize the encoder filters */
    e_mem->dec.type=ENCODER;
    initialize_zero_filter(&(e_mem->hipass_z), 2);
    e_mem->hipass_z.zero_coeff[0]= -2.0;
    e_mem->hipass_z.zero_coeff[1]=  1.0;
    initialize_pole_filter(&(e_mem->hipass_p), 2);
    e_mem->hipass_p.pole_coeff[0]=  1.88;
    e_mem->hipass_p.pole_coeff[1]= -0.8836; /* -0.8836 = 0.94*0.94 */

    initialize_zero_filter(&(e_mem->decimate), DEC_ORDER);
    for (i=0; i<DEC_ORDER; i++) {
	e_mem->decimate.zero_coeff[i] = decimate_filter[i];
    }
    /* initialize bpf for unvoiced speech for enc and dec */
    initialize_zero_filter(&(e_mem->dec.bpf_unv), FIR_UNV_LEN);
    for (i=0; i<FIR_UNV_LEN; i++) {
	e_mem->dec.bpf_unv.zero_coeff[i] = unv_filter[i];
    }
    initialize_zero_filter(&(d_mem->bpf_unv), FIR_UNV_LEN);
    for (i=0; i<FIR_UNV_LEN; i++) {
	d_mem->bpf_unv.zero_coeff[i] = unv_filter[i];
    }
    initialize_zero_filter(&(e_mem->form_res_filt), LPCORDER);
    initialize_pole_filter(&(e_mem->spch_wght_syn_filt), LPCORDER);
    initialize_pole_filter(&(e_mem->dec.wght_syn_filt), LPCORDER);
    initialize_pole_1_tap_filter(&(e_mem->dec.pitch_filt),
				 MAXLAG+FR_INTERP_FILTER_LENGTH/2+3);
    initialize_pole_filter(&(e_mem->dec.lpc_filt), LPCORDER);
    initialize_pole_filter(&(e_mem->dec.jt_gain_opt), LPCORDER);
    initialize_pole_filter(&(e_mem->target_syn_filter), LPCORDER);
    initialize_zero_filter(&(e_mem->dec.pitch_sm), PSMORDER);

    e_mem->dec.low_rate_cnt = 0;
    for (i=0; i<PSMORDER; i++) {
	e_mem->dec.pitch_sm.zero_coeff[i] = hammsinc((float)(MAX_FR_RESOLUTION*
                   (i-PSMORDER/2+.5)));
    }
    e_mem->dec.last_G = 0.0;

    for (i=0; i<LPCORDER; i++) {
	e_mem->dec.last_qlsp[i]= (float)(i+1)/(float)(LPCORDER+1);
    }
    for (i=0; i<LPCORDER; i++) {
	e_mem->dec.pred_qlsp[i]= 0.0;
    }

    for (i=0; i<LPCORDER; i++) {
	e_mem->last_lsp[i]= (float)(i+1)/(float)(LPCORDER+1);
    }

    for (i=0; i<2; i++) {
	control->snr.num_frames[i]=0;
	control->snr.signal_energy[i]=0;
	control->snr.noise_energy[i]=0;
	control->snr.seg_snr[i]=0;
    }

    e_mem->last_rate = EIGHTH;
    e_mem->last_rate_1st_stage = EIGHTH;

    /* init for average rate calc and thresholds */

    e_mem->avg_rate_thr = control->avg_rate;
    e_mem->avg_rate = e_mem->avg_rate_thr;


    /* start the target snr threshold at 10 db */
    e_mem->target_snr_thr = control->target_snr_thr;

    e_mem->target_snr = 1.0;

    e_mem->target_snr_var = 1.0;
    e_mem->target_snr_mean = control->target_snr_thr;
    e_mem->features.pred_gain_mean = control->target_snr_thr;
    for(i =0; i < FREQBANDS; i++){
      e_mem->features.band_snr_inst_last[i] = 0.0;
    }

    for (i=0; i<LPCORDER; i++) {
      e_mem->features.last_lsp[i]= (float)(i+1)/(float)(LPCORDER+1);
    }
    e_mem->features.last_nacf = 0.0;
    e_mem->features.last_diff_lsp = 0.0;
    e_mem->features.last_pred_gain = 0.0;
    e_mem->features.pred_gain_mean = 0.0;
    e_mem->features.pred_gain_var = 0.0;

    e_mem->block_cnt = STATWINDOW;
    e_mem->full_force = e_mem->full_cnt =  e_mem->full_cnt_t = e_mem->full_force_t =
    e_mem->half_force = e_mem->half_cnt = e_mem->half_cnt_t = e_mem->quarter_cnt = 0;
    e_mem->quarter_cnt_t = 0;
    e_mem->total_speech_frames = 0;
    e_mem->half_force_t = 0;
    for(i=0;i< 4;i++)
      e_mem->hist_above[i] = e_mem->hist_below[i] = 0;

    /* rate decision initialization */
    initialize_zero_filter(&(e_mem->unq_form_res_filt), LPCORDER);
    e_mem->num_full_frames=0;
    e_mem->hangover=10;
    e_mem->hangover_in_progress=0;

    for(i = 0; i < PMAX; i++)
      e_mem->resid_mem[i] = 0.0;
    e_mem->adaptcount=0;
    e_mem->pitchrun=0;

    e_mem->frame_energy_sm[0] = 3200000;
    e_mem->frame_energy_sm[1] = 320000;

    for (i=0; i<FREQBANDS; i++) {
      e_mem->band_power_last[i] = 0.0;
    }

   e_mem->signal_energy[0] = 3200000;
   e_mem->signal_energy[1] = 320000;


    e_mem->snr_stat_once=0;

    e_mem->dec.G_pred[0] = e_mem->dec.G_pred[1] = 0;

    /* MODIFICATION: state migrated from file-scope statics. */
    e_mem->hysteresis = 0;
    e_mem->last_solid_voiced = 0;

    for (j=0; j<FREQBANDS; j++)
    {
        e_mem->band_noise_sm[j]= HIGH_THRESH_LIM;

        for (i=0; i<FILTERORDER; i++)
        {
            e_mem->r_filt[j][i]=0;
            for (k=0; k<FILTERORDER-i; k++)
            {
                e_mem->r_filt[j][i]+=rate_filt[j][k]*rate_filt[j][k+i];
            }
        }
    }
    for (j=0; j<FREQBANDS; j++)
    {
      e_mem->snr[j] = e_mem->signal_energy[j]/e_mem->band_noise_sm[j];
      e_mem->snr_map[j] = 0.0;
    }

}/* end of initialize_encoder_and_decoder() */

void free_encoder_and_decoder(
    struct  ENCODER_MEM *e_mem,
    struct  DECODER_MEM *d_mem
)
{
    free((char*) d_mem->pitch_filt.memory);
    free((char*) d_mem->pitch_filt_per.memory);

    free_pole_filter(&(d_mem->lpc_filt));
    free_pole_filter(&(d_mem->post_filt_p));
    free_zero_filter(&(d_mem->post_filt_z));
    free_zero_filter(&(d_mem->pitch_sm));

    free_pole_filter(&(d_mem->bright_filt));

    free_zero_filter(&(e_mem->hipass_z));
    free_zero_filter(&(e_mem->dec.bpf_unv));
    free_zero_filter(&(d_mem->bpf_unv));

    free_pole_filter(&(e_mem->hipass_p));

    free_zero_filter(&(e_mem->decimate));

    free_zero_filter(&(e_mem->form_res_filt));
    free_pole_filter(&(e_mem->spch_wght_syn_filt));
    free_pole_filter(&(e_mem->dec.wght_syn_filt));
    free((char *) e_mem->dec.pitch_filt.memory);
    free_pole_filter(&(e_mem->dec.lpc_filt));
    free_pole_filter(&(e_mem->dec.jt_gain_opt));
    free_pole_filter(&(e_mem->target_syn_filter));
    free_zero_filter(&(e_mem->dec.pitch_sm));

    /* free rate decision */
    free_zero_filter(&(e_mem->unq_form_res_filt));

}

void alloc_mem_for_speech(
    float **in_speech,
    float **out_speech
)
{
    int     i;
    float   *ptr;

  if( (*in_speech = (float *)
       calloc((unsigned) (FSIZE+LPCSIZE-FSIZE+LPCOFFSET),
				    (unsigned) sizeof(float))) == NULL){
    fprintf(stderr,"Error Allocating memory for in_speech\n");
    exit(-2);
  }

  if ((*out_speech = (float *)
       calloc((unsigned) FSIZE, sizeof(float))) == NULL){
    fprintf(stderr,"Error Allocating memory for out_speech\n");
    exit(-2);
  }

  ptr = *in_speech;
  for( i=0 ; i < LPCSIZE+LPCOFFSET ; i++ )
  {
    ptr[i] = 0.0;
  }

  ptr = *out_speech;
  for( i=0 ; i < FSIZE ; i++ )
  {
    ptr[i] = 0.0;
  }
}
