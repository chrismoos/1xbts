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
/* cb.c - codebook search routines */

#include"struct.h"

compute_cb(rate, target, control, e_mem, lpc, cb_params)
INTTYPE    rate;
float  *target;
struct CONTROL        *control;
struct ENCODER_MEM    *e_mem;
float  *lpc;
struct CBPARAMS       *cb_params;
{
    INTTYPE   i,j;
    INTTYPE   qcode_G, qcode_Gsign;
    float lpc_ir[LENGTH_OF_IMPULSE_RESPONSE];
    float cb_out[CBLENGTH+FSIZE];
    float *cb_out_shifted;
    float G, Exy, Eyy, err, min_err;

    for (i=0; i<LPCORDER; i++) {
	e_mem->wght_syn_filt.pole_coeff[i]=lpc[i];
    }

                                  /* get LPC impulse response */
    get_impulse_response_pole(lpc_ir, LENGTH_OF_IMPULSE_RESPONSE,
			      &(e_mem->wght_syn_filt));

                                  /* construct c(n) */
    cb_out_shifted= &cb_out[CBLENGTH];

    for (i= -CBLENGTH+1; i<FSIZE/CBSF[rate]; i++) {
	cb_out_shifted[i] = CODEBOOK[(i+CBLENGTH)%CBLENGTH];
    }                             /* complete c(n) in memory */

    initial_recursive_conv(rate, &cb_out_shifted[1],
			   FSIZE/CBSF[rate] -1, lpc_ir);

    min_err=0; /* dummy initialization - reinitialized at first index */
    for (i=0; i<CBLENGTH; i++) {
	recursive_conv(rate, &cb_out_shifted[-i], lpc_ir,
		       FSIZE/CBSF[rate]);
	Exy=0;
	Eyy=0;
	for (j=0; j<FSIZE/CBSF[rate]; j++) {
	    Exy+=target[j]*cb_out_shifted[-i+j];
	    Eyy+=cb_out_shifted[-i+j]*cb_out_shifted[-i+j];
	}
	if (Eyy!=0) { /* quantization in the loop performs the same */
	              /* search as searching over all quantized Gs  */
	    quantize_G(rate, Exy/Eyy, &G, &qcode_G, &qcode_Gsign,
		       e_mem->dec.G_pred);
	}
	else {
	    quantize_G(rate, .025, &G, &qcode_G, &qcode_Gsign,
		       e_mem->dec.G_pred);
	}
	err= -2*G*Exy+G*G*Eyy;
	if ((i==0)||(err<min_err)) {
	    min_err=err;
	    cb_params->G=G;
	    cb_params->i=i;
	    cb_params->qcode_G=qcode_G;
	    cb_params->qcode_Gsign=qcode_Gsign;
	}
    }
    quantize_i(rate, &(cb_params->i), &(cb_params->qcode_i));
    update_Gpred(rate, cb_params->qcode_G, e_mem->dec.G_pred);

}
