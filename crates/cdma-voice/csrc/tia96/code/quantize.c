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
/* quantize.c - quantiztation routines */

#include"struct.h"
#include"math.h"

lin_quant(qcode, min, max, num_levels, input)
INTTYPE   *qcode;
float min, max;
INTTYPE   num_levels;
float input;
{
    *qcode=(INTTYPE)((num_levels-1)*(input-min)/(max-min)+0.5);
    if (*qcode>=num_levels) *qcode=num_levels-1;
    if (*qcode<0) *qcode=0;

}

lin_unquant(output, min, max, num_levels, qcode)
float *output;
float min, max;
INTTYPE   num_levels;
INTTYPE   qcode;
{
    *output=(max-min)*(float)(qcode)/(float)(num_levels-1) + min;
}

quantize_lsp(rate, lsp, qlsp, lpc_params, e_mem)
INTTYPE    rate;
float  *lsp;
float  *qlsp;
struct LPCPARAMS *lpc_params;
struct ENCODER_MEM *e_mem;
{
    INTTYPE   i,j;
    float err, minerror;

    for (i=0; i<LPCORDER; i++) {
	err=lsp[i] - 0.5*(float)(i+1)/(float)(LPCORDER+1)
	  - LSP_DPCM_DECAY[rate]*e_mem->dec.lsp_pred[i];

	lin_quant(&(lpc_params->qcode_lsp[i]),
		  MIN_DELTA_LSP[rate][i], MAX_DELTA_LSP[rate][i],
		  NUM_LSP_QLEVELS[rate][i], err);

    }
    unquantize_lsp(rate, qlsp, e_mem->dec.lsp_pred,
		   lpc_params->qcode_lsp);

}

unquantize_lsp(rate, qlsp, pred, qcode)
INTTYPE rate;
float *qlsp, *pred;
INTTYPE *qcode;
{
    INTTYPE i;
    float err;

    for (i=0; i<LPCORDER; i++) {
	lin_unquant(&err, MIN_DELTA_LSP[rate][i], MAX_DELTA_LSP[rate][i],
		    NUM_LSP_QLEVELS[rate][i], qcode[i]);
	qlsp[i]=err+LSP_DPCM_DECAY[rate]*pred[i];
	pred[i]= qlsp[i];
	qlsp[i]+= 0.5*(float)(i+1)/(float)(LPCORDER+1);
    }
    check_lsp_stab(qlsp);

}

check_lsp_stab(qlsp)
float *qlsp;
{
    INTTYPE i;

    if (qlsp[0]<LSP_SPREAD_FACTOR) qlsp[0]=LSP_SPREAD_FACTOR;
    for (i=1; i<LPCORDER; i++) {
	if (qlsp[i]-qlsp[i-1]<LSP_SPREAD_FACTOR) {
	    qlsp[i]=qlsp[i-1]+LSP_SPREAD_FACTOR;
	}
    }
    if (qlsp[LPCORDER-1]>0.5-LSP_SPREAD_FACTOR) {
	qlsp[LPCORDER-1]=0.5-LSP_SPREAD_FACTOR;
    }
    for (i=LPCORDER-2; i>=0; i--) {
	if (qlsp[i+1]-qlsp[i]<LSP_SPREAD_FACTOR) {
	    qlsp[i]=qlsp[i+1]-LSP_SPREAD_FACTOR;
	}
    }

}

quantize_b_and_lag(rate, lag, qcode_lag, b, qcode_b)
INTTYPE   rate;
INTTYPE   *lag;
INTTYPE   *qcode_lag;
float *b;
INTTYPE   *qcode_b;
{
    if (*b==0.0) {
	*qcode_lag= 0;
	*qcode_b=0;
	unquantize_b_and_lag(rate, lag, qcode_lag, b, qcode_b);
    }
    else {
	*qcode_lag = *lag-MINLAG+1;
	*qcode_b -=1;
	unquantize_b_and_lag(rate, lag, qcode_lag, b, qcode_b);
    }
}

