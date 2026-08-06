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
/* decode.c - main CELP decoder */

#include <stdio.h>
#include <math.h>

#include "celp.h"
#include "quantize.h"
#include "cb.h"
#include "tty.h"
/* #include <sys/limits.h> */

/* for ERASURE processing */
/* MODIFICATION: const-qualified (read-only tables) */
static const float ERA_LSP_DEC[4]={1.0, 0.9, 0.9, 0.7};
static const float ERA_B_SAT[4]={0.8984375, 0.6015625, 0.30078125, 0.0}; /* Q8 */

void agc(
    float *input,
    float *output,
    float *return_vals,
    int   length,
    float *factor
)
{
  int i;
  float input_energy, output_energy;
  float norm;

  input_energy=0;
  output_energy=0;
  for (i=0; i<length; i++) {
    input_energy+=input[i]*input[i];
    output_energy+=output[i]*output[i];
  }

  if ((input_energy!=0)&&(output_energy!=0)) {
    norm=sqrt((double)(input_energy/output_energy));
    *factor=AGC_FACTOR*(*factor)+(1-AGC_FACTOR)*norm;
  }

  for (i=0; i<length; i++) {
    return_vals[i]=output[i]*(*factor);
  }
}/* end of agc() */

void agc_prefilter(
    float *input,
    float *output,
    float *return_vals,
    int   length
)
{
  int i;
  float input_energy, output_energy;
  register float norm;

  input_energy=0;
  output_energy=0;
  for (i=0; i<length; i++) {
    input_energy+=input[i]*input[i];
    output_energy+=output[i]*output[i];
  }

  if ((input_energy!=0)&&(output_energy!=0)) {
    norm=sqrt((double)(input_energy/output_energy));
  }

  for (i=0; i<length; i++) {
    return_vals[i]=output[i]*norm;
  }
}/* end of agc_prefilter() */


