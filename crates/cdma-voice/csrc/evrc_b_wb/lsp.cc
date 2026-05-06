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
    "$Id: lsp.cc,v 1.9.22.4 2007/06/24 06:43:12 apadmana Exp $";
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

#include "macro.h"
#define STEPSNUM 4


/*===================================================================*/
/* FUNCTION      :  LPC_chebyshev ().                                */
/*-------------------------------------------------------------------*/
/* PURPOSE       :  Evaluate a series expansion in Chebyshev         */
/*                  polynomials.                                     */
/*                                                                   */
/*  The polynomial order is                                          */
/*     n = m/2   (m is the prediction order)                         */
/*  The polynomial is given by                                       */
/*    C(x) = T_n(x) + f(1)T_n-1(x) + ... +f(n-1)T_1(x) + f(n)/2      */
/*                                                                   */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS:                                                  */
/*     _ (float   ) x:     value of evaluation; x=cos(freq).         */
/*     _ (float []) f:     coefficients of sum or diff polynomial.   */
/*     _ (int     ) n:     order of polynomial.                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS:                                                 */
/*     _ None.                                                       */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*     _ None.                                                       */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*     _ (float   ) val:   the value of the polynomial C(x).         */
/*===================================================================*/


float
LPC_chebyshev (float x, float f[], int n)
{

    float b1, b2, b0, x2, val;
    int i;

    x2 = 2.0 * x;		/* x2 = 2.0*x;                   */
    b2 = 1.0;
    b1 = x2 + f[0];

    for (i = 2; i < n; i++) {
	b0 = x2 * b1 - b2 + f[i - 1];
	b2 = b1;
	b1 = b0;
    }

    val = (x * b1 - b2 + f[i - 1]);

    return val;

}



/*----------------------------------------------------------------------*/
/*  Module:     a2lsp                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*      1/1/95   Written by Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/*
 * pctolsp - convert pc to lsp
 *
 * NOTES: 1. This routine is hardwired for 10th order
 *        2. The routine uses 3 stage uniform grid quantization of lsp.
 */


void
a2lsp (float *freq, float *a, short order)
{
    static float STEPS[4] = { 0.00635, 0.003175, 0.0015875, 0.00079375 };	/* rom storage */
    int lspnumber;
    int root, notlast;
    float temp, temp0, temp1, temp2;
    float *q;
    float prev[2];
    int offset;
    int iswitch, i;
    float frequency, LastFreq;
    float STEP;
    int STEPindex;

    LastFreq = 0;
    q = (float *) calloc (order, sizeof (float));


    /* calculate q[z] and p[z] , they are all stored in q */
    offset = order / 2;
    q[0] = a[0] + a[order - 1] - 1.0;
    q[offset] = a[0] - a[order - 1] + 1.0;
    for (i = 1; i < order / 2; i++) {
	q[i] = a[i] + a[order - 1 - i] - q[i - 1];
	q[i + offset] = a[i] - a[order - 1 - i] + q[i - 1 + offset];
    }

    q[(order / 2) - 1] = q[(order / 2) - 1] / 2;
    q[(order / 2) - 1 + offset] = q[(order / 2) - 1 + offset] / 2;

    prev[0] = 9e9;
    prev[1] = 9e9;
    lspnumber = 0;
    notlast = TRUE;
    iswitch = 0;
    frequency = 0;


    while (notlast) {

	root = TRUE;
	offset = iswitch * (order / 2);
	STEPindex = 0;		/* Start with low resolution grid */
	STEP = STEPS[STEPindex];

	while (root) {
	    temp = cos (frequency * 6.2832);
	    if (order >= 4)
		temp2 = LPC_chebyshev (temp, q + offset, order / 2);
	    else
		temp2 = temp + q[0 + offset];

	    if ((temp2 * prev[iswitch]) <= 0.0 || frequency >= 0.5) {
		if (STEPindex == STEPSNUM - 1) {
		    if (fabs (temp2) < fabs (prev[iswitch])) {
			freq[lspnumber] = frequency;
		    }
		    else {
			freq[lspnumber] = frequency - STEP;
		    }

		    if ((prev[iswitch]) < 0.0) {
			prev[iswitch] = 9e9;
		    }
		    else {
			prev[iswitch] = -9e9;
		    }
		    root = FALSE;
		    frequency = LastFreq;
		    STEPindex = 0;
		}
		else {
		    if (STEPindex == 0) {
			LastFreq = frequency;
		    }
		    frequency -= STEPS[++STEPindex];	/* Go back one grid step */
		    STEP = STEPS[STEPindex];
		}
	    }
	    else {
		prev[iswitch] = temp2;
		frequency += STEP;
	    }
	}

	lspnumber++;
	if (lspnumber > order - 1)
	    notlast = FALSE;
	iswitch = 1 - iswitch;
    }
    free (q);
}




