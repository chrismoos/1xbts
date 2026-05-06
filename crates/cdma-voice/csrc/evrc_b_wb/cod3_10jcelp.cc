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

#include <float.h>
#include <math.h>
#include <stdio.h>
#include "macro.h"
#include "acelp.h"
#include "struct.h"
#include "globs.h"


static int index0[22] = { 1, 3, 6, 8, 10, 13, 15, 18, 20, 22, 25,
    27, 30, 32, 34, 37, 39, 42, 44, 46, 49, 51
};

static int index1[23] = { 0, 2, 4, 7, 9, 12, 14, 16, 19, 21, 24,
    26, 28, 31, 33, 36, 38, 40, 43, 45, 48, 50, 52
};



void
FGV_MEM::cod3_10jcelp (float xn[], float h[],	/* input : impulse response of weighted synthesis filter */
		       int pit_lag, float pit_gain, int l_subfr,	/* input : lenght of subframe                            */
		       float cod[],	/* output: algebraic (fixed) codebook excitation         */
		       float *gain_code, int *indices	/* output: indices of 3 pulses (10 bits, 1 word)         */
    )
{
    int i0, i1, i2;
    int i, j, k, pos, dec, index, step, max, flag;
    int codvec[3];
    float psk = -1, ps1, ps2, sq2, sqk;
    float alpk, alp1, alp2;
    float ps, s, r, sign, rr[VCM_L0];
    float dn[VCM_L0];


    //int cod3_10_offset[3] = {0, 2, 4};


    int pos_table[512][3], ix;


    /* Construct JCELP configuration */
    flag =
	JCELP_Config (pit_lag, pit_gain, l_subfr, pos_table, cod3_10_offset,
		      &step, &max, &sign, &r);

    if (flag == 2)
	for (i = pit_lag; i < l_subfr; i++)
	    h[i] += h[i - pit_lag];

    corr_xh (xn, dn, h, l_subfr /**VCM_L**/ );
    for (i = l_subfr; i < VCM_L0; i++)
	dn[i] = 0.0;		/* Put dn[i] = 0 for i>l_subfr */


	/*----------------------------------------------------------------*
 	 *        BUILD "rr[][]", MATRIX OF AUTOCORRELATION.              *
	 *----------------------------------------------------------------*/
    //Doing regular diag elements
    for (k = 0, ps = 0.0; k < l_subfr; k++) {
	ps += h[k] * h[k];
    }
    rr[0] = ps;			//note rr0 has the first row of matrix instead of 
    //     of diagonal in EVRC

    //Make first row
    for (i = 1; i < l_subfr; i++) {
	for (k = 0, ps = 0.0; k < (l_subfr - i); k++) {
	    ps += h[k] * h[k + i];
	}
	rr[i] = ps;
    }

    //  need to initialized out of subframe values
    for (i = l_subfr; i < VCM_L0; i++)
	rr[i] = 0;		/* Put rr[i] = 0 for i>l_subfr */

	/*----------------------------------------------------------------*
 	 *              SEARCH THE BEST CODEVECTOR.                       *
 	 *----------------------------------------------------------------*/
    sqk = -1.0;
    alpk = 1.0;
    ix = 0;

    if (flag > 0) {		/* Search 3 pulses */
	for (i = 0; i < 512; i++) {
	    i0 = pos_table[i][0];
	    i1 = pos_table[i][1];
	    i2 = pos_table[i][2];

	    ps1 = dn[i0] - dn[i1];
	    alp1 = rr[0] + rr[0] - 2.0 * rr[abs (i0 - i1)];

	    ps2 = ps1 + dn[i2];
	    alp2 =
		alp1 + rr[0] + 2.0 * (rr[abs (i0 - i2)] - rr[abs (i1 - i2)]);
	    sq2 = ps2 * ps2;

	    if ((alpk * sq2) > (sqk * alp2)) {
		sqk = sq2;
		alpk = alp2;
		psk = ps2;
		ix = i;
	    }
	}
    }
    else {			/* Search 2 pulses */
	for (i = 0; i < 22; i++) {
	    i0 = index0[i];
	    i2 = i0 + cod3_10_offset[1];

	    ps1 = dn[i0] + r * dn[i2];
	    alp1 = rr[0] + r * (r * rr[0] + 2.0 * rr[abs (i0 - i2)]);

	    for (j = 0; j < 23; j++) {
		i1 = index1[j];

		ps2 = ps1 + sign * dn[i1];
		alp2 =
		    alp1 + rr[0] + 2.0 * sign * (rr[abs (i0 - i1)] +
						 r * rr[abs (i2 - i1)]);
		sq2 = ps2 * ps2;

		if ((alpk * sq2) > (sqk * alp2)) {
		    sqk = sq2;
		    alpk = alp2;
		    psk = ps2;
		    codvec[0] = i0;
		    codvec[1] = i1;
		    codvec[2] = i2;
		}
	    }
	}
    }

	/*--------------------------------------------------------------------*
 	 * BUILD THE CODEWORD, THE FILTERED CODEWORD AND INDEX OF CODEVECTOR. *
 	 *--------------------------------------------------------------------*/

    for (i = 0; i < VCM_L0; i++)
	cod[i] = 0.0;

    if ((*gain_code = psk / alpk) < 0) {
	*gain_code = -(*gain_code);
	s = -1.0;
	index = 1;
    }
    else {
	s = 1.0;
	index = 0;
    }

    if (flag > 0) {
	for (i = 0; i < 3; i++) {
	    pos = pos_table[ix][i];
	    cod[pos] += s;
	    s = -s;
	}
	index <<= 9;
	index += ix;
    }
    else {
	pos = codvec[0];
	cod[pos] += s;
	cod[pos + cod3_10_offset[1]] += s * r;
	i0 = (pos - pos / 6) / 2;

	pos = codvec[1];
	cod[pos] += s * sign;
	i1 = (pos - pos / 6) / 2;

	index = (index << 9) + i0 * 23 + i1;
    }

    *indices = index;

    if (flag == 2)
	for (i = pit_lag; i < l_subfr; i++)
	    cod[i] += cod[i - pit_lag];


    return;
}

