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
/* decode.c - main QCELP decoder */

#include"math.h"
#include"struct.h"

decoder(out_speech, packet, control, d_mem)
float *out_speech;
struct PACKET      *packet;
struct CONTROL     *control;
struct DECODER_MEM *d_mem;
{
    INTTYPE    i,j;
    INTTYPE    rate;
    INTTYPE    cbsf_number;
    float  qlsp[LPCORDER];
    float  lpc[MAX_PITCH_SF][2][LPCORDER];
    float  bright[MAX_PITCH_SF];
    float  out_nopf[FSIZE];
    float  last_G;
    struct PITCHPARAMS pitch_params;
    struct CBPARAMS    cb_params;

    unpack_frame(packet, &(d_mem->err_cnt), d_mem->last_b, d_mem->last_lag);
    rate=packet->rate;

    if (rate==BLANK) {
	cb_params.G=0;
	cb_params.i=0;  /* arbitrary */
	pitch_params.b  =d_mem->last_b;
	if (pitch_params.b>1.0) {
	    pitch_params.b=1.0;
	}
	pitch_params.lag=d_mem->last_lag;
	bright[0]=0;
	for (i=0; i<LPCORDER; i++) {
	    qlsp[i]=d_mem->last_qlsp[i];
	    bright[0]+=qlsp[i];
	}
	bright[0]/=(float)(LPCORDER);
	if (bright[0]<0.24) {
	    bright[0]=  0.25;
	}
	else if (bright[0]>0.26) {
	    bright[0]= -0.25;
	}
	else {
	    bright[0]= -25.0*(bright[0]-0.25);
	}
	lsp2lpc(qlsp, lpc[0][NOT_WGHTED], LPCORDER);
	run_decoder(rate, d_mem, lpc[0], &pitch_params, &cb_params,
		    out_nopf, out_speech, bright[0], FSIZE, control);
    }
    else if (rate==ERASURE) {
	cb_params.qcode_G= 0.7*d_mem->G_pred[0];
	d_mem->G_pred[1]=d_mem->G_pred[0];
	d_mem->G_pred[0]=cb_params.qcode_G;
	cb_params.G=GA[cb_params.qcode_G +6];
	d_mem->seed=((521*(d_mem->seed)+259)&0xffff);
	cb_params.i=((d_mem->seed)&0x007f);
	pitch_params.b  =d_mem->last_b;
	if (pitch_params.b>ERASE_B_SAT[d_mem->err_cnt]) {
	    pitch_params.b=ERASE_B_SAT[d_mem->err_cnt];
	}
	pitch_params.lag=d_mem->last_lag;
	for (i=0; i<LPCORDER; i++) {
	    d_mem->lsp_pred[i]*= .90625;
	    qlsp[i]= d_mem->lsp_pred[i]+0.5*(float)(i+1)/(float)(LPCORDER+1);
	}
	check_lsp_stab(qlsp);
	bright[0]=0;
	for (i=0; i<LPCORDER; i++) {
	    qlsp[i]=.875*d_mem->last_qlsp[i]+.125*qlsp[i];
	    bright[0]+=qlsp[i];
	}
	bright[0]/=(float)(LPCORDER);
	if (bright[0]<0.24) {
	    bright[0]=  0.25;
	}
	else if (bright[0]>0.26) {
	    bright[0]= -0.25;
	}
	else {
	    bright[0]= -25.0*(bright[0]-0.25);
	}
	lsp2lpc(qlsp, lpc[0][NOT_WGHTED], LPCORDER);
	run_decoder(rate, d_mem, lpc[0], &pitch_params, &cb_params,
		    out_nopf, out_speech, bright[0], FSIZE, control);
    }
    else if (rate==EIGHTH) {
	unquantize_lsp(rate, qlsp, d_mem->lsp_pred, packet->lsp);
	interp_lpcs(rate, d_mem->last_qlsp, qlsp, lpc, &(d_mem->low_rate_cnt),
		    bright);

	unpack_cb(rate, packet, &cb_params, 0);
	unquantize_G(rate, &(cb_params.G), &(cb_params.qcode_G),
		     &(cb_params.qcode_Gsign), d_mem->G_pred);
	update_Gpred(rate, cb_params.qcode_G, d_mem->G_pred);
	last_G = 0.5*(cb_params.G+d_mem->last_G);

	cb_params.seed=packet->data[1];

	pitch_params.b=0.0;
	pitch_params.lag=0;

	for (j=0; j<8; j++) {
	    cb_params.G= d_mem->last_G*(7-j)/8.0 +last_G*(j+1)/8.0;
	    run_decoder(rate, d_mem, lpc[0], &pitch_params, &cb_params,
			&out_nopf[j*FSIZE/8], &out_speech[j*FSIZE/8],
			bright[0], FSIZE/8, control);
	}
    }
    else { /* Full, Half, or Quarter rates here */
	unquantize_lsp(rate, qlsp, d_mem->lsp_pred, packet->lsp);
	interp_lpcs(rate, d_mem->last_qlsp, qlsp, lpc, &(d_mem->low_rate_cnt),
		    bright);

	for (i=0; i<PITCHSF[rate]; i++) {
	    unquantize_b_and_lag(rate, &(pitch_params.lag), &(packet->lag[i]),
				 &(pitch_params.b), &(packet->b[i]));
	    for (j=0; j<CBSF[rate]/PITCHSF[rate]; j++) {
		cbsf_number=i*CBSF[rate]/PITCHSF[rate]+j;
		unpack_cb(rate, packet, &cb_params, cbsf_number);
		unquantize_G(rate, &(cb_params.G), &(cb_params.qcode_G),
			     &(cb_params.qcode_Gsign), d_mem->G_pred);
		update_Gpred(rate, cb_params.qcode_G, d_mem->G_pred);
		unquantize_i(rate, &(cb_params.i), &(cb_params.qcode_i));
		run_decoder(rate, d_mem, lpc[i], &pitch_params, &cb_params,
			    &out_nopf[cbsf_number*FSIZE/CBSF[rate]],
			    &out_speech[cbsf_number*FSIZE/CBSF[rate]],
			    bright[i], FSIZE/CBSF[rate], control);
	    }
	}
    }

