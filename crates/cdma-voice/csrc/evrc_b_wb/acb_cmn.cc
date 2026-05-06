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
    "$Id: acb_cmn.cc,v 1.16.6.5 2007/06/24 06:43:03 apadmana Exp $";

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

#include <math.h>
#include "macro.h"
#include "proto.h"


void
FGV_MEM::getgain (float *exctation, float *lambda, float *H, short *idxcb,
		  float *gcb, float *gcb_mid, short gcb_size, short Quantize,
		  float *mresidual, short subframel, short hlength)
{
    register int i;
    float cross, ener;
    float temp[54];

    cross = ener = 0.0;

    ConvolveImpulseR (temp, exctation, H, hlength, subframel);

    for (i = 0; i < subframel; i++) {
	cross += mresidual[i] * temp[i];
	ener += temp[i] * temp[i];
    }



    if (Quantize == 1) {	/* If lambda should be quantized */
	if (cross > 0) {
	    *idxcb = 0;
	    for (i = 1; i < gcb_size; i++) {
		if (cross > gcb_mid[i - 1] * ener)
		    *idxcb = i;
	    }
	    *lambda = gcb[*idxcb];
	}
	else {			/* No possitive cross correlation */
	    *lambda = gcb[0];	/* Ex1 = 0.0 */
	    *idxcb = 0;
	}
    }
    else {
	if (cross < 0)
	    *lambda = 0.0;
	else
	    *lambda = cross / (ener + 1.0);
	if (*lambda > 1.2)
	    *lambda = 1.2;
    }
    if (data_packet.WB_MODE_BIT == 1) {



    }
    else {
	for (i = 0; i < subframel; i++)
	    exctation[i] *= *lambda;
    }


}

void
FGV_MEM::gainvq (float *target, float *ACBExH, float *FCBExH, float *ACBEx,
	float *fcbGain, float *acbGain, short *idxppg, short *idxcbg,
	float *ppvq, float *gnvq, short ACBSize, short FCBGainSize, int delay,
	short subframel)
{
    float old_acb, sum, sum0, sum1;
    float mvalue = 1e99;
    float factor = 1;
    short i, j, k;
    float cross[2];
    float corr[2][2];
    float *p[2];

    p[0] = ACBExH;
    p[1] = FCBExH;

    for (i = 0; i < 2; i++) {
	sum = 0;
	for (j = 0; j < subframel; j++) {
	    sum += target[j] * p[i][j];
	}
	cross[i] = sum;
    }
    for (i = 0; i < 2; i++) {
	for (k = 0; k <= i; k++) {
	    sum = 0;
	    for (j = 0; j < subframel; j++) {
		sum += p[k][j] * p[i][j];
	    }
	    corr[i][k] = corr[k][i] = sum;
	}
    }
    for (i = 0; i < ACBSize; i++) {
	sum0 = ppvq[i] * ppvq[i] * corr[0][0] - 2 * ppvq[i] * cross[0];

	factor = 1 / (1 - 0.12 * ppvq[i]);

	for (j = 0; j < FCBGainSize; j++) {

	    {
		float gx;

		if (j < NOOFFCBLEVELFOR_REGQ) {
		    gx = GAIN_RE_F (ppvq[i], j);
		    sum1 =
			sum0 + factor * factor * gx * gx * corr[1][1] -
			2 * gx * cross[1] * factor;
		    sum1 += 2 * gx * ppvq[i] * corr[0][1] * factor;
		}
		else
		    break;
	    }

	    if (sum1 < mvalue) {
		mvalue = sum1;
		*idxppg = i;
		*idxcbg = j;
	    }
	}

    }
    *fcbGain = GAIN_RE_F (ppvq[*idxppg], *idxcbg);
    *acbGain = ppvq[*idxppg];
}



void
FGV_MEM::gainfcbsearch (float *Targetw, float *y2, float *fcbGain, float acbgain,
	       short *idxcbg, int subframesize)
{
    int i, j, minindex;
    float dist, mindist = 1e99, g, factor, c1 = 0, c2 = 0;

    for (i = 0; i < subframesize; i++) {
	c1 += Targetw[i] * y2[i];
	c2 += y2[i] * y2[i];
    }
    factor = 1 / (1 - 0.12 * acbgain);
    *idxcbg = 0;
    for (j = 0; j < NOOFFCBLEVELFOR_REGQ; j++) {
	g = GAIN_RE_F (acbgain, j) * factor;
	dist = g * g * c2 - 2 * c1 * g;
	if (dist < mindist) {
	    mindist = dist;
	    *idxcbg = j;
	}
    }
    *fcbGain = GAIN_RE_F (acbgain, *idxcbg);

}




#include <assert.h>


float
FGV_MEM::GAIN_RE_F (float acbGain, int j)
{

    const float k_table[16] = {
	//s=log10(0.0251);e=log10(1.00);n=16;i=(e-s)/(n-1);10.^(s:i:e)'
	0.0251,
	0.0321,
	0.0410,
	0.0524,
	0.0671,
	0.0857,
	0.1096,
	0.1401,
	0.1791,
	0.2290,
	0.2928,
	0.3743,
	0.4786,
	0.6118,
	0.7822,
	1.0000
    };

    assert (j >= 0 && j < 16);

    float A = (1.0 - acbGain * acbGain / NoOfSubFrames);

    if (A > 0) {
	A = k_table[j] * Erq * sqrt (A / ckck);
    }
    else {
	A = 0.0;
	printf ("%d\t  %f\t  %f\t  %f\n", j, acbGain, Erq, k_table[j]);
    }
    return (A);

}



/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:    putacbc                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/* put acbc()
   puts unscaled adaptive codebook contribution in place
   assumes that pitch is known at dpl+extra, and dpl+extra+subframel
   */
void
FGV_MEM::putacbc (float *exctation, float *input, short dpl, short subframel,
		  short extra, float *delay3, float freq, short prec)
{
    float denom;
    float locdelay;
    register int i, j;
    short dpr;
    float invsubframel;

    invsubframel = 1.0 / ((float) subframel);
    dpr = dpl + subframel;

    /* first at-most extra samples */
    denom = (delay3[1] - delay3[0]) * invsubframel;
    for (i = dpl, j = 0; i < dpr; i++) {
	locdelay = delay3[0] + (i - dpl) * denom;
	bl_intrp (exctation + i, input + i, locdelay, freq, prec);
    }
    denom = (delay3[2] - delay3[1]) * invsubframel;
    /* interpolation */
    for (i = dpr; i < dpr + extra; i++) {
	locdelay = delay3[1] + (i - dpr) * denom;
	bl_intrp (exctation + i, input + i, locdelay, freq, prec);
    }

}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     acb_ex                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     09/12/92  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: acb_excitation                                          *
* Function: Adaptive codebook excitation.                               *
************************************************************************/

void
FGV_MEM::acb_excitation (float *Ex1, float gain, float *delay3,
			 float *PitchMemory, short length)
{
    register short i;

    if (data_packet.WB_MODE_BIT == 1) {

	putacbc (Ex1, PitchMemory + ACBMemSize, 0, length, 10, delay3, BLFREQ,
		 BLPRECISION);
    }
    else {

	putacbc (Ex1, PitchMemory + ACBMemSize, 0, length, 10, delay3,
		 BLFREQ_NB, BLPRECISION);
    }

    /* Scale excitation */
    for (i = 0; i < length; i++)
	Ex1[i] *= gain;
}