/* Construct VCM Configuration */
int
JCELP_Config (int lag,		/* input */
	      float gain, int l_subfr, int pos[512][3],	/* output */
	      int offset[], int *step, int *max, float *sign, float *ratio)
{
    static float skew_table1[31] = {
	1, -1, -2, -3, 0, 0, 3, -2,
	0, -1, -3, -3, 1, 2, 0, -3,
	1, 0, -1, 0, 0, 0, 0, 0,
	0, 0, 0, 0, 0, 0, 0
    };

    static float skew_table0[31] = {
	0, -2, -3, -1, 1, 0, -2, -3,
	-1, -1, 2, 0, 2, 0, 3, -3,
	0, 3, -3, 0, 0, 0, 0, 0,
	0, 0, 0, 0, 0, 0, 0
    };

    int t0, t1, t2;
    int l0, l1, l2;
    int span, rem, iskew;
    double skew0, skew1;
    double space, inc, offs;
    int i, return_code;


    float tempgain = ((int) (gain * 16384 + 0.5)) / 16384.;

    gain = tempgain;


    if ((lag <= VCM_UDU_PIT) || (lag < VCM_UD_PIT && gain <= PIT_THRSH)) {

	if (gain > PIT_THRSH && lag < l_subfr) {
	    if (lag < 24)
		span = 24;
	    else
		span = lag;
	    return_code = 2;
	}
	else {
	    span = l_subfr;
	    return_code = 1;
	}


	/* fill position table */
	t0 = span / 3;
	t1 = t0;
	t2 = t0;
	rem = span - t0 - t1 - t2;
	if (rem == 2) {
	    t0++;
	    t1++;
	}
	else if (rem == 1) {
	    t0++;
	}

	space = (double) t0 *t1 * t2;

	inc = space * (1.0 / 512.0);
	offs = inc * (1.0 / 2.0);
	iskew = 54 - span;
	skew0 = (double) skew_table0[iskew];
	skew1 = (double) skew_table1[iskew];

	for (i = 0; i < 512; i++) {
	    l0 = (int) (((i * inc) + offs) / (t2 * t1));
	    l0 %= t0;

	    l1 = (int) (((i * inc) + offs) / t2);
	    l1 %= t1;

	    l2 = (int) (((i * inc) + offs) + l0 * skew0 + l1 * skew1);

	    l2 %= t2;

	    pos[i][0] = 3 * l0 + 0;
	    pos[i][1] = 3 * l1 + 1;
	    pos[i][2] = 3 * l2 + 2;

	}

	return (return_code);

    }
    else {
	if (lag >= VCM_UD_PIT) {
	    *sign = 1.0;
	    *ratio = VCM_FAC * (lag - VCM_UD_PIT);
	    if (*ratio < 0.1)
		*ratio = 0.1;
	    if (*ratio > 0.8)
		*ratio = 0.8;
	    offset[1] = (lag < 100) ? 1 : VCM_UuU_OVERSTEP;
	}
	else {
	    *sign = -1.0;
	    *ratio = 0.0;
	    offset[1] = 0;
	}
	return (0);
    }
}
