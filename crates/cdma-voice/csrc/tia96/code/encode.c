/**********************************************************************/
/* QCELP Variable Rate Speech Codec - Simulation of TIA IS96-A, service */
/*     option one for TIA IS95, North American Wideband CDMA Digital  */
/*     Cellular Telephony.                                            */
/*                                                                    */
/* (C) Copyright 1993, QUALCOMM Incorporated                          */
/* QUALCOMM Incorporated                                              */
/* 10555 Sorrento Valley Road                                         */
/* San Diego, CA 92121                                                */
/*                                                                    */
/* Note:  Reproduction and use of this software for the design and    */
/*     development of North American Wideband CDMA Digital            */
/*     Cellular Telephony Standards is authorized by                  */
/*     QUALCOMM Incorporated.  QUALCOMM Incorporated does not         */
/*     authorize the use of this software for any other purpose.      */
/*                                                                    */
/*     The availability of this software does not provide any license */
/*     by implication, estoppel, or otherwise under any patent rights */
/*     of QUALCOMM Incorporated or others covering any use of the     */
/*     contents herein.                                               */
/*                                                                    */
/*     Any copies of this software or derivative works must include   */
/*     this and all other proprietary notices.                        */
/**********************************************************************/
/* encode.c - main QCELP encoder */

#include"math.h"
#include"struct.h"

encoder(in_buffer, packet, control, sig_ptr, e_mem, out_buffer, frame_num)
float   *in_buffer, *out_buffer;
struct  PACKET          *packet;
struct  CONTROL         *control;
struct  SIGNAL_DATA     **sig_ptr;
struct  ENCODER_MEM     *e_mem;
INTTYPE     frame_num;
{
    INTTYPE    i,j,l;
    float  lpc[LPCORDER], interp_lpc[MAX_PITCH_SF][2][LPCORDER];
    float  lsp[LPCORDER];
    float  qlsp[LPCORDER];
    float  bright[MAX_PITCH_SF]; /* not used in the encoder */
    struct LPCPARAMS   lpc_params;
    struct PITCHPARAMS pitch_params;
    struct CBPARAMS    cb_params;
    float  R0;
    INTTYPE    rate;
    INTTYPE    sf_pointer;
    float  form_resid[FSIZE], target[FSIZE], pw_speech[FSIZE];
    float  wght_factor;
    float  last_G;

    dc_block(&in_buffer[LPCOFFSET], &(e_mem->dc_block_mem), FSIZE, control);

    compute_lpc(&in_buffer[LPCOFFSET], lpc, &R0, control);

    lpc2lsp(lpc, lsp, LPCORDER);

    select_rate(&rate, frame_num, R0, &(e_mem->noise_est), control, sig_ptr,
		&(e_mem->last_rate), &(packet->rate));

    quantize_lsp(rate, lsp, qlsp, &lpc_params, e_mem);

    pack_lsp(rate, &lpc_params, packet);

    interp_lpcs(rate, e_mem->dec.last_qlsp, qlsp, interp_lpc,
		&(e_mem->dec.low_rate_cnt), bright);

    if (rate==BLANK) {
	cb_params.G=0;
	cb_params.i=0;
	pitch_params.b=e_mem->dec.last_b;
	if (pitch_params.b>1.0) {
	    pitch_params.b=1.0;
	}
	pitch_params.lag=e_mem->dec.last_lag;
	for (i=0; i<LPCORDER; i++) {
	    qlsp[i]=e_mem->dec.last_qlsp[i];
	}
	lsp2lpc(qlsp, interp_lpc[0][NOT_WGHTED], LPCORDER);
	wght_factor=1;
	for (i=0; i<LPCORDER; i++) {
	    wght_factor*=PERCEPT_WGHT_FACTOR;
	    interp_lpc[0][WGHTED][i]=wght_factor*interp_lpc[0][NOT_WGHTED][i];
	}
	run_decoder(rate, &(e_mem->dec), interp_lpc[0], &pitch_params,
		    &cb_params, in_buffer, out_buffer, bright[0],
		    FSIZE, control);
	pack_frame(rate, packet);
	/* don't use formant residual here, but need to update the */
	/* memories for the following frames */
	update_form_resid_mems(in_buffer, &(e_mem->form_res_filt));
    }
    else if (rate==EIGHTH) {
	pitch_params.b=0;
	pitch_params.lag=17;

	create_cb_target(rate, in_buffer, target,
			 interp_lpc[0], &pitch_params, &(e_mem->dec));

	compute_cb(rate, target, control, e_mem, interp_lpc[0][WGHTED],
		   &cb_params);

	pack_cb(rate, &cb_params, packet, 0, 0);

	e_mem->seed=((521*e_mem->seed +259)&0xffff);
	packet->sd=e_mem->seed;

	pack_frame(rate, packet);

	/* check for null Traffic Channel data */
	while (packet->data[1]==0xffff) {
	    e_mem->seed=((521*e_mem->seed +259)&0xffff);
	    packet->sd=e_mem->seed;
	    pack_frame(rate, packet);
	}

	cb_params.G=fabs(cb_params.G);
	last_G=0.5*(cb_params.G+e_mem->dec.last_G);
	cb_params.seed=packet->data[1];

	for (j=0; j<8; j++) {
	    cb_params.G= e_mem->dec.last_G*(7.0-j)/8.0 +last_G*(j+1.0)/8.0;
	    run_decoder(rate, &(e_mem->dec), interp_lpc[0], &pitch_params,
			&cb_params, &in_buffer[j*20], &out_buffer[j*20],
			bright[0], 20, control);
	}
	/* don't use formant residual here, but need to update the */
	/* memories for the following frames */
	update_form_resid_mems(in_buffer, &(e_mem->form_res_filt));
    }
    else {
	compute_formant_residual(rate, in_buffer, form_resid, interp_lpc,
				 &(e_mem->form_res_filt) );

	sf_pointer=0;

	for(i=0; i<PITCHSF[rate]; i++) {

	    create_pitch_target(rate, &in_buffer[sf_pointer], target,
		      interp_lpc[i], &(e_mem->dec));
	    compute_pitch(rate, target, control, e_mem,
			  interp_lpc[i][WGHTED], &pitch_params,
			  &form_resid[sf_pointer]);
	    pack_pitch(rate, &pitch_params, packet, i);

	    for(j=0; j<CBSF[rate]/PITCHSF[rate]; j++) {

		create_cb_target(rate, &in_buffer[sf_pointer], target,
		      interp_lpc[i], &pitch_params, &(e_mem->dec));

		compute_cb(rate, target, control, e_mem,
		      interp_lpc[i][WGHTED],
		      &cb_params);
		pack_cb(rate, &cb_params, packet, i, j);

		run_decoder(rate, &(e_mem->dec), interp_lpc[i],
			    &pitch_params, &cb_params,
			    &in_buffer[sf_pointer],
			    &out_buffer[sf_pointer], bright[i],
			    FSIZE/CBSF[rate],
			    control);

		sf_pointer+=FSIZE/CBSF[rate];
	    }
	}
	pack_frame(rate, packet);
    }

    for (i=0; i<LPCORDER; i++) {
	e_mem->dec.last_qlsp[i]=qlsp[i];
    }
    e_mem->dec.last_b  =pitch_params.b;
    e_mem->dec.last_lag=pitch_params.lag;
    e_mem->dec.last_G= (float)fabs((double)(cb_params.G));

    /* update buffer for next frame */
    for (i=0; i<LPCOFFSET; i++) {
	in_buffer[i]=in_buffer[i+FSIZE];
    }

}
