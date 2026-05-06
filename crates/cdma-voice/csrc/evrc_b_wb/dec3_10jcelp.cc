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

#include <float.h>
#include <math.h>
#include "macro.h"
#include "struct.h"
#include "globs.h"
#define L_CODE    55
#define NB_PULSE  3
#define STEP      7
#define NB_TRACK  3


/*-------------------------------------------------------------------*
 * routine   dec3_10.c                                               *
 *           ~~~~~~~~~~                                              *
 * Algebraic codebook decoder; 10 bits:                              *
 * 3 pulses in a frame of 55 samples.  (see cod3_10.c)               *
 *-------------------------------------------------------------------*
 * Bruno Bessette,   august 1995                                     *
 * Claude Laflamme,  august 1995                                     *
 *-------------------------------------------------------------------*/

extern int index0[], index1[];

void
FGV_MEM::dec3_10jcelp (int pit_lag, float pit_gain, int l_subfr, int *indices,	/* input: indices of 3 pulses (10 bits, 1 word)        */
		       float cod[]	/* output: algebraic (fixed) codebook excitation       */
    )
{
    int i, pos, step, flag, index = indices[0];
    float r, s, sign;

    //static int offset[3] = {0, 2, 4};

    int pos_table[512][3], ix;

	/*--------------------------------------*/
    /*      Reconstruct configuration       */
	/*--------------------------------------*/

    /* Construct JCELP configuration */
    flag =
	JCELP_Config (pit_lag, pit_gain, l_subfr, pos_table, cod3_10_offset,
		      &step, &pos, &sign, &r);

	/*--------------------------------------------------------------------*
  	 * BUILD THE CODEWORD, THE FILTERED CODEWORD AND INDEX OF CODEVECTOR. *
 	 *--------------------------------------------------------------------*/
    for (i = 0; i < L_CODE; i++)
	cod[i] = 0.0;
    s = ((index & 0x0200) == 0) ? 1.0 : -1.0;

    if (flag > 0) {
	ix = index & 0x1ff;
	for (i = 2; i >= 0; i--) {
	    pos = pos_table[ix][i];
	    cod[pos] += s;
	    s = -s;
	}
    }
    else {
	index &= 0x01ff;	/* strip off sign bit */

	i = index0[index / 23];
	cod[i] += s;
	cod[i + cod3_10_offset[1]] += r * s;

	i = index1[index % 23];
	cod[i] += s * sign;
    }


    if (flag == 2)
	for (i = pit_lag; i < l_subfr; i++)
	    cod[i] += cod[i - pit_lag];

    return;
}
