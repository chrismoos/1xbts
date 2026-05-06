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

static char const rcsid[] =
    "$Id: lpcana.cc,v 1.11.12.3 2007/06/24 06:43:11 apadmana Exp $";
/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     lpcanalysis                                             */
/*----------------------------------------------------------------------*/

#include "macro.h"
#include "proto.h"
#include "struct.h"

/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/06/94  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/***********************************************************************
* Routine name: lpcanalys                                              *
* Function: Calculates LPC prediction coefficients from input speech.  *
***********************************************************************/

void
FGV_MEM::lpcanalys (float *pc, float *rc, float *input, short order,
		    short len, float *R, float *window)
{
    autocorrelation (R, input, len, order + 1 + 6, window);
    /*...add 6 for rate determination... */

    durbin (R, rc, pc, order);
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     autocorr                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/02/93  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/****************************************************************************
* Routine name: autocorrelation.c                                           *
* Function: Calculates autocorrelation matrix for LPC calculation.          *
****************************************************************************/
#include "globs.h"



void
FGV_MEM::autocorrelation (float *r, float *input, short len, short order,
			  float *window)
{
    register int k, i;
    float winput[LPC_ANALYSIS_WINDOW_LENGTH];


    /* Window for spectral smoothing of ac coefficients */
    /* Note: also includes WNCF */

    /* Calculate autocorrelation coefficients and store them as long integers. */
    for (k = 0; k < len; k++)
	winput[k] = input[k] * window[k];

    for (k = 0; k < order; k++)
	for (i = 0, r[k] = 0; i < len - k; i++)
	    r[k] += winput[i] * winput[i + k];

    /* Spectral smoothing of autocorrelation coefficients */
    for (k = 0; k < order; k++)
	r[k] = r[k] * w[k];
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     durbin                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/06/95  Written By Dror Nahumi, AT&T                           */
/*     09/10/96  Modified *pc_pointer inc for Sun incompatibilities (TJ)*/
/*----------------------------------------------------------------------*/

/**********************************************************************
* Routine name: durbin                                                *
* Function: Calculates prediction and reflection coefficients         *
*           from autocorrelation coefficients.                        *
**********************************************************************/


void
FGV_MEM::durbin (float *r, float *rc, float *pc, short order)
{
    register short minc, ip, i;
    float acc0, acc1, alpha;
    float *pc_pointer1, *pc_pointer2;
    float *rc_pointer;
    float *rcminc;
    short minc_half;

    if (*r <= 1.0e-10) {

	/* if r(0)<=0, set LPC coeff. to zero */
	if (r[0] != 0) {
	    if (args->verbose)
		fprintf (stderr, "Ill condition in durbin.\n");
	}
	for (ip = 0; ip < order; ip++) {
	    *pc++ = 0;
	    *rc++ = 0;
	}

    }
    else {

	/* Initialize variables before recursion */
	*rc = -(*(r + 1)) / (*r);
	*pc = *rc;
	alpha = *r + (*(r + 1)) * (*rc);
	rcminc = rc + 1;
	for (minc = 2; minc <= order; minc++) {
	    pc_pointer1 = pc;
	    rc_pointer = r + minc - 1;
	    for (i = 0, acc0 = 0; i < minc - 1; i++) {
		acc0 += (*pc_pointer1++) * (*rc_pointer--);
	    }
	    acc0 += *(r + minc);
	    *rcminc = -acc0 / alpha;
	    minc_half = minc >> 1;
	    pc_pointer1 = pc;
	    pc_pointer2 = pc + minc - 2;
	    for (ip = 1; ip <= minc_half; ip++) {
		acc1 = (*pc_pointer1) + (*rcminc) * (*pc_pointer2);
		*pc_pointer2 = (*pc_pointer2) + (*rcminc) * (*pc_pointer1);
		pc_pointer2--;
		*pc_pointer1++ = acc1;
	    }
	    *(pc + minc - 1) = *rcminc;
	    alpha += (*rcminc++) * (acc0);
	}
    }
}


void
a2rc (float *a, float *refl, short lpcorder)
{
    float *f = new float[lpcorder];

    short m, j, n;
    float km, denom, x;

    for (m = 0; m < lpcorder; m++)
	f[m] = -a[m];
    //Init
    for (m = lpcorder - 1; m >= 0; m--) {
	km = f[m];
	if (km <= -1.0 || km >= 1.0)
	    return;
	refl[m] = -km;
	denom = 1.0 / (1.0 - km * km);
	for (j = 0; j < m / 2; j++) {
	    n = m - 1 - j;
	    x = denom * f[j] + km * denom * f[n];
	    f[n] = denom * f[n] + km * denom * f[j];
	    f[j] = x;
	}
	if (m & 1)
	    f[j] = denom * f[j] + km * denom * f[j];
    }

    return;

}