/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     lsp2a                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: lsptopc.                                                *
* Function: Convert lsp to pc.                                          *
* Inputs: freq - Quantized line spectral frequencies.                   *
* Outputs: pc - prediction coefficients.                                *
************************************************************************/
void
lsp2a (float *pc, float *freq, short order)
{
    float *p, *q;
    float *a, *a1, *a2;
    float *b, *b1, *b2;
    float xx, xf;
    int i, k;
    int lspflag;

    lspflag = 1;

    p = (float *) calloc ((int) order / 2, sizeof (float));
    q = (float *) calloc ((int) order / 2, sizeof (float));

    a = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    a1 = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    a2 = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    b = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    b1 = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    b2 = (float *) calloc ((((int) order / 2) + 1), sizeof (float));
    /*    *** check input for ill-conditioned cases */
    if ((freq[0] <= 0.0) || (freq[order - 1] >= 0.5)) {
	lspflag = 0;
	if (freq[0] <= 0)
	    freq[0] = 0.022;
	if (freq[order - 1] >= 0.5)
	    freq[order - 1] = 0.499;
    }

    if (!lspflag) {
	xx = (freq[order - 1] - freq[0]) / (float) order;
	for (i = 1; i < order; i++)
	    freq[i] = freq[i - 1] + xx;
    }

    for (i = 0; i <= order / 2; i++) {
	a[i] = 0.;
	a1[i] = 0.;
	a2[i] = 0.;
	b[i] = 0.;
	b1[i] = 0.;
	b2[i] = 0.;
    }

    for (i = 0; i < order / 2; i++) {
	p[i] = cos (6.2832 * freq[2 * i]);
	q[i] = cos (6.2832 * freq[2 * i + 1]);
    }

    xf = 0.;
    xx = .25;

    for (k = 0; k <= order; k++) {
	a[0] = xx + xf;
	b[0] = xx - xf;
	xf = xx;
	xx = 0;

	for (i = 0; i < order / 2; i++) {
	    a[i + 1] = a[i] - 2 * p[i] * a1[i] + a2[i];
	    b[i + 1] = b[i] - 2 * q[i] * b1[i] + b2[i];
	    a2[i] = a1[i];
	    a1[i] = a[i];
	    b2[i] = b1[i];
	    b1[i] = b[i];
	}
	if (k != 0) {
	    pc[k - 1] = 2 * (a[order / 2] + b[order / 2]);
	}
    }
    free (p);
    free (q);
    free (a);
    free (a1);
    free (a2);
    free (b);
    free (b1);
    free (b2);
}


/*===================================================================*/
/* FUNCTION      :  LPC_pred2refl ().                                */
/*-------------------------------------------------------------------*/
/* PURPOSE       :  This function calculate the PARCOR coefficients  */
/*                  using  the prediction coeff.                     */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*             _ (float []) a         : output LP filter coeff.    */
/*             _ (int    ) LPC_order : LPC order.                  */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*             _ (float []) refl      : output reflection coeff.   */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                            _ None.                                */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                            _ None.                                */
/*===================================================================*/

void
LPC_pred2refl (float a[], float refl[], int LPC_order)
{
	 /*-------------------------------------------------------------------*/

    float f[LPC_order];

    int m, j, n;
    float km, denom, x;

	 /*-------------------------------------------------------------------*/



	 /*-------------------------------------------------------------------*/
    /*                          Initialisation                           */
	 /*-------------------------------------------------------------------*/
    for (m = 0; m < LPC_order; m++)
	f[m] = -a[m];

	 /*-------------------------------------------------------------------*/

    for (m = LPC_order - 1; m >= 0; m--) {
	km = f[m];

	if (km <= -1.0 || km >= 1.0) {
	    return;
	}

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

	 /*-------------------------------------------------------------------*/

    return;

	 /*-------------------------------------------------------------------*/
}

/*----------------------------------------------------------------------------*/
