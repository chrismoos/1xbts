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
//  File: hnwfilters.cc
//  Author: Edgardo M. Cruz-Zeno
//  Copyright (C) 1997-2001 Motorola, Inc. All rights reserved.
//
//###########################################################################
// CVS $Revision: 1.3.22.5 $
// Revision $Date: 2007/06/24 06:43:09 $
//###########################################################################

/*###########################################################################
 # Includes
 ###########################################################################*/
#include <stdlib.h>

#include "defines.h"
#include "hnw.h"

#include "macro.h"


//###########################################################################
// Prototypes
//###########################################################################


void
hnwFilter (float input[],
	   float state[],
	   float delay3[], short length, float &ghnw, float output[]
    )
{
    int i, j;
    float locdelay;
    float cor;
    float pwr;

    float temp[SubFrameSize];

    /* Read the input data to the state buffer */
    for (i = 0; i < length; i++) {
	state[i] = input[i];
    }

    /* Use average subframe delay as constant */
    locdelay = (delay3[1] + delay3[0]) / 2.0;

    HNW_interp (state, temp, locdelay, length);

  /*-----------------------------------------------------*/
    /* Compute the value of the HNW coefficient to be used */
  /*-----------------------------------------------------*/

    cor = 0.0;
    pwr = 0.0;
    for (i = 0; i < length; i++) {
	cor += input[i] * temp[i];
	pwr += temp[i] * temp[i];
    }

    if (pwr > 0. && cor > 0.) {
	ghnw = cor / pwr;
	if (ghnw > 1.) {
	    ghnw = 1.;
	}
    }
    else {
	ghnw = 0.;
    }


    {
	const float hnw_cutoff = HNW_CUTOFF, hnw_slope = HNW_SLOPE, hnw_min =
	    HNW_MIN, hnw_max = HNW_MAX;
	float scale;


	if (locdelay > hnw_cutoff) {
	    scale = hnw_min;
	}
	else {
	    scale =
		(hnw_min +
		 hnw_slope * ((hnw_cutoff - locdelay) / (hnw_cutoff)));
	    if (scale > hnw_max) {
		scale = hnw_max;
	    }
	}
	ghnw *= scale;

	if (ghnw > hnw_max) {
	    fprintf (stderr, "warning: ghnw = %f\n", ghnw);
	}

    }
    if (ghnw != 0.) {
	for (i = 0; i < length; i++) {
	    output[i] = input[i] - ghnw * temp[i];
	}
    }
    else {
	/* Transfer input to output (not necessary if done in-place) */
	for (i = 0; i < length; i++) {
	    output[i] = state[i];
	}
    }

}


//###########################################################################
// Zero-state Harmonic Noise Weighting Filter
//###########################################################################
void
hnwZeroState (float input[],
	      float delay3[], short length, float ghnw, float output[])
{
    int i, j, k;
    float locdelay;
    float xt[SubFrameSize];
    float state[SubFrameSize + 2 * HNW_BLPREC];

    /* Perform filtering operation only for non-zero pitch gain */
    if (ghnw != 0.0) {
	/* Read the input data to the state buffer */
	for (i = 0; i < 2 * HNW_BLPREC; i++) {
	    state[i] = 0.0;
	}
	for (i = 0; i < length; i++) {
	    state[i + 2 * HNW_BLPREC] = input[i];
	}

	/* Use average subframe delay as constant */
	locdelay = (delay3[1] + delay3[0]) / 2.0;
	k = Min (length, (int) rint (locdelay) - HNW_BLPREC);
	j = 2 * HNW_BLPREC;
	for (i = 0; i < k; i++) {
	    output[i] = state[j];
	    j++;		//ptr, no OP_COUNT
	}
	if (i < length) {
	    HNW_interp (&state[j], xt, locdelay, length - i);
	    for (k = 0; i < length; i++, j++, k++) {
		output[i] = state[j] - ghnw * xt[k];
	    }
	}
    }
    else {
	/* Transfer input to output (not necessary if done in-place) */
	for (i = 0; i < length; i++) {
	    output[i] = input[i];
	}
    }

}


//###########################################################################
// Computes Zero Input Response (ZIR) of HNW part of H
//###########################################################################

void
hnwZeroInput (float input[],
	      float state[],
	      float delay3[], short length, float ghnw, float output[]
    )
{
    register short i;
    int j;
    float locdelay;

    /* ZIR of HNW filter C(z) = (1 - HNW_FACTOR*ghnw*z^(-T) */

    /* Perform filtering operation only for non-zero pitch gain */
    if (ghnw != 0.0) {
	/* Read the ringing sythesis filter output to the state buffer */
	for (i = 0; i < length; i++) {
	    state[ACBMemSize + i] = input[i];
	}

	/* Use average subframe delay as constant */
	locdelay = (delay3[1] + delay3[0]) / 2.0;
	HNW_interp (&state[ACBMemSize], output, locdelay, length);
	for (i = 0; i < length; i++) {
	    output[i] = state[ACBMemSize + i] - ghnw * output[i];
	}
    }
    else {
	/* Transfer input to output (not necessary if done in-place) */
	for (i = 0; i < length; i++) {
	    output[i] = input[i];
	}
    }

}
