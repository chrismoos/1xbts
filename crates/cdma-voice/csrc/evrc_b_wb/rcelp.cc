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
    "$Id: rcelp.cc,v 1.13.6.4 2007/06/24 06:43:17 apadmana Exp $";



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
/*  Module:     bl_intrp                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written by Dror Nahumi,AT&T                            */
/*----------------------------------------------------------------------*/

#include "macro.h"
#include "struct.h"
/**************************************************************
* Routine name: bl_intrp                                      *
* Function: Delay of speech signal.                           *
* Inputs: input  - Input spech buffer.                        *
*         delay  - Amount of delay.                           *
*         factor - Cut of frequency.                          *
* Output: output - Delayed sample.                            *
*                                                             *
* This version was hardwired to: factor =0.9 fl=16            *
**************************************************************/

#define BL_INTRP_PI 3.141592654
void
FGV_MEM::bl_intrp (float *output, float *input, float delay, float factor,
		   short fl)
{
    register int n, i;
    register short t;
    register float *f, *coef_ptr;
    register float arg1, arg3;
    register float denom, tt;
    int offset;

    /* initialized first&2nd time only... this should be done in the
       ROM storage for implementations */
    if (factor1 == 0) {
	factor1 = factor;
	offset = 0;
	if (data_packet.WB_MODE_BIT == 1) {
	    if (data_packet.WB_MODE_BIT)
		denom = 2.0 / (2.0 * fl);
	    else
		denom = 2.0 / (2.0 * fl + 1.0);
	}
	else {
	    denom = 2.0 / (2.0 * fl + 1.0);

	}
	for (i = 0; i < RRESOLUTION; i++) {
	    tt = (float) (i - RRESOLUTION / 2) / (float) RRESOLUTION;
	    for (n = -fl; n <= fl; n++) {
		arg1 = BL_INTRP_PI * factor * (tt - n);
		arg3 = BL_INTRP_PI * (tt - n);
		if (arg1 < 1.e-2 && arg1 > -1.e-2) {	/* just copy */
		    Table[offset++] = factor;
		}
		else {		/* sinc function multiplied by hamming window */
		    if (data_packet.WB_MODE_BIT == 1) {
			if (data_packet.WB_MODE_BIT) {
			    Table[offset++] = factor *
				sqrt (0.54 +
				      0.46 * cos (arg3 * denom)) *
				sin (arg1) / arg1;
			}
			else {
			    Table[offset++] = factor *
				(0.54 +
				 0.46 * cos (arg3 * denom)) * sin (arg1) /
				arg1;
			}
		    }
		    else {
			Table[offset++] = factor *
			    (0.54 +
			     0.46 * cos (arg3 * denom)) * sin (arg1) / arg1;

		    }
		}
	    }
	}
    }
    else {
	if (factor2 == 0 && factor != factor1) {
	    factor2 = factor;
	    offset = 0;
	    if (data_packet.WB_MODE_BIT == 1) {
		if (data_packet.WB_MODE_BIT)
		    denom = 2.0 / (2.0 * fl);
		else
		    denom = 2.0 / (2.0 * fl + 1.0);
	    }
	    else {
		denom = 2.0 / (2.0 * fl + 1.0);

	    }
	    for (i = 0; i < RRESOLUTION; i++) {
		tt = (float) (i - RRESOLUTION / 2) / (float) RRESOLUTION;
		for (n = -fl; n <= fl; n++) {
		    arg1 = BL_INTRP_PI * factor * (tt - n);
		    arg3 = BL_INTRP_PI * (tt - n);
		    if (arg1 < 1.e-2 && arg1 > -1.e-2) {	/* just copy */
			Table1[offset++] = factor;
		    }
		    else {	/* sinc function multiplied by hamming window */
			if (data_packet.WB_MODE_BIT) {
			    Table1[offset++] = factor *
				sqrt (0.54 +
				      0.46 * cos (arg3 * denom)) *
				sin (arg1) / arg1;
			}
			else {
			    Table1[offset++] = factor *
				(0.54 +
				 0.46 * cos (arg3 * denom)) * sin (arg1) /
				arg1;
			}
		    }
		}
	    }
	}
    }

    /* normal run-time bl_intrp function */
    if (delay > 0)
	offset = (short) (delay + 0.5);
    else
	offset = -(short) (-delay + 0.5);

    t = (short) ((offset - delay + 0.5) * RRESOLUTION + 0.5);
    if (t == RRESOLUTION) {	/* if t = 0.5 */
	t = 0;
	offset--;
    }

    f = input - offset - fl;	/* center sum around f */

    if (factor == factor2)
	coef_ptr = Table1 + t * (2 * (short) fl + 1);
    else
	coef_ptr = Table + t * (2 * (short) fl + 1);


    *output = 0.0;
    for (n = 0; n < 2 * fl + 1; n++)
	*output += *coef_ptr++ * *f++;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     mdfyorig                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*----------------------------------------------------------------------*/






/* Modify the original residual to match the TARGET residual buffer */
#include "globs.h"
void
FGV_MEM::modifyorig (float *residualm, float *accshift, float beta,
		     short *dpm, short shiftrange, short resolution,
		     float *TARGET, float *residual, short dp, short sfend)
{
    //static short modifyorig_FirstTime = 1;
    short best;
    float y = 0, tmp;

    /* Fraction sampled correlation function */
    float ex0y[(RSHIFT * 2 + 2) * RRESOLUTION + 1];

    /* Integer sampled correlation function */
    float ex0y2[RSHIFT * 2 + 2];
    float Residual[2 * SubFrameSize + 2 * RSHIFT + 1];
    register short i, j, k, n;
    short sfstart, length, shiftrangel, shiftranger;
    float e01, e02;

	/****** INITIALIZATION *****/
    if (modifyorig_FirstTime) {
	modifyorig_FirstTime = 0;
	/* Calculate interpolation coefficients */
	tmp = 1.0 / (float) resolution;
	y = -(float) (resolution / 2) * tmp;
	for (i = 0; i < resolution; i++) {
	    modifyorig_a1[i] = (y * y - y) / 2.0;
	    modifyorig_a2[i] = (1.0 - y * y);
	    modifyorig_a3[i] = (y * y + y) / 2.0;
	    y += tmp;
	}
    }

  /********************
   * CORRELATION MATCH *
   ********************/
    length = sfend - dp;
    sfstart = dp;

    if (shiftrange != 0) {
	/* Limit the search range to control accshift */
	shiftrangel = shiftranger = shiftrange;
	if (*accshift < 0)
	    shiftrangel += 1;
	if (*accshift > 0)
	    shiftranger += 1;
	/* For non-periodic signals */
	if ((beta < 0.2 && fabs (*accshift) > 15.0)
	    || (beta < 0.3 && fabs (*accshift) > 30.0))
	    if (*accshift < 0)
		shiftranger = 1;
	    else
		shiftrangel = 1;

	if ((shiftrangel + (short) *accshift) > GUARD - BLPRECISION) {
	    shiftrangel = GUARD - BLPRECISION - (short) *accshift;
	    fprintf (stderr, "mdfyorig:*** Buffer limit. shiftrangel is:%d\n",
		     shiftrangel);
	}
	if ((shiftranger - (short) *accshift) > (GUARD - BLPRECISION)) {
	    shiftranger = (GUARD - BLPRECISION) + (short) *accshift;
	    fprintf (stderr, "mdfyorig:*** Buffer limit. shiftranger is:%d\n",
		     shiftranger);
	}

	/* Create a buffer of modify residual for match at low cut-off frequency */
	for (i = 0; i <= length + shiftrangel + shiftranger; i++)
	    bl_intrp (Residual + i, residual + dp + i,
		      *accshift + shiftrangel, 0.5, 3);

	/* Search for all integer delays of residual */
	for (n = 0; n <= shiftrangel + shiftranger; n++) {
	    ex0y2[n] = 0.0;
	    for (i = 0; i < length; i++) {
		ex0y2[n] += Residual[n + i] * TARGET[sfstart + i];
	    }
	}

	/* Do quadratic interpolation of ex0y */
	for (n = 1, k = 0; n < shiftrangel + shiftranger; n++) {
	    for (j = 0; j < resolution; j++) {
		ex0y[k++] =
		    modifyorig_a1[j] * ex0y2[n - 1] +
		    modifyorig_a2[j] * ex0y2[n] + modifyorig_a3[j] * ex0y2[n +
									   1];
	    }
	}

	/* Find maximum with positive correlation */
	y = 0.0;
	best = resolution * shiftrangel - RRESOLUTION / 2;
	for (n = 0; n < k; n++) {
	    if (ex0y[n] > y) {
		y = ex0y[n];
		best = n;
	    }
	}

	/* Calculate energy in selected shift index */
	e01 = e02 = 0.0;
	for (i = shiftrangel; i < length + shiftrangel; i++)
	    e01 += Residual[i] * Residual[i];
	for (i = 0; i < length; i++)
	    e02 += TARGET[i + sfstart] * TARGET[i + sfstart];
	if (e01 * e02 == 0)
	    y = 0.0;
	else
	    y = y / sqrt (e02 * e01);

	if (data_packet.WB_MODE_BIT == 1) {
	    if (y > 0.7)
		*accshift -=
		    (float) (best - resolution * shiftrangel +
			     RRESOLUTION / 2) / (float) resolution;

	    else
		rcelp_fail = 1;	//=====================================================================//
	}
	else {
	    if (y > 0.7)
		*accshift -=
		    (float) (best - resolution * shiftrangel +
			     RRESOLUTION / 2) / (float) resolution;

	    else
		rcelp_fail = 1;	//=====================================================================//

	}



    }






    if (data_packet.WB_MODE_BIT == 1) {
	for (k = 0; k < length; k++)
	    bl_intrp (residualm + dp + k, residual + dp + k, *accshift,
		      BLFREQ, BLPRECISION);
    }
    else {
	for (k = 0; k < length; k++)
	    bl_intrp (residualm + dp + k, residual + dp + k, *accshift,
		      BLFREQ_NB, BLPRECISION);
    }

    *dpm = dp + length;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     maxeloc                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

void
maxeloc (short *maxloc, float *maxener, float *signal, short dp, short length,
	 short ewl)
{
    float ener;
    register int i;
    int tail, front;

    ener = 0.0;
    front = dp + ewl;
    tail = dp - ewl;
    for (i = tail; i <= front; i++)
	ener += signal[i] * signal[i];
    *maxloc = dp;
    *maxener = ener;
    for (i = 1; i < length; i++) {
	front++;
	ener += signal[front] * signal[front] - signal[tail] * signal[tail];
	tail++;
	if (*maxener < ener) {
	    *maxloc = i + dp;
	    *maxener = ener;
	}
    }
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     cshift                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*----------------------------------------------------------------------*/

/* Find residual pulse frame for shifting */

#define EWL 2			/* energy window search */

void
cshiftframe (short *sfstart, short *sfend, float *maxshift2, short dpm,
	     float *residual, short guard, float accshift, float maxshift,
	     float delay, short subframel, short extra)
{
    float maxener;
    int offset;
    int iacshift;
    int length;
    short loc, loc2, g;

    if (delay < 0) {
	iacshift = (short) (-accshift + 0.5);
	iacshift = -iacshift;
    }
    else
	iacshift = (short) (-accshift + 0.5);

    /* determine first a pitch pulse somewhere near dpm */
    length = (short) (1.5 * delay);
    offset = dpm + iacshift - (short) (0.25 * delay);
    g = guard - offset - EWL;	/* Maximum allowed search size */
    if (length > g)
	length = g;
    maxeloc (&loc, &maxener, residual, offset, length, EWL);
    loc -= iacshift;

    /* now find the first pitch pulse for sure */
    if (loc < dpm) {
	offset = loc + iacshift + (short) (0.75 * delay + 0.5);
	length = (short) (0.5 * delay);
	g = guard - offset - EWL;	/* Maximum allowed search size */
	if (length > g)
	    length = g;
	maxeloc (&loc, &maxener, residual, offset, length, EWL);
	loc -= iacshift;
    }
    if (loc > dpm + delay) {
	offset = loc + iacshift - (short) (1.25 * delay + 0.5);
	length = (short) (0.5 * delay);
	g = guard - offset - EWL;	/* Maximum allowed search size */
	if (length > g)
	    length = g;
	maxeloc (&loc2, &maxener, residual, offset, length, EWL);
	loc2 -= iacshift;
	if (loc2 >= dpm)
	    loc = loc2;
    }

    *sfstart = dpm;
    *sfend = loc + extra;
    *maxshift2 = maxshift;
    if (loc < subframel - extra / 2 && *sfend > subframel)
	*sfend = subframel;
    if (loc < subframel + extra / 2 && *sfend > subframel + extra)
	*sfend = subframel + extra;
    if (loc >= subframel + extra / 2)
	*sfend = subframel;
    if (loc >= *sfend || loc < *sfstart)
	*maxshift2 = 0;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     pktoav                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

void
PeakToAverage (float *res, float *signal, short length)
{
#define EWL 2
    float ener, maxener, energy, tmp;
    register int i;
    int tail, front;

    ener = energy = 0.0;
    front = 2 * EWL;
    tail = 0;
    for (i = tail; i <= front; i++)
	ener += signal[i] * signal[i];
    maxener = energy = ener;
    for (i = 0; i < length - 2 * EWL - 1; i++) {
	front++;
	tmp = signal[front] * signal[front];
	ener += tmp - signal[tail] * signal[tail];
	if (tmp < 4.0 * energy)
	    energy = energy * 0.875 + tmp * 0.125;
	tail++;
	if (maxener < ener)
	    maxener = ener;
    }

    if (energy == 0)
	*res = 0;
    else
	*res = maxener / energy;
    *res *= SubFrameSize / (float) length;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     mod.c                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/
void
FGV_MEM::mod (float *residualm, float *accshift, float beta, short shiftr,
	      short resolution, float *exctation, float *Dresidual,
	      float *residual, short guard, short *dpm, float delay,
	      short subframel, short extra)
{
    float shiftr2;
    short sfstart, sfend;
    float Shift;
    float peaktoav;


    rcelp_fail = 0;



    while (*dpm < subframel) {
	cshiftframe (&sfstart, &sfend, &shiftr2, *dpm, residual, guard,
		     *accshift, (float) shiftr, delay, subframel, extra);

	PeakToAverage (&peaktoav, residual + sfstart - (short) *accshift,
		       sfend - sfstart);
	if (peaktoav < 16.0)
	    shiftr2 = 0.0;

	Shift = *accshift;
	modifyorig (residualm, accshift, beta, dpm, (short) shiftr2,
		    resolution, Dresidual, residual, sfstart, sfend);

    }
    *dpm -= subframel;




}
