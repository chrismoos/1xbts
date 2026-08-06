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
/* init.c - initialization of all coder parameters. defaults here. */

#include"math.h"
#include"struct.h"

initialize_encoder_and_decoder(e_mem, d_mem, control)
struct  ENCODER_MEM *e_mem;
struct  DECODER_MEM *d_mem;
struct  CONTROL *control;
{
    INTTYPE i;
    INTTYPE j;

    /* initialize the encoder */
    e_mem->dec.type=ENCODER;
    e_mem->dc_block_mem=0;
    e_mem->seed=0;
    e_mem->noise_est=HIGH_THRESH_LIM;
    for (i=0; i<LPCORDER; i++) {
	e_mem->dec.last_qlsp[i]= 0.5*(float)(i+1)/(float)(LPCORDER+1);
    }
    e_mem->dec.last_b=0;
    e_mem->dec.last_lag=0;
    e_mem->dec.last_G=0;
    initialize_zero_filter(&(e_mem->form_res_filt), LPCORDER);
    initialize_pole_filter(&(e_mem->wght_syn_filt), LPCORDER);
    initialize_pole_1_tap_filter(&(e_mem->dec.pitch_filt), MAXLAG);
    initialize_pole_filter(&(e_mem->dec.lpc_filt), LPCORDER);
    initialize_pole_zero_filter(&(e_mem->dec.wghting_filt), LPCORDER);

    /* initialize the decoder */
    d_mem->type=DECODER;
    d_mem->seed=0;
    for (i=0; i<LPCORDER; i++) {
	d_mem->last_qlsp[i]= 0.5*(float)(i+1)/(float)(LPCORDER+1);
    }
    d_mem->last_b=0;
    d_mem->last_lag=0;
    d_mem->last_G=0;
    d_mem->agc_factor=1.0;
    initialize_pole_1_tap_filter(&(d_mem->pitch_filt), MAXLAG);
    initialize_pole_filter(&(d_mem->lpc_filt), LPCORDER);
    initialize_zero_filter(&(d_mem->post_filt_z), LPCORDER);
    initialize_pole_filter(&(d_mem->post_filt_p), LPCORDER);
    initialize_pole_zero_filter(&(d_mem->bright_filt), 1);
}
