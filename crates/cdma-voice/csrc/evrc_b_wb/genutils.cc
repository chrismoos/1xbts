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

static char const rcsid[] =
    "$Id: genutils.cc,v 1.10.6.3 2007/06/24 06:43:09 apadmana Exp $";

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     weight                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/06/93  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

#include "macro.h"
#include "struct.h"

double
mod (double x)
{
    if (x > pi)
	x -= pi2;
    if (x <= -pi)
	x += pi2;
    return x;
}

/************************************************************************
* Routine name: weight                                                  *
* Function: exponential weighting of LPC coefficients.                  *
* Inputs: a - LPC coefficients.                                         *
*         gamma - weighting factor. The factor is used to make          *
*                 a multiplication table each time it is used. For real *
*                 time implementation, send a pointer to a ROM table.   *
*         order - filter order.                                         *
* Output: awght - weighted lpc coefficients.                            *
************************************************************************/
/*
 * weigth - exponential weighting of lpc coefficients
 */

void
FGV_MEM::weight (float *awght, float *a, float gamma, short order)
{
    register int i;
    double fac;

    /* For the simulation: prepare a multiplication table */
    fac = gamma;

    /* Now calculate the exponential weighting */
    for (i = 0; i < order; i++) {
	if (data_packet.WB_MODE_BIT == 1) {
	    *awght++ = *a++ * fac;
	}
	else {			//narrowband case

	    *awght++ = *a++ * fac;

	}
	fac *= gamma;
    }
    if (data_packet.WB_MODE_BIT == 1) {
    }

}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     getres                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: GetResidual                                             *
* Function: FIR filter.                                                 *
************************************************************************/

void
GetResidual (float *residual, float *input, float *coef, float *FIRmemory,
	     short order, short length)
{
    register short i, j;
    float SUM;

    for (i = 0; i < length; i++) {
	/* FIR section */
	SUM = input[i];
	for (j = order - 1; j > 0; j--) {
	    SUM = SUM + coef[j] * FIRmemory[j];
	    FIRmemory[j] = FIRmemory[j - 1];
	}
	SUM = SUM + coef[0] * FIRmemory[0];
	FIRmemory[0] = input[i];
	residual[i] = SUM;
    }
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     impulser                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     4/28/94  Written By Dror Nahumi, AT&T                            */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: ImpulseR                                                *
* Function: Calculates impulse response.                                *
************************************************************************/


void
ImpulseRzp1 (float *output, float *coef_uq, float *coef, float gamma1,
	     float gamma2, short order, short length)
{
    register short i, j;
    float SUM;
    float memory[ORDER];
    float wcoef[ORDER];

    /* ImpulseR of 1/A(Z) */
    output[0] = 1.0;
    memory[0] = 1.0;

    for (i = 1; i < order; i++)
	memory[i] = 0;
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = 0; j > 0; j--) {
	    SUM -= coef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM -= coef[0] * memory[0];
	*memory = SUM;
	output[i] = SUM;
    }

}



void
FGV_MEM::ImpulseRzp (float *output, float *coef_uq, float *coef, float gamma1,
		     float gamma2, short order, short length)
{
    register short i, j;
    float SUM;
    float memory[ORDER];
    float wcoef[ORDER];

    /* ImpulseR of 1/A(Z) */
    output[0] = 1.0;
    memory[0] = 1.0;

    for (i = 1; i < order; i++)
	memory[i] = 0;
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = 0; j > 0; j--) {
	    SUM -= coef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM -= coef[0] * memory[0];
	*memory = SUM;
	output[i] = SUM;
    }

    /* Cascade with ImpulseR of A(Z/gamma1) */
    weight (wcoef, coef_uq, gamma1, order);

    memory[0] = 1.0;
    for (i = 1; i < order; i++)
	memory[i] = 0;
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = output[i]; j > 0; j--) {
	    SUM += wcoef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM += wcoef[0] * memory[0];
	*memory = output[i];
	output[i] = SUM;
    }
    if (data_packet.WB_MODE_BIT == 1) {
	/* Cascade with ImpulseR of 1/A(Z/gamma2) */
	wcoef[0] = DEEMPH_FAC;
	memory[0] = 1.0;
	for (i = 1; i < 1; i++)
	    memory[i] = 0;
	for (i = 1; i < length; i++) {
	    for (j = 1 - 1, SUM = output[i]; j > 0; j--) {
		SUM -= wcoef[j] * memory[j];
		memory[j] = memory[j - 1];
	    }
	    SUM -= wcoef[0] * memory[0];
	    *memory = SUM;
	    output[i] = SUM;
	}
    }
    else {			//narrowband case
	/* Cascade with ImpulseR of 1/A(Z/gamma2) */
	weight (wcoef, coef_uq, gamma2, order);

	memory[0] = 1.0;
	for (i = 1; i < order; i++)
	    memory[i] = 0;
	for (i = 1; i < length; i++) {
	    for (j = order - 1, SUM = output[i]; j > 0; j--) {
		SUM -= wcoef[j] * memory[j];
		memory[j] = memory[j - 1];
	    }
	    SUM -= wcoef[0] * memory[0];
	    *memory = SUM;
	    output[i] = SUM;
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
/*  Module:     convh                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

void
ConvolveImpulseR (float *out, float *in, float *H, short hlength,
		  short length)
{
    register int i, j;

    for (i = 0; i < length; i++) {
	out[i] = 0.0;
	for (j = 0; j <= Min (i, hlength - 1); j++)
	    out[i] += H[j] * in[i - j];
    }
}
