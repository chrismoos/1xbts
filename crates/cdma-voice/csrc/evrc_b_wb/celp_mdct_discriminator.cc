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
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include "struct.h"

#include "mdct.h"

#define PER_ERR    0.75		//search till 75 percent of energy has been included
#define MAXCOEFF   50		//stop the search if the energy goal has not met been met even with these many coeffs
#define DIFFTH     1

#define MDCTCNTMAX 5		//this means a steady state MDCT has 5 consecutive mdct frames
#define CELPCNTMAX 5		//this means a steady state CELP has 5 consecutive celp frames

#define BOOST_CELP_COMP

#define COMP_BOOST_FAC 1.12




int
compare (const void *a, const void *b)
{
    int temp;

    if ((*(float *) a - *(float *) b) > 0)
	temp = -1;
    else {
	if ((*(float *) a - *(float *) b) < 0)
	    temp = 1;
	else
	    temp = 0;

    }
    return (temp);

}

float
mx (float a, float b)
{
    if (a > b)
	return (a);
    else {
	if (a < b)
	    return (b);
	else
	    return (a);
    }

}



void
FGV_MEM::celp_mdct_discriminator (short *decision)
{
    int i, j, k;    float Ftot = 0.0, Ttot = 0.0;
    float dsp, dmu;

    short TsparseCnt = 0, FsparseCnt = 0, ccnt = 0, decmade = 0;
    float tempsum = 0, ftempsum = 0;



    float Fresidual[320], Tener[320], Fener[320], tm[160], fm[160];	// Frequency domain residual
    float diffth;


    if (lastrateE != 4 || prev_celp_mdct_dec != 1)
	mdct (residual, Fresidual, 1);
    else
	mdct (residual, Fresidual, 0);

    for (i = 0; i < 160; i++) {
	Tener[i] =
	    (residual[2 * i] + residual[2 * i + 1]) * (residual[2 * i] +
						       residual[2 * i +
								1]) / 4;
	Ttot += Tener[i];
    }


    for (i = 0; i < 160; i++) {
	Fener[i] = Fresidual[i] * Fresidual[i];
	Ftot += Fener[i];

    }


    qsort (Tener, 160, sizeof (float), compare);
    tm[0] = Tener[0] / Ttot;


    qsort (Fener, 160, sizeof (float), compare);
    fm[0] = Fener[0] / Ftot;

    for (i = 1; i < 160; i++) {

	tempsum = Tener[i] / Ttot;
	tm[i] = tm[i - 1] + tempsum;

	tempsum = (Fener[i]) / Ftot;
	fm[i] = fm[i - 1] + tempsum;
    }

    for (i = 0; i < 160; i++) {
	tm[i] = tm[i] * ((COMP_BOOST_FAC * (159 - i) + i) / 159);
    }


    dsp = 0;
    dmu = 0;

    while (mx (tm[ccnt], fm[ccnt]) <= PER_ERR && ccnt <= MAXCOEFF) {
	if (tm[ccnt] > fm[ccnt]) {
	    TsparseCnt = TsparseCnt + 1;
	    dsp = dsp + (tm[ccnt] - fm[ccnt]);
	}
	else {
	    FsparseCnt = FsparseCnt + 1;
	    dmu = dmu + (fm[ccnt] - tm[ccnt]);
	}
	ccnt = ccnt + 1;
    }


    if (ccnt > 3)
	diffth = DIFFTH;
    else
	diffth = 0;

    if (((TsparseCnt > FsparseCnt + diffth) || beta > 0.85)
	&& mdct_hist_cnt < 4) {

	*decision = 0;
	celp_hist_cnt = celp_hist_cnt + 2;
	mdct_hist_cnt = mdct_hist_cnt - 1;
	if (celp_hist_cnt > CELPCNTMAX)
	    celp_hist_cnt = CELPCNTMAX;
	if (mdct_hist_cnt < 0)
	    mdct_hist_cnt = 0;

    }
    else if (FsparseCnt > TsparseCnt + diffth && mdct_hist_cnt == 5) {

	*decision = 1;
	celp_hist_cnt = celp_hist_cnt - 1;
	mdct_hist_cnt = mdct_hist_cnt + 2;
	if (mdct_hist_cnt > MDCTCNTMAX)
	    mdct_hist_cnt = MDCTCNTMAX;
	if (celp_hist_cnt < 0)
	    celp_hist_cnt = 0;
    }


    else if (ccnt != MAXCOEFF) {
	decmade = 0;
	if (dmu > dsp) {
	    if (mdct_hist_cnt > 4) {	//   && celp_hist_cnt <= 0.5 
		*decision = 1;
		decmade = 1;
		if (fabs (dmu - dsp) > 1) {
		    celp_hist_cnt = celp_hist_cnt - 1;
		    mdct_hist_cnt = mdct_hist_cnt + 1;
		    if (mdct_hist_cnt > MDCTCNTMAX)
			mdct_hist_cnt = MDCTCNTMAX;
		    if (celp_hist_cnt < 0)
			celp_hist_cnt = 0;
		}
	    }
	    else {

		*decision = 0;
		decmade = 1;
		if (fabs (dmu - dsp) > 1) {
		    celp_hist_cnt = celp_hist_cnt - 0.75;
		    mdct_hist_cnt = mdct_hist_cnt + 0.75;
		    if (mdct_hist_cnt > MDCTCNTMAX)
			mdct_hist_cnt = MDCTCNTMAX;
		    if (celp_hist_cnt < 0)
			celp_hist_cnt = 0;
		}
	    }

	}

	else if (dmu < dsp) {

	    if (celp_hist_cnt >= 1) {	//   && mdct_hist_cnt <= 4
		*decision = 0;
		decmade = 1;
		celp_hist_cnt = celp_hist_cnt + 1;
		mdct_hist_cnt = mdct_hist_cnt - 1;
		if (celp_hist_cnt > CELPCNTMAX)
		    celp_hist_cnt = CELPCNTMAX;
		if (mdct_hist_cnt < 0)
		    mdct_hist_cnt = 0;
	    }
	    else {
		if (mdct_hist_cnt > 3) {
		    *decision = 1;
		    decmade = 0;
		    celp_hist_cnt = celp_hist_cnt + 0.5;
		    mdct_hist_cnt = mdct_hist_cnt - 0.5;
		    if (celp_hist_cnt > CELPCNTMAX)
			celp_hist_cnt = CELPCNTMAX;
		    if (mdct_hist_cnt < 0)
			mdct_hist_cnt = 0;
		}
		else {
		    *decision = 0;
		    decmade = 0;
		    celp_hist_cnt = celp_hist_cnt + 0.75;
		    mdct_hist_cnt = mdct_hist_cnt - 0.75;
		    if (celp_hist_cnt > CELPCNTMAX)
			celp_hist_cnt = CELPCNTMAX;
		    if (mdct_hist_cnt < 0)
			mdct_hist_cnt = 0;
		}

	    }
	}
	else
	    *decision = 0;
    }


    else
	*decision = 13;

}
