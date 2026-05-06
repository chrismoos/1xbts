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
    "$Id: olpitch.cc,v 1.8.22.4 2007/06/24 06:43:15 apadmana Exp $";

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
/*  Module:     fndppf.c                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/07/93  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: fndppf                                                  *
* Function: forward pitch predictor.		                        *
* Inputs:   buf    - data buffer.                                       *
*           dmin   - minimum delay value.                               *
*           dmax   - maximum delay value.                               *
*           prevdelay - previous frame delay value.                     *
*           length - size of pitch window.                              *
* Outputs:  delay  - predicted delay.                                   *
*           beta   - gain value.                                        *
*                                                                       *
************************************************************************/
#include "globs.h"
#include "macro.h"
#include "struct.h"


int
get_delay (int center, int range, float buf[], int length, float *corrmax,
	   int dmin, int dmax)
{
    int i, m, m0, M1, M2;
    float cmax = -1.0e30, sum;

    M1 = center - range;
    M2 = center + range;
    if (M1 < dmin) {
	M1 = dmin;
	M2 = dmin + 2 * range;
    }
    if (M2 > dmax) {
	M2 = dmax;
	M1 = dmax - 2 * range;
    }

    for (m = M1; m <= M2; m++) {
	for (i = 0, sum = 0.0; i < length - m; i++)
	    sum += buf[i] * buf[i + m];
	if (sum > cmax) {
	    cmax = sum;
	    m0 = m;
	}
    }

    *corrmax = cmax;
    return m0;
}

float
get_beta (int delay, int length, float cmax, float buf[])
{
    int i;
    float sum1, sum2;

    for (i = delay, sum1 = 0.0; i < length; i++)
	sum1 += buf[i] * buf[i];
    for (i = 0, sum2 = 0.0; i < length - delay; i++)
	sum2 += buf[i] * buf[i];

    sum1 = sqrt (sum1 * sum2);
    if (sum1 == 0 || cmax <= 0)
	sum2 = 0.0;
    else if ((sum2 = cmax / sum1) > 1.0)
	sum2 = 1.0;

    return (sum2);
}

void
FGV_MEM::fndppf (float *delay, float *beta, float *buf, short dmin,
		 short dmax, short length)
{
    static float b = -0.312;	/* rom storage */
    static float a[3] = { -2.2875, 1.956, -0.5959 };	/* rom storage */

    short dnew = dmin / 4;
    float sum;
    register int m, i, n;
    float corrmax, cmax, tap1, tmp;
    short M1, M2, dnewtmp = dmin / 4;

    /* init static variables (should be in init routine for implementation) */
    if (fndppf_FirstTime) {
	fndppf_FirstTime = 0;
	for (i = 0; i < FrameSize / 4; i++)
	    DECbuf[i] = 0.0;
	olp_memory[0] = olp_memory[1] = olp_memory[2] = 0.0;
    }

    /* Shift memory of DECbuf */
    for (i = 0; i < length / 8; i++)
	DECbuf[i] = DECbuf[i + length / 8];

    /* filter signal and decimate */
    for (i = 0, n = length / 8; i < length / 2; i++) {
	sum =
	    buf[i + length / 2] - a[0] * olp_memory[0] -
	    a[1] * olp_memory[1] - a[2] * olp_memory[2];
	if ((i + 1) % 4 == 0)
	    DECbuf[n++] =
		sum + olp_memory[0] * b + olp_memory[1] * b + olp_memory[2];
	olp_memory[2] = olp_memory[1];
	olp_memory[1] = olp_memory[0];
	olp_memory[0] = sum;
    }

    /* perform first search for best delay value in decimated domain */
    corrmax = -1.0e30;

    for (m = dmin / 4; m <= dmax / 4; m++) {
	for (i = 0, sum = 0.0; i < length / 4 - m; i++)
	    sum += DECbuf[i] * DECbuf[i + m];
	if (sum > corrmax) {
	    corrmax = sum;
	    dnew = m;
	}
    }

    /* perform first search for best delay value in non-decimated buffer */
    dnew = get_delay (4 * dnew, 3, buf, length, &corrmax, dmin, dmax);
    *beta = get_beta (dnew, length, corrmax, buf);

    /* perform search for best delay value in around old pitch delay */
    if (lastgoodpitch != 0
	&& (dnew > (lastgoodpitch + 6) || dnew < (lastgoodpitch - 6))) {
	dnewtmp =
	    get_delay (lastgoodpitch, 6, buf, length, &cmax, dmin, dmax);
	tap1 = get_beta (dnewtmp, length, cmax, buf);

	/* Replace dnew with dnewtmp if tap1 is large enough */
	if (dnew > lastgoodpitch + 6)
	    tmp = 0.6;
	else if (dnew < lastgoodpitch - 6)
	    tmp = 1.2;
	else
	    tmp = 10.0;

	if (tap1 > tmp * *beta) {
	    dnew = dnewtmp;
	    *beta = tap1;
	}
    }

    /* Check if half of the selected pitch a better candidate */
    if (dnew >= 40) {
	dnewtmp = get_delay (dnew / 2, 2, buf, length, &cmax, dmin, dmax);
	tap1 = get_beta (dnewtmp, length, cmax, buf);

	if (tap1 > 0.8 * *beta) {
	    dnew = dnewtmp;
	    *beta = tap1;
	}
    }

    *delay = (float) dnew;
    if (*beta > 0.4) {
	lastgoodpitch = dnew;
	lastbeta = *beta;
    }
    else {
	lastbeta *= 0.75;
	if (lastbeta < 0.3)
	    lastgoodpitch = 0;
    }

}