void run_decoder(
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
)
{
  int   j, k, l;

  float cb_out[FSIZE], pitch_out[FSIZE];
  float white_out[FSIZE];
  float pitch_pre_out[FSIZE];
  short stmp;

  for (k=0; k<numcbsf; k++) {
    for (j=0; j<length/numcbsf; j++) {
      cb_out[j+k*length/numcbsf]=0;
    }

    if (mode!=QUARTERRATE_UNVOICED && mode != EIGHTH) {
      for (l=0; l<numcb; l++) {

	if(mode == FULLRATE_VOICED || mode == BLANK){
	  for (j=0; j<length/numcbsf; j++) {
	    cb_out[j+k*length/numcbsf]+=
	      CODEBOOK[(CBLENGTH-cb_params[k][l].i +j)
	      %CBLENGTH]*cb_params[k][l].G;
	  }
	}
	else{/* HALFRATE_VOICED */
	  for (j=0; j<length/numcbsf; j++) {
	    cb_out[j+k*length/numcbsf]+=
	      CODEBOOK_HALF[(CBLENGTH-cb_params[k][l].i +j)
			    %CBLENGTH]*cb_params[k][l].G;
	  }
	}
      }
    }
    else if(mode == EIGHTH) {/* EIGHTH rate */
      for (j=0; j<length/numcbsf; j++) {
	cb_params[0][0].sd=(521*cb_params[0][0].sd+259)
	  &(0xffff);
	stmp = (((cb_params[0][0].sd+0x7fff)&(0xffff))-0x7fff);
	cb_out[j+k*length/numcbsf]=stmp;
	cb_out[j+k*length/numcbsf]= 0.5*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]= 1.375*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]= cb_params[0][0].G*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]/=QUANT14BIT;
      }
    }
    else if (mode == QUARTERRATE_UNVOICED){  /* QUARTERRATE_UNVOICED */
      for (j=0; j<length/numcbsf; j++) {
	cb_params[0][0].sd=(521*cb_params[0][0].sd+259)
	  &(0xffff);
	stmp = (((cb_params[0][0].sd+0x7fff)&(0xffff))-0x7fff);
	white_out[j+k*length/numcbsf]=stmp;
      }
      do_fir_linear_filter(&white_out[k*length/numcbsf],
			   &cb_out[k*length/numcbsf],
			   length/numcbsf, &(d_mem->bpf_unv), UPDATE) ;

      for (j=0; j<length/numcbsf; j++) {
	cb_out[j+k*length/numcbsf]= 0.5*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]= 1.375*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]= cb_params[k][0].G*cb_out[j+k*length/numcbsf];
	cb_out[j+k*length/numcbsf]/=QUANT14BIT;
      }
    }
    else{
      fprintf(stderr, "run_decoder(): Unsupported mode = %d\n", mode);
      exit(-2);
    }
  }


  d_mem->pitch_filt.delay=pitch_params->lag;
  d_mem->pitch_filt.coeff=pitch_params->b;
  d_mem->pitch_filt.frac=pitch_params->frac;

  do_pole_filter_1_tap_interp(cb_out, pitch_out, length,
		   &(d_mem->pitch_filt), UPDATE);

  if (d_mem->type==ENCODER) {
    /* generate unweighted speech, if we want the encoder's */
    /* synthesized speech */
    for (j=0; j<LPCORDER; j++) {
      d_mem->lpc_filt.pole_coeff[j]=lpc[NOT_WGHTED][j];
    }

    for (j=0; j<LPCORDER; j++) {
      d_mem->wght_syn_filt.pole_coeff[j]=lpc[WGHTED][j];
    }

    debug_do_pole_filter(pitch_out, out_buffer, length,
	       &(d_mem->lpc_filt), UPDATE);

    do_pole_filter(pitch_out, d_mem->pw_speech_out, length,
	       &(d_mem->wght_syn_filt), UPDATE);
  }

  else {/* DECODER */
    /* increase the periodicity of the excitation   */
    /* saturate pitch gain for pitch pre-filter */
    if(pitch_params->b > 1.0)d_mem->pitch_post_params.b = 1.0;
    else  d_mem->pitch_post_params.b = pitch_params->b;

    if(mode == HALFRATE_VOICED)d_mem->pitch_post_params.b *= HALFG;
    else d_mem->pitch_post_params.b *= FULLG;

    d_mem->pitch_post_params.lag = pitch_params->lag;
    d_mem->pitch_post_params.frac = pitch_params->frac;

    d_mem->pitch_filt_per.delay=d_mem->pitch_post_params.lag;
    d_mem->pitch_filt_per.coeff=d_mem->pitch_post_params.b;
    d_mem->pitch_filt_per.frac=d_mem->pitch_post_params.frac;


    /* Pitch Prefilter */
    /*****************************************************************/
    /* Skip this out when comparing encoder output with decoder output */
    /*****************************************************************/
    if ( !(control->pitch_out==YES || control->pitch_post==NO) ) {
      do_pole_filter_1_tap_interp(pitch_out, pitch_pre_out, length,
				  &(d_mem->pitch_filt_per), UPDATE);

      agc_prefilter(pitch_out, pitch_pre_out, pitch_out, length);

    }
    /******************************************************************/

    for (j=0; j<LPCORDER; j++) {
      d_mem->lpc_filt.pole_coeff[j]=lpc[NOT_WGHTED][j];
    }
    do_pole_filter(pitch_out, out_buffer, length,
		   &(d_mem->lpc_filt),UPDATE);

    if (control->pf_flag==PF_ON) {
      postfilt(out_buffer,&lpc[NOT_WGHTED][0],d_mem,length);

    }

  }

  if (control->cb_out==YES) {
    for (j=0; j<length; j++) {
      out_buffer[j]=cb_out[j];
    }
  }

  if (control->pitch_out==YES) {
    for (j=0; j<length; j++)
      out_buffer[j]=pitch_out[j];
  }

}/* end of run_decoder() */

/***************************************************************
 *  G_erasure_check()
 *  Returns 0 if no erasure
 *  Returns 1 if frame needs to be erased
 ****************************************************************/

