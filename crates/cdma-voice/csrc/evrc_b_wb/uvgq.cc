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
    "$Id: uvgq.cc,v 1.7.22.3 2007/06/24 06:43:19 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <math.h>
#include "uvgq.h"

#define SQR(a) ((a)*(a))

void
dequantize_uvg (int iG1, int *iG2, float *G)
{
    double g1_0 = pow (10.0, UVG1CB[iG1][0]);
    double g1_1 = pow (10.0, UVG1CB[iG1][1]);

    G[0] = g1_0 * UVG2CB[0][iG2[0]][0];
    G[1] = g1_0 * UVG2CB[0][iG2[0]][1];
    G[2] = g1_0 * UVG2CB[0][iG2[0]][2];
    G[3] = g1_0 * UVG2CB[0][iG2[0]][3];
    G[4] = g1_0 * UVG2CB[0][iG2[0]][4];
    G[5] = g1_1 * UVG2CB[1][iG2[1]][0];
    G[6] = g1_1 * UVG2CB[1][iG2[1]][1];
    G[7] = g1_1 * UVG2CB[1][iG2[1]][2];
    G[8] = g1_1 * UVG2CB[1][iG2[1]][3];
    G[9] = g1_1 * UVG2CB[1][iG2[1]][4];
}

float
quantize_uvg (float *G, int &iG1, int *iG2, float *quantG)
{

    float G1[2], G2[10];
    int i, j, k;

    float mse, mmse, snr;

    // The G's are all ensured to be non-zero since I changed nelp.cc
    // So, I don't need to keep a minimum value for G1 or G2
    // Without this, with NS (even on clean speech) gives a seg. fault due to
    // infinities and iG1 is also some unknown value. Also initialized iG1=0
    // to be safe, iG2[] was already initialized - Sharath

    for (i = 0; i < 2; i++) {
	G1[i] = 0;
	for (j = 0; j < 5; j++)
	    G1[i] += SQR (G[i * 5 + j]);
	G1[i] = log10 (sqrt (G1[i] / 5));
    }

    mmse = 1e30;
    iG1 = 0;
    for (i = 0; i < UVG1_CBSIZE; i++) {
	mse = SQR (G1[0] - UVG1CB[i][0]) + SQR (G1[1] - UVG1CB[i][1]);
	if (mse < mmse) {
	    iG1 = i;
	    mmse = mse;
	}

    }

    G1[0] = pow (10.0, UVG1CB[iG1][0]);
    G1[1] = pow (10.0, UVG1CB[iG1][1]);

    for (i = 0; i < 2; i++) {
	for (j = 0; j < 5; j++)
	    G2[i * 5 + j] = G[i * 5 + j] / G1[i];
    }

    for (i = 0; i < 2; i++) {
	mmse = 1e30;
	iG2[i] = 0;
	for (j = 0; j < UVG2_CBSIZE; j++) {
	    mse = 0;
	    for (k = 0; k < 5; k++)
		mse += SQR (G2[i * 5 + k] - UVG2CB[i][j][k]);
	    if (mse < mmse) {
		mmse = mse;
		iG2[i] = j;
	    }
	}
    }

    for (i = 0; i < 10; i++)
	G2[i] = G[i];
    dequantize_uvg (iG1, iG2, quantG);

    mmse = mse = 0;
    for (i = 0; i < 10; i++) {
	mmse += SQR (quantG[i] - G2[i]);
	mse += SQR (G2[i]);
    }
    snr = 10 * log10 (mse / mmse);

    return snr;
}