    for (i=0; i<4; i++) {
	agc(&out_nopf[i*FSIZE/4], &out_speech[i*FSIZE/4],
	    &out_speech[i*FSIZE/4], FSIZE/4, &(d_mem->agc_factor));
    }

    /* Update parameters for next frame */
    for (i=0; i<LPCORDER; i++) {
	d_mem->last_qlsp[i]=qlsp[i];
    }
    d_mem->last_b  =pitch_params.b;
    d_mem->last_lag=pitch_params.lag;
    d_mem->last_G= (float)fabs((double)(cb_params.G));

}

run_decoder(rate, d_mem, lpc, pitch_params, cb_params, buffer, out_buffer,
	    bright, length, control)
INTTYPE                     rate;
struct  DECODER_MEM     *d_mem;
float                   lpc[2][LPCORDER];
struct  PITCHPARAMS     *pitch_params;
struct  CBPARAMS        *cb_params;
float                   *buffer;
float                   *out_buffer;
float                   bright;
INTTYPE                     length;
struct  CONTROL         *control;
{
    INTTYPE j;
    float zero_wght_factor, pole_wght_factor;
    float cb_out[FSIZE], pitch_out[FSIZE], pf_out[FSIZE], null_out[FSIZE];

    if (rate==EIGHTH) {
	for (j=0; j<length; j++) {
	    cb_params->seed=(521*cb_params->seed+259)&(0xffff);
	    cb_out[j]=(((cb_params->seed+0x7fff)&(0xffff))-0x7fff);
	    cb_out[j]*=1.3736895*cb_params->G/32768.0;
	}
	d_mem->seed=cb_params->seed;
    }
    else {
	for (j=0; j<length; j++) {
	    cb_out[j]=CODEBOOK[(CBLENGTH-cb_params->i +j)%CBLENGTH]
	      *cb_params->G;
	}
    }
    d_mem->pitch_filt.coeff=pitch_params->b;
    d_mem->pitch_filt.delay=pitch_params->lag;

    do_pole_filter_1_tap(cb_out, pitch_out, length, &(d_mem->pitch_filt),
			 UPDATE);

    if (d_mem->type==ENCODER) {
	for (j=0; j<LPCORDER; j++) {
	    d_mem->lpc_filt.pole_coeff[j]=lpc[NOT_WGHTED][j];
	    d_mem->wghting_filt.zero_coeff[j]=-lpc[NOT_WGHTED][j];
	    d_mem->wghting_filt.pole_coeff[j]=lpc[WGHTED][j];
	}
	do_pole_filter(pitch_out, out_buffer, length,
		       &(d_mem->lpc_filt), UPDATE);
	for (j=0; j<length; j++) {
	                /* buffer holds input speech in encoder case */
	    null_out[j]=buffer[j]-out_buffer[j];
	}
	do_pole_zero_filter(null_out, null_out, length,
			    &(d_mem->wghting_filt), UPDATE);
    }
    else {
	for (j=0; j<LPCORDER; j++) {
	    d_mem->lpc_filt.pole_coeff[j]=lpc[NOT_WGHTED][j];
	}
	                /* buffer holds no_pf output in decoder case */
	do_pole_filter(pitch_out, buffer, length, &(d_mem->lpc_filt),
		       UPDATE);

	if (control->pf_flag==PF_ON) {
	    zero_wght_factor=1;
	    pole_wght_factor=1;
	    for (j=0; j<LPCORDER; j++) {
	      zero_wght_factor*=PF_ZERO_WGHT_FACTOR;
	      d_mem->post_filt_z.zero_coeff[j]=
		-lpc[NOT_WGHTED][j]*zero_wght_factor;
	      pole_wght_factor*=PF_POLE_WGHT_FACTOR;
	      d_mem->post_filt_p.pole_coeff[j]=
		lpc[NOT_WGHTED][j]*pole_wght_factor;
	    }
	    do_zero_filter(buffer, pf_out, length,
			   &(d_mem->post_filt_z), UPDATE);
	    do_pole_filter(pf_out, pf_out, length,
			   &(d_mem->post_filt_p), UPDATE);

	    d_mem->bright_filt.pole_coeff[0]= -bright;
	    d_mem->bright_filt.zero_coeff[0]= -bright;
	    do_pole_zero_filter(pf_out, out_buffer, length,
				&(d_mem->bright_filt), UPDATE);
	}
	else {
	    for (j=0; j<length; j++) {
		out_buffer[j]=buffer[j];
	    }
	}
    }

}

agc(input, output, return_vals, length, factor)
float *input, *output, *return_vals;
INTTYPE   length;
float *factor;
{
    INTTYPE i;
    float input_energy, output_energy;
    float norm;

    input_energy=0;
    output_energy=0;
    for (i=0; i<length; i++) {
	input_energy+=input[i]*input[i];
	output_energy+=output[i]*output[i];
    }
    if (output_energy>.1) {
	norm=sqrt((double)(input_energy/output_energy));
    }
    else {
	norm= *factor;
    }
    *factor=AGC_FACTOR*(*factor)+(1-AGC_FACTOR)*norm;

    for (i=0; i<length; i++) {
	return_vals[i]=output[i]*(*factor);
    }
}
