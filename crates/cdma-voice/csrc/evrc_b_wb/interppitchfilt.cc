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
//###########################################################################
//
//  File: interppitchfilt.cc
//  Author: James P. Ashley
//  Copyright (C) 1999,2000 Motorola, Inc. All rights reserved.
//
//###########################################################################
// CVS $Revision: 1.3.18.5 $
// Revision $Date: 2007/06/24 06:43:10 $
//###########################################################################

/*###########################################################################
 # Includes
 ###########################################################################*/
#include "macro.h"
#include "hnw.h"

//###########################################################################
// Prototypes
//###########################################################################

#define INTRP_PI 3.14159265358979323846

#define PPF_BLPREC     BLPRECISION
#define RRES_SHFT      3
#define RRES_MASK      0x0007

/*  ways to implement pitch filtering:
   1) ACB method --   recirculate pitch with unity pitch gain, apply ACB
                      gain later.
                      i.e., P(z) = 1/(1-z^(-L)), where L is time varying
   2) Pitch filter -- recirculate pitch in IIR fashion using fixed gain
                      coefficient (typically 0<=beta<1.0)
                      i.e., P(z) = 1/(1-beta*z^(-L)), where L is time varying
   3) HNW filter --   retrieve pitch samples in FIR fashion where HNW state
                      is not influenced by output of filter.
                      i.e., H(z) = 1+beta*z^(-L), where L is fixed
*/



static float IPF_Table[RRESOLUTION * (2 * Max (Max (BLPRECISION, HNW_BLPREC), PPF_BLPREC) + 1)];	/* Largest practical size */
static float IPF_TableHNW[RRESOLUTION * (2 * Max (Max (BLPRECISION, HNW_BLPREC), PPF_BLPREC) + 1)];	/* Largest practical size */
static void
init_coefficientsHNW (float factor, int fl)
{
    int n, i;
    float arg1, arg3;
    float denom, tt;
    int offset;

    static int first = 1;

    if (first) {
	first = 0;

	offset = 0;
	denom = 2.0 / (2.0 * fl + 1.0);
	for (i = 0; i < RRESOLUTION; i++) {
	    tt = (float) (i - RRESOLUTION / 2 + 1) / (float) RRESOLUTION;
	    for (n = -fl; n <= fl; n++) {
		arg1 = INTRP_PI * factor * (tt + n);
		arg3 = INTRP_PI * (tt + n);

		if (arg1 < 1.e-2 && arg1 > -1.e-2) {	/* just copy */
		    IPF_TableHNW[offset++] = factor;
		}
		else {
		    /* sinc function multiplied by HANN window */
		    IPF_TableHNW[offset++] =
			factor * (0.5 +
				  0.5 * cos (arg3 * denom)) * sin (arg1) /
			arg1;
		}
	    }
	}

    }

}

static void
init_coefficients (float factor, int fl)
{
    int n, i;
    float arg1, arg3;
    float denom, tt;
    int offset;

    static int first = 1;

    if (first) {
	first = 0;

	offset = 0;
	denom = 2.0 / (2.0 * fl + 1.0);
	for (i = 0; i < RRESOLUTION; i++) {
	    tt = (float) (i - RRESOLUTION / 2 + 1) / (float) RRESOLUTION;
	    for (n = -fl; n <= fl; n++) {
		arg1 = INTRP_PI * factor * (tt + n);
		arg3 = INTRP_PI * (tt + n);

		if (arg1 < 1.e-2 && arg1 > -1.e-2) {	/* just copy */
		    IPF_Table[offset++] = factor;
		}
		else {
		    /* sinc function multiplied by HANN window */
		    IPF_Table[offset++] =
			factor * (0.5 +
				  0.5 * cos (arg3 * denom)) * sin (arg1) /
			arg1;
		}
	    }
	}
    }
}


void
ACB_interp (float *signal, float *delay, int length)
{
    int i, n;
    float denom;
    float locdelay;
    float invsubframel;
    int offset;
    float *f, *coef_ptr;
    short t;
    short delayq, delayq_last;
    float accA;

    static int first = 1;

    if (first) {
	init_coefficients (BLFREQ, BLPRECISION);
	first = 0;
    }

    invsubframel = 1.0 / ((float) length);
    denom = (delay[1] - delay[0]) * invsubframel;
    delayq_last = 0;
    for (i = 0; i < length; i++) {
	locdelay = delay[0] + i * denom;
	delayq =
	    (short) (locdelay * RRESOLUTION + 0.5) + (RRESOLUTION / 2 - 1);
	if (delayq != delayq_last) {
	    offset = delayq >> RRES_SHFT;

	    t = delayq & RRES_MASK;
	    /* center sum around f */
	    f = &signal[i] - offset - BLPRECISION;
	    coef_ptr = IPF_Table + t * (2 * BLPRECISION + 1);
	    delayq_last = delayq;
	}
	else {
	    f++;		//Free ptr update
	}
	accA = 0.0;
	for (n = 0; n < (2 * BLPRECISION + 1); n++) {
	    accA += coef_ptr[n] * f[n];
	}
	signal[i] = accA;
    }

}



void
PPF_interp (float *signal, float delay[], float gppf, int length)
{
    int i, n;
    float denom;
    float locdelay;
    float invsubframel;
    int offset;
    float *f, *coef_ptr;
    short t;
    short delayq, delayq_last;
    float xt;

    static int first = 1;

    if (first) {
	init_coefficients (BLFREQ, BLPRECISION);
	first = 0;
    }

    invsubframel = 1.0 / ((float) length);
    denom = (delay[1] - delay[0]) * invsubframel;
    delayq_last = 0;
    for (i = 0; i < length; i++) {
	locdelay = delay[0] + i * denom;
	delayq =
	    (short) (locdelay * RRESOLUTION + 0.5) + (RRESOLUTION / 2 - 1);
	if (delayq != delayq_last) {
	    offset = delayq >> RRES_SHFT;
	    t = delayq & RRES_MASK;
	    /* center sum around f */
	    f = &signal[i] - offset - BLPRECISION;
	    coef_ptr = IPF_Table + t * (2 * BLPRECISION + 1);
	    delayq_last = delayq;
	}
	else {
	    f++;		// Free ptr update
	}
	xt = 0.0;
	for (n = 0; n < (2 * BLPRECISION + 1); n++) {
	    xt += coef_ptr[n] * f[n];
	}
	signal[i] += gppf * xt;
    }

}



void
HNW_interp (float *input, float *output, float locdelay, int length)
{
    int i, n;
    int offset;
    float *f, *coef_ptr, xt;
    short t;
    short delayq;

    static int first = 1;

    if (first) {

	float hnw_blfreq = HNW_BLFREQ;

	init_coefficientsHNW (hnw_blfreq, BLPRECISION);
	first = 0;
    }

    delayq = (short) (locdelay * RRESOLUTION + 0.5) + (RRESOLUTION / 2 - 1);
    offset = delayq >> RRES_SHFT;
    t = delayq & RRES_MASK;
    /* center sum around f */
    f = input - offset - BLPRECISION;
    coef_ptr = IPF_TableHNW + t * (2 * BLPRECISION + 1);
    for (i = 0; i < length; i++) {
	xt = 0.0;
	for (n = 0; n < (2 * BLPRECISION + 1); n++) {
	    xt += coef_ptr[n] * f[n];
	}
	output[i] = xt;
	f++;			// Free ptr update
    }

}
