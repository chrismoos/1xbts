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
//  File: pitchprefilter.cc
//  Author: Edgardo M. Cruz-Zeno
//  Copyright (C) 1997-2001 Motorola, Inc. All rights reserved.
//
//###########################################################################
// CVS $Revision: 1.2.22.5 $
// Revision $Date: 2007/06/24 06:43:16 $
//###########################################################################

/*###########################################################################
 # Includes
 ###########################################################################*/
#include <stdlib.h>
#include <math.h>

#include "macro.h"

//###########################################################################
// Prototypes
//###########################################################################
void PPF_interp (float *signal, float delay[], float gppf, int length);


//###########################################################################
// Pitch Pre-Filter
//###########################################################################

void
pitchPreFilter (float *input, float *state, float *delay3, short length, float gppf, float &gppf_sm,
		float *output)
{
    int i, j;
    float locdelay;
    float acc1, acc2;
    float ge;

    /* Vary ppf strength as a function of delay */
    {
	static float ppf_cutoff = 90, ppf_slope = 1.0, ppf_min =
	    0.0, ppf_max = 0.4;
	float scale;
	float locdelay = (delay3[0] + delay3[1]) / 2.0;


	if (locdelay > ppf_cutoff) {
	    scale = ppf_min;
	}
	else {
	    scale =
		(ppf_min +
		 ppf_slope * ((ppf_cutoff - locdelay) / (ppf_cutoff)));
	    if (scale > ppf_max) {
		scale = ppf_max;
	    }
	}

	gppf *= scale;

    }

    /* copy input to state buffer */
    j = ACBMemSize;
    for (i = 0; i < length; i++) {
	state[j] = input[i];
	j++;			//ptr, no OP_COUNT
    }

    /* Do the delay contour pitch filter */
    PPF_interp (&state[ACBMemSize], delay3, gppf, length);

    /* Determine output energy normalization factor, ge */
    acc1 = 1e-40;
    acc2 = 1e-40;
    j = ACBMemSize;
    for (i = 0; i < length; i++) {
      acc1 += input[i] * input[i];
      acc2 += state[j] * state[j];
      j++;		//ptr, no OP_COUNT
    }
    ge = sqrt (acc1 / acc2);

    /* Normalize energy & write to output buffer */
    j = ACBMemSize;
    for (i = 0; i < length; i++) {
      output[i] = ge * state[j];
      j++;		//ptr, no OP_COUNT
    }

    /* Update filter state */
    j = length;
    for (i = 0; i < ACBMemSize; i++) {
	state[i] = state[j];
	j++;			//ptr, no OP_COUNT
    }

}
