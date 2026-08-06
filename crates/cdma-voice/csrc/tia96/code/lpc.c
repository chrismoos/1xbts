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
/* lpc.c - linear predictive coding functions */

#include"math.h"
#include"struct.h"

compute_lpc(speech, lpc, R0, control)
float	*speech;
float	*lpc;
float   *R0;
struct  CONTROL  *control;
{
    INTTYPE	i,j;
    float R[LPCORDER+1], k[LPCORDER], E[LPCORDER+1], wspeech[LPCSIZE];
    float a[LPCORDER*LPCORDER], wght_factor;
    float compute_autocorr();

    /* window the speech signal		*/
    hamming_window(speech, wspeech, LPCSIZE);

    for (i=0; i<=LPCORDER; i++) {
	R[i]=compute_autocorr(wspeech, LPCSIZE, i);
    }
    *R0=R[0];

    E[0]=R[0];

    if (R[0]==0) {
	for (i=0; i<LPCORDER; i++) {
	    a[(LPCORDER-1)*LPCORDER+i]=0;
	}
    }
    else {
	for (i=1; i<=LPCORDER; i++) {
	    /* check for ill-conditioning, so Durbin's Recursion          */
	    /* doesn't go unstable during pseudo-non-full-rank R matrices */
	    /* such as during input files containing only pure tones      */
	    /* if >30 db prediction gain, stop the loop                   */
	    if (E[i-1]/E[0] > 0.001 ) {
		k[i-1]=R[i];
		for (j=1; j<i; j++) {
		    k[i-1]-=a[(i-2)*LPCORDER+(j-1)]*R[i-j];
		}
		k[i-1]/=E[i-1];
	    }
	    else {
		k[i-1]=0;
	    }
	    a[(i-1)*LPCORDER+(i-1)]=k[i-1];
	    for (j=i-1; j>=1; j--) {
		a[(i-1)*LPCORDER+(j-1)]=
		  a[(i-2)*LPCORDER+(j-1)]-k[i-1]*a[(i-2)*LPCORDER+(i-j-1)];
	    }
	    E[i]=(1-k[i-1]*k[i-1])*E[i-1];
	}
    }

    for (i=0; i<LPCORDER; i++) {
	lpc[i]=a[(LPCORDER-1)*LPCORDER + i];
    }

    wght_factor=1;
    for (i=0; i<LPCORDER; i++) {
	wght_factor*=BWE_FACTOR;
	lpc[i]*=wght_factor;
    }

}

/* hamming_window - perform Hamming windowing    */
hamming_window(input, output, length)
float *input, *output;
INTTYPE length;
{
    INTTYPE i;
    for (i=0; i<length; i++) {
	output[i]=input[i]*(.54-.46*cos(2.0*(double)i*PI/(double)(length-1)));
    }
}

/* compute_autocorr - computes the "shift"th autocorrelation of	*/
/* 	"signal" of windowsize "length".			*/
float compute_autocorr(signal, length, shift)
float 	*signal;
INTTYPE	length;
INTTYPE	shift;
{
    INTTYPE	i;
    float R;

    R=0;
    for (i=0; i<length-shift; i++) {
	R+= signal[i]*signal[i+shift];
    }
    return(R);
}

interp_lpcs(rate, prev_lsp, curr_lsp, lpc, rate_cnt, bright)
INTTYPE   rate;
float *prev_lsp;
float *curr_lsp;
float lpc[MAX_PITCH_SF][2][LPCORDER];
INTTYPE   *rate_cnt;
float *bright;
{
    INTTYPE   i,j;
    float wght_factor;
    float tmp_lsp[LPCORDER];
    float current_center;
    float interp_factor;
    float smooth;
    float loops;

    smooth=SMOOTH_FACTOR[rate][0];
    if ((rate==QUARTER)||(rate==EIGHTH)) {
	*rate_cnt+=1;
	if (*rate_cnt>=10) {
	    smooth=SMOOTH_FACTOR[rate][1];
	}
    }
    else if ((rate==FULL)||(rate==HALF)) {
	*rate_cnt=0;
    }

    for (i=0; i<LPCORDER; i++) {
	curr_lsp[i]=smooth*prev_lsp[i]+(1-smooth)*curr_lsp[i];
    }

    if (rate==EIGHTH) {
	loops=1;
    }
    else if (rate==BLANK) {
	loops=0;
    }
    else {
	loops=PITCHSF[rate];
    }

    for(i=0; i<loops; i++) {
	current_center=(float)(i+0.5)*(float)(FSIZE/loops);
	interp_factor= current_center+(float)(FSIZE)-(float)(LPC_CENTER);
	interp_factor/=(float)(FSIZE);

	bright[i]=0;
	for (j=0; j<LPCORDER; j++) {
	    tmp_lsp[j]=curr_lsp[j]*interp_factor
	      +prev_lsp[j]*(1-interp_factor);
	    bright[i]+=tmp_lsp[j];
	}
	bright[i]/=(float)(LPCORDER);
	if (bright[i]<0.24) {
	    bright[i]=0.25;
	}
	else if (bright[i]>0.26) {
	    bright[i]= -0.25;
	}
	else {
	    bright[i]= -25.0*(bright[i]-0.25);
	}

	lsp2lpc(tmp_lsp, lpc[i][NOT_WGHTED], LPCORDER);

	wght_factor=1;
	for (j=0; j<LPCORDER; j++) {
	    wght_factor*=PERCEPT_WGHT_FACTOR;
	    lpc[i][WGHTED][j]
	      =lpc[i][NOT_WGHTED][j]*wght_factor;
	}
    }

}
