/**********************************************************************
Each of the companies; Qualcomm, Motorola, Lucent, and Nokia (hereinafter 
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
all Normative portions of the EVRC-WB Variable Rate Speech Codec as 
it exists on the date of submittal of this form, should the EVRC-WB be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC-WB or the field of use of practice of the 
EVRC-WB Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/*****************************************************************
 *   (C) COPYRIGHT 1999,2000 Motorola, Inc.
 *****************************************************************/
#include <stdio.h>
#include <math.h>
#include <float.h>
#include "struct.h"
#include "macro.h"
#include "acelp.h"
#include "upgrades.h"



#define PRINT_OUT 0

#define	PRINT_NUM_ITERATION	NO

#define	CONFIG	1

#define	N0	4		/* # of candidate positions for pulse 1 */
#define	N1	8		/* # of candidate positions for pulse 2 */
#define	N2	12		/* # of candidate positions for pulse 3 */
#define N3	16

#define N_PRE	24		/* # of candidate combinations for pulse 1, 2 and 3 */
#define	N_POST	7






void
FGV_MEM::cod7_35 (float *xw,	/* (input)  Target signal. */
		  float *x,	/* (input)  Target signal (through inverse weighting filter). */
		  int l_subfr,	/* (input)  Length of current subframe. */
		  float *h,	/* (input)  Weighted filter impulse response. */
		  float *ck,	/* (output) Multipulse excitation. */
		  float y[],	/* (output) Filtered fixed codebook excitation. */
		  float *gain,	/* (output) Excitation gain parameter. */
		  int *indx)
{				/* (output) Factorial Packed codeword. */




    float rr[L_CODE][L_CODE], rr0[L_CODE], alpn[N_PRE], sqn[N_PRE];

    float d2[MAX_TLEN], sign[MAX_TLEN], accum[MAX_TLEN];

    float y3[SubFrameSize];

    int ordered_pos[MAX_TLEN], ivect[N_PULSES * 10], jvect[N_PULSES];
    int i, j, k, m, n, l, dec, pos, vect3[N_PRE][4];

    register int i0, i1, i2, ia, ib, i3, i4, i5, i6;

    int64 codeword;
    float ps, ps0, ps1, ps2, psk;
    float alp, alp0, alp1, alp2, alpk;
    float sq, sq2, sqk, *ptr;



#undef N_PULSES_WB
    int N_PULSES_WB;

    if (da_index[sfcnt] == 0) {
	N_PULSES_WB = 6;
    }
    else {
	N_PULSES_WB = 5;
    }
    sfcnt = ++sfcnt % NoOfSubFrames;




    /* Generate backward filtered target vector d[n] (d' = xw'H) */
    for (i = 0; i < l_subfr; i++) {
	ps = 0;
	for (j = i; j < l_subfr; j++)
	    ps += h[j - i] * xw[j];
	d2[i] = ps;
    }
    if (data_packet.WB_MODE_BIT == 1) {

	for (i = 0; i < l_subfr; i++) {
	    ps = 0;
	    for (j = i; j < l_subfr; j++) {
		ps += h[j - i] * y[j];
	    }
	    y3[i] = ps;
	}


    }


    /* Put h[i]= d2[i] = x[i] = 0; for i>l_subfr */
    for (i = l_subfr; i < L_CODE; i++)
	h[i] = d2[i] = x[i] = 0.0;

	/*----------------------------------------------------------------*
  	*     BUILD "d2[]", "sign[]" AND "d6[]" VECTORS.                 *
  	*----------------------------------------------------------------*/


    if (data_packet.WB_MODE_BIT == 1) {	//if wideband


    }
    else {
	ps1 = 1e-20;
	ps2 = 1e-20;
	for (i = 0; i < l_subfr; i++) {
	    ps1 += (x[i] * x[i]);
	    ps2 += (d2[i] * d2[i]);
	}
	ps = sqrt (ps2 / ps1);


    }

    for (j = 0; j < l_subfr; j++) {
	if (data_packet.WB_MODE_BIT == 1) {	//if wideband              
	    ps2 = d2[j];
	}
	else
	    ps2 = (ps * x[j]) + (2.0 * d2[j]);

	if (ps2 >= 0.0)
	    sign[j] = 1.0;
	else {
	    sign[j] = -1.0;
	    d2[j] = -d2[j];
	    ps2 = -ps2;
	}
	if (data_packet.WB_MODE_BIT == 1) {	//if wideband      

	}
	else {
	    accum[j] = ps2;
	}

    }


	/*----------------------------------------------------------------*
  	*        BUILD "rr[][]", MATRIX OF AUTOCORRELATION.              *
  	*----------------------------------------------------------------*/
    if (data_packet.WB_MODE_BIT == 1) {

	i = l_subfr - 1;
	for (k = 0, ps = 0.0; k < l_subfr; k++, i--) {
	    ps += h[k] * h[k];

	    rr0[i] = 0.5 * (ps - y3[i] * y3[i]);
	    rr[i][i] = ps - y3[i] * y3[i];

	}

	for (dec = 1; dec < l_subfr; dec++) {
	    j = l_subfr - 1;
	    i = j - dec;
	    for (k = 0, ps = 0.0; k < (l_subfr - dec); k++, i--, j--) {
		ps += h[k] * h[k + dec];

		rr[i][j] = rr[j][i] =
		    (ps - y3[i] * y3[j]) * sign[i] * sign[j];

	    }
	}


	/* Put rr[i][j] = 0; for i,j>l_subfr */
	for (i = l_subfr; i < L_CODE; i++) {
	    rr0[i] = 0.0;
	    for (j = 0; j < L_CODE; j++)
		rr[i][j] = rr[j][i] = 0.0;
	}



    }
    else {			//end of data_packet.WB_MOD_BIT == 1
	//Doing regular diag elements
	for (k = 0, ps = 0.0; k < l_subfr; k++) {
	    ps += h[k] * h[k];
	}
	rr0[0] = ps;		//note rr0 has the first row of matrix instead of 
	//     of diagonal in EVRC

	//Make first row
	for (dec = 1; dec < l_subfr; dec++) {
	    for (k = 0, ps = 0.0; k < (l_subfr - dec); k++) {
		ps += h[k] * h[k + dec];
	    }
	    rr0[dec] = ps;
	}
    }



    if (data_packet.WB_MODE_BIT == 1) {

	for (j = 0; j < l_subfr; j++) {
	    if (rr[j][j] != 0) {
		accum[j] = sqrt (d2[j] * d2[j] / rr[j][j]);	// remove sqrt()
		//accum[j] = sqrt(d2[j] * d2[j]);
	    }
	    else {
		accum[j] = 1e38f;
	    }
	}

    }
	/******************************************************************
	     Stage #1:	Pre-pre-select
	 	Select number of candidate positions for the first 3 pulses
		by searching for the positions with largest d6[] values.
	******************************************************************/
    for (k = 0; k < N3; k++) {
	for (ps = -FLT_MAX, i = 0, j = 0; j < l_subfr; j++) {
	    if (accum[j] > ps) {
		ps = accum[j];
		i = j;
	    }
	}
	accum[i] = -FLT_MAX;
	ordered_pos[k] = i;
    }

	/******************************************************************
	     Stage #2:	Pre-select
	 	Select candidate combinations of pulse 1, 2 and 3
		by searching for the highest gain.
	******************************************************************/
    for (i = 0; i < N_PRE; i++) {
	alpn[i] = 1.0;
	sqn[i] = -1.0;
    }

    alpk = 1.0;
    sqk = -1.0;
    pos = 0;

    ia = 0;


    for (i = 0; i < N0; i++) {
	i0 = ordered_pos[i];

	for (j = i + 1; j < N1; j++) {
	    i1 = ordered_pos[j];
	    ps0 = d2[i0] + d2[i1];
	    if (data_packet.WB_MODE_BIT == 1) {

		alp0 = rr0[i0] + rr0[i1] + rr[i0][i1];

	    }
	    else
		alp0 =
		    0.5 * rr0[0] + 0.5 * rr0[0] +
		    rr0[abs (i0 - i1)] * sign[i0] * sign[i1];

	    for (k = j + 1; k < N2 - i; k++) {
		i2 = ordered_pos[k];
		ps1 = ps0 + d2[i2];
		if (data_packet.WB_MODE_BIT == 1) {

		    alp1 = alp0 + rr0[i2] + rr[i0][i2] + rr[i1][i2];

		}
		else
		    alp1 =
			alp0 + 0.5 * rr0[0] +
			rr0[abs (i0 - i2)] * sign[i0] * sign[i2] +
			rr0[abs (i1 - i2)] * sign[i1] * sign[i2];

		for (n = k + 1; n < N3 - j; n++) {
		    i3 = ordered_pos[n];
		    ps = ps1 + d2[i3];
		    if (data_packet.WB_MODE_BIT == 1) {

			alp =
			    alp1 + rr0[i3] + rr[i0][i3] + rr[i1][i3] +
			    rr[i2][i3];

		    }
		    else
			alp =
			    alp1 + 0.5 * rr0[0] +
			    rr0[abs (i0 - i3)] * sign[i0] * sign[i3] +
			    rr0[abs (i1 - i3)] * sign[i1] * sign[i3] +
			    rr0[abs (i2 - i3)] * sign[i2] * sign[i3];

		    sq = ps * ps;


		    if (sq * alpk > alp * sqk) {

			sqn[ia] = sq;
			alpn[ia] = alp;
			vect3[ia][0] = i0;
			vect3[ia][1] = i1;
			vect3[ia][2] = i2;
			vect3[ia][3] = i3;

			alpk = alpn[0];
			sqk = sqn[0];
			ia = 0;
			for (ib = 1; ib < N_PRE; ib++) {
			    if (sqn[ib] * alpk < alpn[ib] * sqk) {
				ia = ib;
				alpk = alpn[ib];
				sqk = sqn[ib];
			    }
			}
		    }

		}
	    }
	}
    }


	/*********************************************************************
	    Stage #3:	Search
		Search the rest of the pulses over the candidate combinations
		of the first 3 pulses.
	*********************************************************************/
    alpk = 1.0;
    sqk = -1.0;
    psk = 0.0;

    for (i = 0; i < N_PRE; i++) {
	i0 = vect3[i][0];
	i1 = vect3[i][1];
	i2 = vect3[i][2];
	i3 = vect3[i][3];

	alp0 = alpn[i];
	ps0 = d2[i0] + d2[i1] + d2[i2] + d2[i3];
	if (data_packet.WB_MODE_BIT == 1) {
	    for (j = 0; j < l_subfr; j++)
		accum[j] = rr[i0][j] + rr[i1][j] + rr[i2][j] + rr0[j];

	}
	else
	    for (j = 0; j < l_subfr; j++)
		accum[j] =
		    rr0[abs (i0 - j)] * sign[i0] * sign[j] +
		    rr0[abs (i1 - j)] * sign[i1] * sign[j] +
		    rr0[abs (i2 - j)] * sign[i2] * sign[j] + 0.5 * rr0[0];

	ia = i3;
	int jmax;

	if (data_packet.WB_MODE_BIT == 1) {

	    jmax = N_PULSES_WB - 4;

	}
	else
	    jmax = 3;
	for (j = 0; j < jmax; j++) {

	    alp = 1.0;
	    sq = -1.0;
	    if (data_packet.WB_MODE_BIT == 1) {

		for (k = 0; k < l_subfr; k++)
		    accum[k] += rr[ia][k];

	    }
	    else
		for (k = 0; k < l_subfr; k++)
		    accum[k] += rr0[abs (ia - k)] * sign[ia] * sign[k];


	    for (k = 0; k < l_subfr; k++) {
		ps2 = ps0 + d2[k];
		alp2 = alp0 + accum[k];
		sq2 = ps2 * ps2;

		if (alp * sq2 > sq * alp2) {
		    sq = sq2;
		    ps = ps2;
		    alp = alp2;
		    jvect[j] = k;
		}
	    }

	    ia = jvect[j];
	    ps0 = ps;
	    alp0 = alp;
	}

	if (alpk * sq > sqk * alp) {
	    sqk = sq;
	    psk = ps;
	    alpk = alp;

	    for (j = 0; j < 4; j++)
		ivect[j] = vect3[i][j];
	    if (data_packet.WB_MODE_BIT == 1) {

		for (j = 0; j < N_PULSES_WB - 4; j++)
		    ivect[j + 4] = jvect[j];

	    }
	    else {
		for (j = 0; j < 3; j++)
		    ivect[j + 4] = jvect[j];
	    }
	}
    }

    i0 = ivect[0];
    i1 = ivect[1];
    i2 = ivect[2];
    i3 = ivect[3];
    i4 = ivect[4];
    if (data_packet.WB_MODE_BIT == 1) {

	if (N_PULSES_WB == 6) {
	    i5 = ivect[5];
	}
    }
    else {
	i5 = ivect[5];
	i6 = ivect[6];

    }

    if (data_packet.WB_MODE_BIT == 1) {

	for (i = 0; i < l_subfr; i++) {
	    accum[i] = rr0[i];
	    for (j = 0; j < N_PULSES_WB; j++) {
		accum[i] += rr[ivect[j]][i];
	    }
	}
    }
    else {
	for (i = 0; i < l_subfr; i++)
	    accum[i] =
		0.5 * rr0[0] + rr0[abs (i0 - i)] * sign[i0] * sign[i] +
		rr0[abs (i1 - i)] * sign[i1] * sign[i] +
		rr0[abs (i2 - i)] * sign[i2] * sign[i] +
		rr0[abs (i3 - i)] * sign[i3] * sign[i] +
		rr0[abs (i4 - i)] * sign[i4] * sign[i] +
		rr0[abs (i5 - i)] * sign[i5] * sign[i] +
		rr0[abs (i6 - i)] * sign[i6] * sign[i];

    }
    for (i = 0; i < N_POST; i++) {
	if (data_packet.WB_MODE_BIT == 1) {

	    ia = ivect[i % N_PULSES_WB];

	}
	else
	    ia = ivect[i % 7];

	ps0 = psk - d2[ia];
	if (data_packet.WB_MODE_BIT == 1) {

	    alp0 = alpk - (accum[ia] - rr[ia][ia]);

	}
	else {
	    alp0 = alpk - (accum[ia] - rr0[0]);
	}


	for (j = 0; j < l_subfr; j++) {
	    ps2 = ps0 + d2[j];
	    if (data_packet.WB_MODE_BIT == 1) {

		alp2 = alp0 + (accum[j] - rr[ia][j]);

	    }
	    else
		alp2 =
		    alp0 + (accum[j] -
			    rr0[abs (ia - j)] * sign[ia] * sign[j]);

	    sq2 = ps2 * ps2;

	    if (alpk * sq2 > sqk * alp2) {
		sqk = sq2;
		psk = ps2;
		alpk = alp2;
		if (data_packet.WB_MODE_BIT == 1)
		    ivect[i % N_PULSES_WB] = j;
		else
		    ivect[i % 7] = j;
	    }
	}
	if (data_packet.WB_MODE_BIT == 1) {

	    ib = ivect[i % N_PULSES_WB];

	}
	else
	    ib = ivect[i % 7];

	if (ib != ia) {
	    if (data_packet.WB_MODE_BIT == 1) {

		for (j = 0; j < l_subfr; j++)
		    accum[j] += (rr[ib][j] - rr[ia][j]);

	    }
	    else
		for (j = 0; j < l_subfr; j++)
		    accum[j] +=
			(rr0[abs (ib - j)] * sign[ib] * sign[j] -
			 rr0[abs (ia - j)] * sign[ia] * sign[j]);
	}
    }

    i0 = ivect[0];
    i1 = ivect[1];
    i2 = ivect[2];
    i3 = ivect[3];
    i4 = ivect[4];
    if (data_packet.WB_MODE_BIT == 1) {

	if (N_PULSES_WB == 6)
	    i5 = ivect[5];
    }
    else {
	i5 = ivect[5];
	i6 = ivect[6];

    }

	/*--------------------------------------------------------------------*
  	* BUILD THE CODEWORD, THE FILTERED CODEWORD AND INDEX OF CODEVECTOR. *
  	*--------------------------------------------------------------------*/

    for (i = 0; i < L_CODE; i++) {
	ck[i] = 0.0;
	y[i] = 0.0;
    }
    *gain = 0.5 * psk / alpk;

    int kmax;

    if (data_packet.WB_MODE_BIT == 1)
	kmax = N_PULSES_WB;
    else
	kmax = N_PULSES;

    for (k = 0; k < kmax; k++) {
	pos = ivect[k];
	ps = sign[pos];
	ck[pos] += ps;

	for (i = pos, ptr = h; i < L_CODE; i++)
	    y[i] += ps * (*ptr++);
    }

    if (data_packet.WB_MODE_BIT == 1) {

	codeword = factorial_pack (l_subfr, ck);

    }
    else
	codeword = factorial_pack (54, ck);


    indx[0] = (unsigned int) (codeword & 0x000000ffL);
    indx[1] = (unsigned int) (codeword >> 8 & 0x000000ffL);
    indx[2] = (unsigned int) (codeword >> 16 & 0x000000ffL);

    if (data_packet.WB_MODE_BIT == 1) {

	indx[3] = (unsigned int) (codeword >> 24 & 0x000000ffL);

    }
    else {
	indx[3] = (unsigned int) (codeword >> 24 & 0x000000ffL);
	indx[3] += (unsigned int) ((codeword >> 32 & 0x00000007L) << 8);

    }

    return;
}






