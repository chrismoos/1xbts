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
/* target.c - create and update the "target" waveform for the searches */

#include"struct.h"

compute_formant_residual(rate, input, resid, lpc, err_filt)
INTTYPE    rate;
float  *input;
float  *resid;
float  lpc[MAX_PITCH_SF][2][LPCORDER];
struct ZERO_FILTER *err_filt;
{
    INTTYPE   i,j;
    INTTYPE   sf_ptr;

    sf_ptr=0;
    for (i=0; i<PITCHSF[rate]; i++) {
	for (j=0; j<LPCORDER; j++) {
	    err_filt->zero_coeff[j]= -lpc[i][NOT_WGHTED][j];
	}
	do_zero_filter(&input[sf_ptr], &resid[sf_ptr], FSIZE/PITCHSF[rate],
		       err_filt, UPDATE);

	sf_ptr+=FSIZE/PITCHSF[rate];

    }
}

update_form_resid_mems(input, err_filt)
float  *input;
struct ZERO_FILTER *err_filt;
{
    INTTYPE   i,j;

    for (i=0; i<LPCORDER; i++) {
	err_filt->memory[i]= input[FSIZE-1-i];
    }
}

create_pitch_target(rate, input, target, lpc, d_mem)
INTTYPE   rate;
float *input;
float *target;
float lpc[2][LPCORDER];
struct DECODER_MEM *d_mem;
{
    INTTYPE   i;
    float zir[FSIZE];

    for (i=0; i<LPCORDER; i++) {
	d_mem->lpc_filt.pole_coeff[i]= lpc[NOT_WGHTED][i];
	d_mem->wghting_filt.zero_coeff[i]= -lpc[NOT_WGHTED][i];
	d_mem->wghting_filt.pole_coeff[i]= lpc[WGHTED][i];
    }

    get_zero_input_response_pole(zir, FSIZE/PITCHSF[rate],
				       &(d_mem->lpc_filt));
    for (i=0; i<FSIZE/PITCHSF[rate]; i++) {
	target[i]=input[i]-zir[i];
    }

    do_pole_zero_filter(target, target, FSIZE/PITCHSF[rate],
			&(d_mem->wghting_filt), NO_UPDATE);

}

create_cb_target(rate, input, target, lpc, pitch_params, d_mem)
INTTYPE   rate;
float *input;
float *target;
float lpc[2][LPCORDER];
struct PITCHPARAMS *pitch_params;
struct DECODER_MEM *d_mem;
{
    INTTYPE   i;
    float tmp_mem[LPCORDER];
    float zir_p[FSIZE], pitch_est[FSIZE];

    d_mem->pitch_filt.delay=pitch_params->lag;
    d_mem->pitch_filt.coeff=pitch_params->b;

    get_zero_input_response_pole_1_tap(zir_p, FSIZE/CBSF[rate],
				       &(d_mem->pitch_filt));

    for (i=0; i<LPCORDER; i++) {
	d_mem->lpc_filt.pole_coeff[i]= lpc[NOT_WGHTED][i];
	d_mem->wghting_filt.zero_coeff[i]= -lpc[NOT_WGHTED][i];
	d_mem->wghting_filt.pole_coeff[i]= lpc[WGHTED][i];
    }

    do_pole_filter(zir_p, pitch_est, FSIZE/CBSF[rate],
		   &(d_mem->lpc_filt), NO_UPDATE);

    for (i=0; i<FSIZE/CBSF[rate]; i++) {
	target[i]=input[i]-pitch_est[i];
    }

    do_pole_zero_filter(target, target, FSIZE/CBSF[rate],
			&(d_mem->wghting_filt), NO_UPDATE);

}
