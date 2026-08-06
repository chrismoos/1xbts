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
/* pitch.c - perform pitch functions, including open loop pitch */
/*    estimation, and the pitch search                          */

#include"struct.h"

compute_pitch(rate, target, control, e_mem, lpc, pitch_params, form_residual)
INTTYPE    rate;
float  *target;
struct CONTROL        *control;
struct ENCODER_MEM    *e_mem;
float  *lpc;
struct PITCHPARAMS    *pitch_params;
float  *form_residual;
{
    INTTYPE   i,j,k;
    INTTYPE   qcode_b;
    float lpc_ir[LENGTH_OF_IMPULSE_RESPONSE];
    float pitch_out[MAXLAG+FSIZE];
    float *pitch_out_shifted;
    float b, Exy, Eyy, err, min_err;

    for (i=0; i<LPCORDER; i++) {
	e_mem->wght_syn_filt.pole_coeff[i]=lpc[i];
    }
                                  /* get LPC impulse response */
    get_impulse_response_pole(lpc_ir, LENGTH_OF_IMPULSE_RESPONSE,
			      &(e_mem->wght_syn_filt));

    pitch_out_shifted= &pitch_out[MAXLAG];

                                  /* construct p(n) */
    for (i= -MAXLAG;  i<0; i++) {
	pitch_out_shifted[i] = e_mem->dec.pitch_filt.memory[-i-1];
    }
    for (i=0; i<FSIZE/PITCHSF[rate]-MINLAG; i++) {
	pitch_out_shifted[i] = form_residual[i];
    }                             /* complete p(n) in memory */

    initial_recursive_conv(rate, &pitch_out_shifted[1-MINLAG],
			   FSIZE/PITCHSF[rate]-1, lpc_ir);

    min_err=0;
    pitch_params->lag=MINLAG;
    pitch_params->b =0;
    quantize_b(rate, 0.0, &b, &qcode_b);

    for (i=MINLAG; i<=MAXLAG; i++) {
	recursive_conv(rate, &pitch_out_shifted[-i], lpc_ir,
		       FSIZE/PITCHSF[rate]);
	Exy=0;
	Eyy=0;
	for (j=0; j<FSIZE/PITCHSF[rate]; j++) {
	    Exy+=target[j]*pitch_out_shifted[-i+j];
	    Eyy+=pitch_out_shifted[-i+j]*pitch_out_shifted[-i+j];
	}
	if (Eyy!=0) {
	    quantize_b(rate, Exy/Eyy, &b, &qcode_b);
	}
	else {
	    quantize_b(rate, 0.0, &b, &qcode_b);
	}
	err= -2*b*Exy+b*b*Eyy;

	if (err<min_err) {
	    min_err=err;
	    pitch_params->b=b;
	    pitch_params->lag=i;
	    pitch_params->qcode_b=qcode_b;
	}
    }
    quantize_b_and_lag(rate, &(pitch_params->lag), &(pitch_params->qcode_lag),
		       &(pitch_params->b), &(pitch_params->qcode_b) );


}


initial_recursive_conv(rate, resid, length, impulse_response)
INTTYPE   rate;
float *resid;
float *impulse_response;
INTTYPE   length;
{
    INTTYPE i;

    for (i= length-1; i>=0; i--) {
	recursive_conv(rate, &resid[i], impulse_response,
		       length-i);
    }
}

recursive_conv(rate, resid, impulse_response, length)
INTTYPE   rate;
float *resid;
float *impulse_response;
INTTYPE   length;
{
    INTTYPE i;

    if (resid[0]==0) return 0;

    for (i=1; (i<length)&&(i<LENGTH_OF_IMPULSE_RESPONSE); i++) {
	resid[i]+=resid[0]*impulse_response[i];
    }
    return 0;
}