void
FGV_MEM::dec_fpmp (int *index, float *cb_out, int wideband_mode)
{
    int64 code;
    int tmp_cb[MAX_TLEN];
    int indx;

    int N_PULSES_WB;

    if (wideband_mode == 1) {
	if (da_index[sfcnt] == 0) {
	    N_PULSES_WB = 6;
	}
	else {
	    N_PULSES_WB = 5;
	}

    }				/*else
				   N_PULSES =7;      */
    sfcnt = ++sfcnt % NoOfSubFrames;


    if (wideband_mode == 1) {

	code = (index[3] & 0x000000ffL);

	code = (code << 8) + (index[2] & 0x000000ffL);
	code = (code << 8) + (index[1] & 0x000000ffL);
	code = (code << 8) + (index[0] & 0x000000ffL);

	if (sfcnt == 0)
	    factorial_unpack (code, 54, N_PULSES_WB, tmp_cb);
	else
	    factorial_unpack (code, 53, N_PULSES_WB, tmp_cb);

	for (indx = 54; indx < MAX_TLEN; indx++)
	    tmp_cb[indx] = 0;	//initializing to prevent UMR in decoder
	for (indx = 0; indx < MAX_TLEN; indx++)
	    cb_out[indx] = (float) tmp_cb[indx];
	for (indx = MAX_TLEN; indx < L_CODE; indx++)
	    cb_out[indx] = 0.0;

    }
    else			//for narrowband case
    {
	code =
	    (((index[3] >> 8) & 0x00000007L) << 8) + (index[3] & 0x000000ffL);
	code = (code << 8) + (index[2] & 0x000000ffL);
	code = (code << 8) + (index[1] & 0x000000ffL);
	code = (code << 8) + (index[0] & 0x000000ffL);

	factorial_unpack (code, 54, N_PULSES, tmp_cb);
	for (indx = 54; indx < MAX_TLEN; indx++)
	    tmp_cb[indx] = 0;	//initializing to prevent UMR in decoder
	for (indx = 0; indx < MAX_TLEN; indx++)
	    cb_out[indx] = (float) tmp_cb[indx];
	for (indx = MAX_TLEN; indx < L_CODE; indx++)
	    cb_out[indx] = 0.0;


    }


}


int64 Ioff_S (int Nd, int Np, int len);
int64
get_code_from_indices (int *index)
{
    int64 code;

    code = (index[3] & 0x000000ffL);
    code = (code << 8) + (index[2] & 0x000000ffL);
    code = (code << 8) + (index[1] & 0x000000ffL);
    code = (code << 8) + (index[0] & 0x000000ffL);

    return (code);
}

/* Should be modified to actually store output of  get_code_from_indices()
 * into struct internal variable to avoid decoding each factorial code
 * twice.
 */
int
FGV_MEM::check_fpmp_code (int *index, int sfnum)
{
    int64 code;
    int code_err;
    int sflen;

    int N_PULSES_WB;

    if (da_index[sfnum] == 0) {
	N_PULSES_WB = 6;
    }
    else {
	N_PULSES_WB = 5;
    }

    code = get_code_from_indices (index);

    sfnum = (sfnum + 1) % 3;	//Why does sfcnt start at one????

    if (sfnum == 0)
	sflen = 54;
    else
	sflen = 53;

    if (code < Ioff_S (0, N_PULSES_WB, sflen)) {
	code_err = 0;
    }
    else {
	code_err = 1;
    }
    return (code_err);
}
