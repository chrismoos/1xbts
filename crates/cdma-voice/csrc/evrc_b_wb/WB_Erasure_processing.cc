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


/* This performs erasure processing for the wideband parameters. If the mode indicates erasure (=14)
then the quantized upper nband parameters are modified otherise some local static parameters are updated
and the program quits*/

#include "struct.h"		//includes above
#include "defines.h"

#define M 5

void
FGV_MEM::WB_Erasure_processing (struct QUANTIZED_HB *p, unsigned short mode,
				short FirstTime)
{
    int i, j;

    static float ScaleFac[5] = { 0.9, 0.7, 0.5, 0.3, 0.1 }, LsfFac[5] =
    {
    0.8, 0.6, 0.4, 0.2, 0.1};

    if (FirstTime == 0) {

	float ftmp;

	ftmp = 0.48 / (float) LPC_ORD_WB;

	prev_LSFs[0] = ftmp;
	for (i = 1; i < LPC_ORD_WB; i++)
	    prev_LSFs[i] = prev_LSFs[i - 1] + ftmp;

	for (i = 0; i < 5; i++)
	    prev_Gain[i] = 0.0;

	prev_GainFrame = 0.0;

    }
    if (mode == ERASURE) {
	GoodCount = 0;
	ErasureCount =
	    (ErasureCount + 1) <
	    MaxErasureCount ? (ErasureCount + 1) : MaxErasureCount;
	if (ErasureCount >= 1) {

	    for (i = 0; i < M; i++)
		p->Gain[i] = (float) (1 / (float) M);


	    float GainSum = 0.0;

	    for (i = 0; i < M; i++)
		GainSum += prev_Gain[i];
	    p->GainFrame =
		prev_GainFrame * GainSum * ScaleFac[ErasureCount - 1];


	    for (i = 0; i < LPC_ORD_WB; i++) {
		p->LSFs[i] =
		    (prev_LSFs[i] * LsfFac[ErasureCount - 1]) +
		    ((i + 1) * 0.5 * (1 -
				      LsfFac[ErasureCount -
					     1]) / (LPC_ORD_WB + 1));
	    }


	}


    }
    else {

	GoodCount =
	    (GoodCount + 1 <
	     MaxGoodCountNeeded) ? (GoodCount + 1) : MaxGoodCountNeeded;

	if (GoodCount > MaxGoodCountNeeded)
	    ErasureCount = 0;
	else
	    ErasureCount = (ErasureCount - 1) > 0 ? (ErasureCount - 1) : 0;
    }

    /*Update previous frame parameters for next frame */

    for (i = 0; i < LPC_ORD_WB; i++)
	prev_LSFs[i] = p->LSFs[i];

    for (i = 0; i < 5; i++)
	prev_Gain[i] = p->Gain[i];

    prev_GainFrame = p->GainFrame;




}
