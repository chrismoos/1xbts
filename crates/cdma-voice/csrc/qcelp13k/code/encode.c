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
/* encode.c - main CELP encoder */

#include <stdio.h>
#include <math.h>
#include "celp.h"

#include "tty.h"
#include "tty_dbg.h"


void encoder(
    float                   *in_buffer,
    struct  PACKET          *packet,
    struct  CONTROL         *control,
    struct  ENCODER_MEM     *e_mem,
    float                   *out_buffer
)
{
  /* MODIFICATION: hysteresis lives on ENCODER_MEM (was function-local static). */
  int    i,j,index;
  float  lpc[LPCORDER], interp_lpc[2][MAXSF][2][LPCORDER];
  float  lsp[LPCORDER];
  float  qlsp[LPCORDER];
  struct LPCPARAMS   lpc_params;
  struct PITCHPARAMS pitch_params;
  struct CBPARAMS    cb_params[MAXCBPERPIT][MAXNUMCB];
  int    sf_pointer;
  float  Rs[FILTERORDER+1];
  int    mode;
  float  form_resid[FSIZE], target[FSIZE], pw_speech[FSIZE];
  float  before_search[FSIZE];
  float  target_after_save[FSIZE];
  float  wght_factor, targ_ene[MAXSF];
  float  last_G, form_ene, Gtmp[5];

  clear_packet_params(packet);
  pitch_params.frac = 0;

  front_end_filter(&in_buffer[LPCOFFSET],e_mem);

  if( tty_option == TTY_NO_GAIN )
  {
      tty_enc_flag = tty_enc( &tty_enc_char,
                              &tty_enc_header,
                              &tty_enc_baud_rate,
                              &in_buffer[LPCOFFSET],
                              FSIZE );
  }

  compute_lpc(&in_buffer[LPCOFFSET], LPCSIZE, HAMMING, LPCORDER,
			lpc, Rs, control);

#if PREV_LSP_FIX
  lpc2lsp(lpc, lsp, e_mem->features.last_lsp, LPCORDER);
#else
  lpc2lsp(lpc, lsp, LPCORDER);
#endif

  set_lag_range(&pitch_params, &(packet->min_lag));

  select_mode1(&mode, Rs, control, e_mem);

  /* Force the rate during TTY transmissions */
  if( tty_option == TTY_NO_GAIN && tty_enc_flag )
  {
     if( tty_enc_flag != TTY_SILENCE )
     {
        mode = FULLRATE_VOICED;
        e_mem->last_rate_1st_stage = mode;
        e_mem->last_rate = mode;

     }
  }

  packet->mode=mode;


  quantize_lpc(mode, lpc, lsp, qlsp, Rs, &lpc_params, e_mem);

  pack_lpc(mode, &lpc_params, packet);

  interp_lpcs(mode, e_mem->dec.last_qlsp, qlsp,
	      interp_lpc, BOTH,control);

  create_target_speech(mode, in_buffer, form_resid, pw_speech,
		       interp_lpc, &(e_mem->form_res_filt),
		       &(e_mem->spch_wght_syn_filt));

  select_mode2(&mode, form_resid, Rs, control, e_mem,in_buffer,lpc,lsp);

  /* Force the rate during TTY transmissions */
  if( tty_option == TTY_NO_GAIN && tty_enc_flag )
  {
     if( tty_enc_flag != TTY_SILENCE )
     {
         mode = FULLRATE_VOICED;
         e_mem->last_rate = mode;
     }
  }

  packet->mode=mode;

  if(mode == BLANK){
    e_mem->dec.last_G=cb_params[0][0].G=0;
    cb_params[0][0].i=0;
    pitch_params.b=e_mem->dec.last_b;
    pitch_params.frac=e_mem->dec.last_frac;
    if (pitch_params.b>1.0) {
      pitch_params.b=1.0;
    }
    pitch_params.lag=e_mem->dec.last_lag;
    for (i=0; i<LPCORDER; i++) {
      qlsp[i]=e_mem->dec.last_qlsp[i];
    }
    lsp2lpc(qlsp, interp_lpc[PITCH][0][NOT_WGHTED], LPCORDER);
    wght_factor=1;
    for (i=0; i<LPCORDER; i++) {
      wght_factor*=PERCEPT_WGHT_FACTOR;
      interp_lpc[PITCH][0][WGHTED][i]=wght_factor*interp_lpc[PITCH][0][NOT_WGHTED][i];
    }
    run_decoder(mode, &(e_mem->dec), interp_lpc[PITCH][0], &pitch_params,
		cb_params, out_buffer, FSIZE, control, 1, 1);
    pack_frame(mode, packet);

    /* don't use formant residual here, but need to update the */
    /* memories for the following frames */

    update_form_resid_mems(in_buffer, &(e_mem->form_res_filt));

    e_mem->target_snr = 0.0;
  }
  else if(mode != EIGHTH && mode != QUARTERRATE_UNVOICED ){
#if 0
    packet->sd=(random()&0xffff);
#else
    /*packet->sd=(rand()&0xffff);*/
#endif

    cb_params[0][0].sd = packet->sd_enc;

    sf_pointer=0;
    if (PITCHSF[mode]>0) {
      for(i=0; i<PITCHSF[mode]; i++) {
	save_target(&pw_speech[sf_pointer],before_search,FSIZE/PITCHSF[mode]);

	update_target_pitch(mode, &pw_speech[sf_pointer], target,
			    interp_lpc[PITCH][i][WGHTED], targ_ene,
			    &(e_mem->dec));


        /*** Turn gain to zero and set lag to TTY char */

        if( tty_option == TTY_NO_GAIN && tty_enc_flag )
        {
            /* Stuff TTY info in 1st subframe only */
            if( i == 0 )
            {
                pitch_params.qcode_frac = tty_enc_baud_rate;
                pitch_params.qcode_lag = ((tty_enc_header & 0x03) << 5);
                pitch_params.qcode_lag += tty_enc_char & 0x1F;
            }
            else
            {
                pitch_params.qcode_lag = 0;
                pitch_params.qcode_frac = 0;
            }
            unquantize_lag( &(pitch_params.lag),
                            &(pitch_params.qcode_lag),
                            &(pitch_params.frac),
                            &(pitch_params.qcode_frac)
                          );

            quantize_b( 0.0, &(pitch_params.b), &(pitch_params.qcode_b));

        }
        else
        {
            compute_pitch(mode, target, control, e_mem,
		      interp_lpc[PITCH][i][WGHTED], &pitch_params);
        }

	target_reduction(target,before_search,
			 &e_mem->pitch_target_energy[i],
			 &e_mem->pitch_target_energy_after[i],
			 FSIZE/PITCHSF[mode]);

	save_pitch(&pitch_params,&e_mem->pitch_lag[i],&e_mem->pitch_gain[i]);

	pack_pitch(&pitch_params, packet, i);

	save_target(target,before_search,
		    FSIZE/PITCHSF[mode]);

	for(j=0; j<CBSF[mode]/PITCHSF[mode]; j++) {

	  update_target_cb(mode, &pw_speech[sf_pointer],
			   &target[j*FSIZE/CBSF[mode]], interp_lpc[PITCH][i][WGHTED],
			   &pitch_params, &(e_mem->dec));
	  compute_cb(mode, &target[j*FSIZE/CBSF[mode]], e_mem,
		     interp_lpc[PITCH][i][WGHTED], cb_params, j);

	  pack_cb(mode, cb_params, packet, i, j);

	  run_decoder(mode, &(e_mem->dec), interp_lpc[PITCH][i],
		      &pitch_params, cb_params+j,
		      &out_buffer[sf_pointer], FSIZE/CBSF[mode],
		      control, 1,
		      NUMCB[mode]);

	  for(index=0;index<FSIZE/CBSF[mode];index++)
	    target[j*FSIZE/CBSF[mode]+index] =
	      pw_speech[sf_pointer+index]-e_mem->
		dec.pw_speech_out[index];



	  target_reduction(&target[j*FSIZE/CBSF[mode]],
			   &before_search[j*FSIZE/CBSF[mode]],
			   &e_mem->codebook_target_energy[j+
					    i*CBSF[mode]/PITCHSF[mode]],
			   &e_mem->codebook_target_energy_after[j+
						i*CBSF[mode]/PITCHSF[mode]],
			   FSIZE/CBSF[mode]);

	  save_target(&target[j*FSIZE/CBSF[mode]],&target_after_save
		      [j*FSIZE/CBSF[mode]+i*FSIZE/PITCHSF[mode]],FSIZE/CBSF[mode]);

	  sf_pointer+=FSIZE/CBSF[mode];
	}/* end of for(j = 0; j < CBSF[mode]/PITCHSF[mode]; j++) */
      }/* end of for(i = 0; i < PITCHSF[mode]; i++) */

      compute_target_snr(mode, e_mem);
    }
    pack_frame(mode, packet);

    e_mem->dec.last_G=fabs(cb_params[CBSF[mode]/PITCHSF[mode]-1][0].G);
    e_mem->dec.low_rate_cnt=0;

  }
  else if(mode == QUARTERRATE_UNVOICED){

    pitch_params.b=0;
    pitch_params.lag=0;
    pitch_params.frac = 0;
    pitch_params.qcode_b = 0;
    pitch_params.qcode_lag = 0;
    pitch_params.qcode_frac = 0;

    sf_pointer = 0;
    /* need to compute 5 gain parameters which get interpolated */
    for(j = 0; j < CBSF[mode]; j++){
      compute_cb_gain(mode, e_mem, cb_params, &in_buffer[sf_pointer], j);

      pack_cb(mode, cb_params, packet, 0, j);

      Gtmp[j] = cb_params[j][0].G;
      sf_pointer += FSIZE/CBSF[mode];

    }
    pack_frame(mode, packet);

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
      default:
	fprintf(stderr,
       "encoder(): mode=%d Does not support this many pitch sub frames: %d\n",
		mode, PITCHSF[mode]);
	exit(-4);
	break;
      }
      run_decoder(mode, &(e_mem->dec), interp_lpc[PITCH][i],
		  &pitch_params, cb_params,
		  &out_buffer[i*FSIZE/PITCHSF[mode]], FSIZE/PITCHSF[mode],
		  control, 2, 1);
    }
    e_mem->dec.last_G=fabs(Gtmp[4]);
    e_mem->dec.low_rate_cnt=0;
    e_mem->target_snr = 0.0;

  }
  else { /* Lowest Rate */

    pitch_params.b=0;
    pitch_params.lag=0;
    pitch_params.frac = 0;
    pitch_params.qcode_b = 0;
    pitch_params.qcode_lag = 0;
    pitch_params.qcode_frac = 0;

    pack_pitch(&pitch_params, packet, 0);

    form_ene=0;
    for (i=0; i<FSIZE; i++) {
      form_ene+=form_resid[i]*form_resid[i];
    }
    form_ene/=FSIZE;
    form_ene=sqrt(form_ene);
    form_ene*=LOWRATE_GAIN_FACTOR;

    /* correction factor to agree with DSP at decoder */
    form_ene *= G_FACTOR;

    if(e_mem->snr_map[0] > 3){
      form_ene *= 0.5;
      e_mem->hysteresis = 1;
    }
    else if(e_mem->snr_map[0] < 2 )
      e_mem->hysteresis = 0;
    else if(e_mem->hysteresis == 1)
      form_ene *= 0.5;

    quantize_G_8th(form_ene, &(cb_params[0][0].G),
		   &(cb_params[0][0].qcode_G),
		   e_mem->dec.G_pred);

    pack_cb(mode, cb_params, packet, 0, 0);
#if 0
    packet->sd=(random()&0xffff);
#endif
    packet->sd_enc=((521*(packet->sd_enc)+259)&0xffff);

    pack_frame(mode, packet);


    /* check for null Traffic Channel data */
    while (packet->data[1]==0xffff) {
      packet->sd_enc &= 0xfff7;
      pack_frame(mode, packet);
    }
    cb_params[0][0].sd = packet->sd_enc;

    last_G=0.5*(cb_params[0][0].G+e_mem->dec.last_G);

    for (i=0; i<PITCHSF8TH; i++) {
      cb_params[0][0].G=e_mem->dec.last_G*(PITCHSF8TH-1-i)/PITCHSF8TH
	+last_G*(i+1.0)/PITCHSF8TH;

      run_decoder(mode, &(e_mem->dec), interp_lpc[PITCH][0],
		  &pitch_params, cb_params,
		  &out_buffer[i*FSIZE/PITCHSF8TH], FSIZE/PITCHSF8TH,
		  control, 1, 1);
    }
    e_mem->dec.last_G=cb_params[0][0].G;
    e_mem->dec.low_rate_cnt+=1;
    e_mem->target_snr = 0.0;
  }
  e_mem->dec.last_b=pitch_params.b;
  e_mem->dec.last_lag=pitch_params.lag;
  e_mem->dec.last_frac=pitch_params.frac;

  /* Need to put in case with pitch subframes and no cb subframes */;

  for (i=0; i<LPCORDER; i++) {
    e_mem->last_lsp[i]=lsp[i];
  }
  for (i=0; i<LPCORDER; i++) {
    e_mem->dec.last_qlsp[i]=qlsp[i];
  }

  if (control->form_res_out==YES) {
    for (i=0; i<FSIZE; i++) {
      out_buffer[i]=form_resid[i];
    }
  }
  if (control->target_after_out==YES) {
    for (i=0; i<FSIZE; i++) {
      out_buffer[i]=target_after_save[i];
    }
  }
  if (control->print_packets==YES) {

    printf("lsp -> ");
    for (i=0; i<LPCORDER; i++) {
      printf("%4.2f ",lsp[i]);
    }
    printf("\nb, L -> ");
    for (i=0; i<PITCHSF[mode]; i++) {
      printf("%d %3d : ",packet->b[i], packet->lag[i]);
    }
    printf("\nG, i -> ");
    for (i=0; i<CBSF[mode]; i++) {
      printf("%d %3d : ",packet->G[i][0], packet->i[i][0]);
    }
    printf("\n\n");
  }
}/* end of encoder() */