unquantize_b_and_lag(rate, lag, qcode_lag, b, qcode_b)
INTTYPE   rate;
INTTYPE   *lag;
INTTYPE   *qcode_lag;
float *b;
INTTYPE   *qcode_b;
{
    if (*qcode_lag==0) {
	*lag=MINLAG;
	*b=0.0;
    }
    else {
	*lag= *qcode_lag+MINLAG-1;
	*qcode_b+=1;
	unquantize_b(rate, b, qcode_b);
	*qcode_b-=1;
    }
}

quantize_b(rate, unq_b, q_b, qcode_b)
INTTYPE   rate;
float unq_b;
float *q_b;
INTTYPE   *qcode_b;
{
    lin_quant(qcode_b, MINB, MAXB, NUMBER_OF_B_LEVELS, unq_b);
    unquantize_b(rate, q_b, qcode_b);
}

unquantize_b(rate, q_b, qcode_b)
INTTYPE   rate;
float *q_b;
INTTYPE   *qcode_b;
{
    lin_unquant(q_b, MINB, MAXB, NUMBER_OF_B_LEVELS, *qcode_b);
}

quantize_i(rate, i, qcode_i)
INTTYPE   rate;
INTTYPE   *i;
INTTYPE   *qcode_i;
{
    *qcode_i= *i;
    unquantize_i(rate, i, qcode_i);
}

unquantize_i(rate, i, qcode_i)
INTTYPE   rate;
INTTYPE   *i;
INTTYPE   *qcode_i;
{
    *i=*qcode_i;
}

quantize_G(rate, unq_G, q_G, qcode_G, qcode_Gsign, G_pred)
INTTYPE   rate;
float unq_G;
float *q_G;
INTTYPE   *qcode_G;
INTTYPE   *qcode_Gsign;
INTTYPE   *G_pred;
{
    INTTYPE   i;
    INTTYPE   ind;
    float pred;
    float G;
    float min_error, err;

    pred=0;
    for (i=0; i<GORDER; i++) {
	pred+=G_pred[i];
    }
    pred/=(float)(GORDER);

    /* change in Version 2.01.  This makes the floor function correct for */
    /* negative values of pred. */
    if (pred<0) {
	pred-=1.0;
    }

    ind=(INTTYPE)(pred);
    ind=FG[ind+6]+6;

    if (unq_G>0) {
	*qcode_Gsign=POSITIVE;
	G=unq_G;
    }
    else {
	*qcode_Gsign=NEGATIVE;
	G= -unq_G;
    }

    min_error=1000000;
    *qcode_G=0;
    for (i=0; i<4; i++) {
	err=(G-GA[ind+QG[rate][i]])*(G-GA[ind+QG[rate][i]]);
	if (min_error>err) {
	    min_error=err;
	    *qcode_G=i;
	}
    }

    unquantize_G(rate, q_G, qcode_G, qcode_Gsign, G_pred);
}

unquantize_G(rate, q_G, qcode_G, qcode_Gsign, G_pred)
INTTYPE   rate;
float *q_G;
INTTYPE   *qcode_G;
INTTYPE   *qcode_Gsign;
INTTYPE   *G_pred;
{
    INTTYPE   i;
    float pred;
    INTTYPE   ind;

    pred=0;
    for (i=0; i<GORDER; i++) {
	pred+=G_pred[i];
    }
    pred/=(float)(GORDER);

    /* change in Version 2.01.  This makes the floor function correct for */
    /* negative values of pred. */
    if (pred<0) {
	pred-=1.0;
    }

    ind=(INTTYPE)(pred);
    ind=FG[ind+6]+6;

    *q_G= GA[ind+QG[rate][*qcode_G]];
    if (*qcode_Gsign==NEGATIVE) {
	*q_G*= -1;
    }
}

update_Gpred(rate, qcode_G, G_pred)
INTTYPE rate;
INTTYPE qcode_G, *G_pred;
{
    INTTYPE   i;
    float pred;
    INTTYPE   ind;

    pred=0;
    for (i=0; i<GORDER; i++) {
	pred+=G_pred[i];
    }
    pred/=(float)(GORDER);
    ind=(INTTYPE)(pred);
    ind=FG[ind+6];

    for (i=GORDER-1; i>0; i--) {
	G_pred[i]=G_pred[i-1];
    }
    G_pred[0]=ind+QG[rate][qcode_G];

}