int G_erasure_check(
    struct CBPARAMS    cb_params[MAXCBPERPIT][MAXNUMCB]
)
{
  float Gdiff;
  int j, i;

  for(i = 0, j = 0; i < 4; i++){
    Gdiff = fabs((double) cb_params[i+1][j].qcode_G - cb_params[i][j].qcode_G);
    if(fabs((double) cb_params[i+1][j].qcode_G - cb_params[i][j].qcode_G) > 10)
      return(1);
  }
  for(i = 0; i < 3; i++){
    Gdiff = fabs((double) cb_params[i+2][j].qcode_G - 2*cb_params[i+1][j].qcode_G + cb_params[i][j].qcode_G);
    if(fabs((double) cb_params[i+2][j].qcode_G - 2*cb_params[i+1][j].qcode_G + cb_params[i][j].qcode_G) > 12)
      return(1);
    }
  return(0);
}/* end of G_erasure_check() */

void decoder(
    float *out_speech,
    struct PACKET      *packet,
    struct CONTROL     *control,
    struct DECODER_MEM *d_mem
)
{
  int    i,j,k;
  int    mode;
  float  qlsp[LPCORDER];
  float  lpc[2][MAXSF][2][LPCORDER];
  float  wght_factor;
  struct PITCHPARAMS pitch_params;
  struct CBPARAMS    cb_params[MAXCBPERPIT][MAXNUMCB];
  struct LPCPARAMS   lpc_params;
  float  last_G, Gtmp[5];
  float  lsp_spread_factor;
  short  tty_outbuf[FSIZE];     /* Temp output buffer for TTY_NO_GAIN */

  extern void   fer_sim(int *rate);
  extern char *const trans_fname;

  if( trans_fname != NULL )
  {
      fer_sim( &(packet->data[0]) );
  }

  unpack_frame(packet);

  mode=packet->mode;
  if( tty_option == TTY_NO_GAIN )
  {

    for (i=0; i<PITCHSF[mode]; i++)
    {
      if( mode == ERASURE || mode == BLANK )
      {
          j = 1;                            /* use as FER flag for TTY */
          tty_dec_header = TTY_FER;
          tty_dec_char = 0;
      }
      else if( mode == EIGHTH || mode == QUARTERRATE_UNVOICED )
      {
          j = 0;                            /* use as FER flag for TTY */
          tty_dec_header = TTY_EIGHTH_RATE;
          tty_dec_char = 0;
          pitch_params.qcode_b = 1;         /* make 1/8 rate look like NON_TTY */
      }
      else
      {
          j = 0;                            /* use as FER flag for TTY */
          k = 0;
          unpack_pitch(&pitch_params, packet, i);

          /* TTY information is in 1st subframe only */
          if( i == 0 )
          {
              tty_dec_baud_rate = pitch_params.qcode_frac;
              tty_dec_header = (pitch_params.qcode_lag >> 5) & 0x03;
              tty_dec_char = pitch_params.qcode_lag & 0x1F;
          }
          else if( pitch_params.qcode_lag != 0 )
          {
              pitch_params.qcode_b = 1;
          }
      }

      tty_dec_flag = tty_dec(
          &tty_outbuf[i*FSIZE/PITCHSF[mode]],
          pitch_params.qcode_b,
          tty_dec_header,
          tty_dec_char,
          tty_dec_baud_rate,
          j,
          i,
          PITCHSF[mode],
          FSIZE/PITCHSF[mode] );


      if( tty_dec_flag )
      {
          /* Convert from short to float */
          for( j=0 ; j < FSIZE/PITCHSF[mode] ; j++ )
          {
              out_speech[j+i*FSIZE/PITCHSF[mode]] =
                  (float) tty_outbuf[j+i*FSIZE/PITCHSF[mode]]/4.0;
          }
      }

    } /* end for(i) subframe loop */

    if( tty_dec_flag )
    {
        initialize_decoder(d_mem);
        return;
    }

  } /* end if( TTY_NO_GAIN ) */

  if (mode==EIGHTH) {
    unpack_lpc(mode, &lpc_params, packet);
    unquantize_lpc(mode, qlsp, d_mem, &lpc_params);

    interp_lpcs(mode, d_mem->last_qlsp, qlsp, lpc, ONLY_UNWGHTED, control);

    /* no need for unpack_pitch() */
    pitch_params.b=0;
    pitch_params.lag=0;
    pitch_params.frac=0;

    unpack_cb(mode, packet, cb_params, 0, 0);
    unquantize_G_8th(&(cb_params[0][0].G),
		 &(cb_params[0][0].qcode_G),
		 d_mem->G_pred);

    cb_params[0][0].G=fabs(cb_params[0][0].G);

    cb_params[0][0].sd=packet->sd_dec;
    last_G= (0.5*(cb_params[0][0].G+d_mem->last_G));

    for (i=0; i<PITCHSF8TH; i++) {
      cb_params[0][0].G=d_mem->last_G*(PITCHSF8TH-1-i)/PITCHSF8TH
	+last_G*(i+1.0)/PITCHSF8TH;

      run_decoder(mode, d_mem, lpc[PITCH][0],
		  &pitch_params, cb_params,
		  &out_speech[i*FSIZE/PITCHSF8TH], FSIZE/PITCHSF8TH,
		  control, 1, 1);
    }
    cb_params[0][0].seed = cb_params[0][0].sd; /* for erasure processing */

    d_mem->last_G=cb_params[0][0].G;

    d_mem->low_rate_cnt+=1;
    d_mem->err_cnt=0;
  }
  else if (mode == QUARTERRATE_UNVOICED){

    unpack_lpc(mode, &lpc_params, packet);

    if(lsp_erasure_check(mode, lpc_params.qcode_lsp)){
      d_mem->erasure_count++;
      packet->mode = mode = ERASURE;
      goto erasure;
    }

    for (i=0, j = 0; i<CBSF[mode]; i++)
      unpack_cb(mode, packet, cb_params, j, i);

    if(G_erasure_check(cb_params)){
      d_mem->erasure_count++;
      packet->mode = mode = ERASURE;
      goto erasure;
    }

    unquantize_lpc(mode, qlsp, d_mem, &lpc_params);

    interp_lpcs(mode, d_mem->last_qlsp, qlsp,
		lpc, ONLY_UNWGHTED, control);

    pitch_params.b=0;
    pitch_params.lag=0;
    pitch_params.frac=0;

    j = 0;
    for (i=0; i<CBSF[mode]; i++) {
      unpack_cb(mode, packet, cb_params, j, i);
      unquantize_G(mode, &(cb_params[i][j].G),
		   &(cb_params[i][j].qcode_G),
		   &(cb_params[i][j].qcode_Gsign),
		   d_mem->lastG, j, d_mem->G_pred);
      Gtmp[i] = cb_params[i][j].G;
      if(Gtmp[i] < 0.0){
	fprintf(stderr,"QUARTERRATE Gain is negative: Gtmp[%d] = %f\n",
		i, Gtmp[i]);
	exit(-2);
      }
    }


    cb_params[0][0].sd = ((packet->data[2] & 0xffc ) << 4) |
                               ((packet->data[1] & 0x1f8) >> 3);

    /* produce 8 interpolated CB Gains from 5 CB Gains */
    for(i = 0; i < PITCHSF[mode]; i++){
      switch(i){
      case 0:
	cb_params[0][0].G = Gtmp[0];
	cb_params[1][0].G = 0.6 *Gtmp[0] + 0.4 * Gtmp[1];
	break;
      case 1:
	cb_params[0][0].G = Gtmp[1];
	cb_params[1][0].G = 0.2 *Gtmp[1] + 0.8 * Gtmp[2];
	break;
      case 2:
	cb_params[0][0].G = 0.8 *Gtmp[2] + 0.2 * Gtmp[3];
	cb_params[1][0].G = Gtmp[3];
	break;
      case 3:
	cb_params[0][0].G = 0.4 *Gtmp[3] + 0.6 * Gtmp[4];
	cb_params[1][0].G = Gtmp[4];
	break;
      }
      run_decoder(mode, d_mem, lpc[PITCH][i], &pitch_params,
		  cb_params, &out_speech[i*FSIZE/PITCHSF[mode]],
		  FSIZE/PITCHSF[mode], control, 2,
		  NUMCB[mode]);
    }
    cb_params[0][0].seed = cb_params[0][0].sd;
    d_mem->last_G=fabs(Gtmp[4]);
    d_mem->low_rate_cnt=0;
    d_mem->err_cnt=0;
  }/* end of QUARTERRATE_UNVOICED */

  else if(mode == BLANK){

    d_mem->last_G=cb_params[0][0].G=0;
    cb_params[0][0].i=0;  /* arbitrary */
    pitch_params.b  =d_mem->last_b;
    pitch_params.frac=d_mem->last_frac;

    if (pitch_params.b>1.0) {
      pitch_params.b=1.0;
    }
    pitch_params.lag=d_mem->last_lag;

    for (i=0; i<LPCORDER; i++) {
      qlsp[i]=d_mem->last_qlsp[i];
    }

    lsp2lpc(qlsp, lpc[PITCH][0][NOT_WGHTED], LPCORDER);
    wght_factor=1;
    for (j=0; j<LPCORDER; j++) {
      wght_factor*=BWE_FACTOR;
      lpc[PITCH][0][NOT_WGHTED][j]*=wght_factor;
    }

    run_decoder(mode, d_mem, lpc[PITCH][0], &pitch_params, cb_params,
		out_speech, FSIZE, control, 1, 1);

  }
  else if(mode == FULLRATE_VOICED || mode == HALFRATE_VOICED){/* FULLRATE_VOICED or HALFRATE_VOICED */
    unpack_lpc(mode, &lpc_params, packet);

    if(lsp_erasure_check(mode, lpc_params.qcode_lsp)){
      d_mem->erasure_count++;
      packet->mode = mode = ERASURE;
      goto erasure;
    }

    unquantize_lpc(mode, qlsp, d_mem, &lpc_params);
    interp_lpcs(mode, d_mem->last_qlsp, qlsp,
		lpc, ONLY_UNWGHTED, control);

    unquantize_min_lag(&(pitch_params.min_lag), packet->min_lag);

    cb_params[0][0].sd=packet->sd_dec;
    for (i=0; i<PITCHSF[mode]; i++) {

      unpack_pitch(&pitch_params, packet, i);
      unquantize_lag(&(pitch_params.lag), &(pitch_params.qcode_lag),
		     &(pitch_params.frac),
		     &(pitch_params.qcode_frac));
      unquantize_b(&(pitch_params.b), &(pitch_params.qcode_b),
		   pitch_params.qcode_lag);

      for (j=0; j<CBSF[mode]/PITCHSF[mode]; j++) {
	unpack_cb(mode, packet, cb_params, i, j);

	for (k=0; k<NUMCB[mode]; k++) {

	  unquantize_G(mode, &(cb_params[j][k].G),
		       &(cb_params[j][k].qcode_G),
		       &(cb_params[j][k].qcode_Gsign),
		       d_mem->lastG, j, d_mem->G_pred);

	  unquantize_i(&(cb_params[j][k].i),
		       &(cb_params[j][k].qcode_i));
	}
      }
      run_decoder(mode, d_mem, lpc[PITCH][i], &pitch_params,
		  cb_params, &out_speech[i*FSIZE/PITCHSF[mode]],
		  FSIZE/PITCHSF[mode], control, CBSF[mode]/PITCHSF[mode],
		  NUMCB[mode]);
      d_mem->last_G=fabs(cb_params[CBSF[mode]/PITCHSF[mode]-1][0].G);
      d_mem->low_rate_cnt=0;
      d_mem->err_cnt=0;
    }
  }

erasure:
  if (mode== ERASURE) {    /* ERASURES  */
    /* reconstruct decayed LSP's and smooth */
    for (i=0; i<LPCORDER; i++) {
      qlsp[i]=d_mem->pred_qlsp[i]*ERA_LSP_DEC[d_mem->err_cnt];
      d_mem->pred_qlsp[i] = qlsp[i];
      qlsp[i] = qlsp[i] * 0.90625 ;
      /* add in bias */
      qlsp[i]+=  (float)(i+1)/(float)(LPCORDER+1);
    }

    /* Enforce LSP separation */
    lsp_spread_factor = LSP_SPREAD_FACTOR;

    if (qlsp[0]<lsp_spread_factor) qlsp[0]=lsp_spread_factor;
    for (i=1; i<LPCORDER; i++) {
      if (qlsp[i]-qlsp[i-1]<lsp_spread_factor) {
	qlsp[i]=qlsp[i-1]+lsp_spread_factor;
      }
    }
    if (qlsp[LPCORDER-1]>1.0-lsp_spread_factor) {
      qlsp[LPCORDER-1]=1.0-lsp_spread_factor;
    }
    for (i=LPCORDER-2; i>=0; i--) {
      if (qlsp[i+1]-qlsp[i]<lsp_spread_factor) {
	qlsp[i]=qlsp[i+1]-lsp_spread_factor;
      }
    }

    /* Smooth LSP's */
    for (i=0; i<LPCORDER; i++) {
      qlsp[i] = 0.875 * d_mem->last_qlsp[i] + 0.125 * qlsp[i];
    }

    lsp2lpc(qlsp, lpc[PITCH][0][NOT_WGHTED], LPCORDER);

    /* Bandwidth Expansion */
    wght_factor=1;
    for (j=0; j<LPCORDER; j++) {
      wght_factor*=BWE_FACTOR;
      lpc[PITCH][0][NOT_WGHTED][j]*=wght_factor;
    }

    cb_params[0][0].seed=((521*(cb_params[0][0].seed)+259)&0xffff);
    cb_params[0][0].i = cb_params[0][0].seed&0x7f;
    pitch_params.lag=d_mem->last_lag;
    pitch_params.frac=d_mem->last_frac;

    pitch_params.b=d_mem->last_b;
    if (pitch_params.b>ERA_B_SAT[d_mem->err_cnt]) {
      pitch_params.b=ERA_B_SAT[d_mem->err_cnt];
    }

    for (i=GPRED_ORDER-1; i>0; i--) {
      d_mem->G_pred[i]=d_mem->G_pred[i-1];
    }

    if(!(d_mem->last_mode == EIGHTH && d_mem->G_pred[0] < 14))
    {
      if(d_mem->err_cnt < 3)
        d_mem->G_pred[0] -= d_mem->err_cnt;
      else
        d_mem->G_pred[0] -= 2*d_mem->err_cnt;/* -= 6 */
    }
    if(d_mem->G_pred[0] < 0)
      d_mem->G_pred[0] = 0;

    cb_params[0][0].G = GA[d_mem->G_pred[0] + 6];

    last_G=0.5*(cb_params[0][0].G+d_mem->last_G);

    for (i=0; i<PITCHSF_ERASURE; i++) {
      cb_params[i][0].G=d_mem->last_G*(PITCHSF_ERASURE-1-i)/PITCHSF_ERASURE
	+last_G*(i+1.0)/PITCHSF_ERASURE;

      run_decoder(FULLRATE_VOICED, d_mem, lpc[PITCH][0],
		  &pitch_params, cb_params,
		  &out_speech[i*FSIZE/PITCHSF_ERASURE], FSIZE/PITCHSF_ERASURE,
		  control, 1, 1);
      cb_params[0][0].i = (cb_params[0][0].i - 40) & 0x7f;
    }
    d_mem->last_G=cb_params[0][0].G;

    d_mem->err_cnt+=1;
    if (d_mem->err_cnt>3) {
      d_mem->err_cnt=3;
    }
  }

  /* save for erasure processing */
  d_mem->last_b=pitch_params.b;
  d_mem->last_lag=pitch_params.lag;
  d_mem->last_frac=pitch_params.frac;
  d_mem->last_mode = mode;
  for (i=0; i<LPCORDER; i++) {
    d_mem->last_qlsp[i]=qlsp[i];
  }

}/* end of decoder() */
