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
    "$Id: acbevrc.cc,v 1.12.6.4 2007/06/24 06:43:03 apadmana Exp $";

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
/*  Module:     computeacb                                              */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: ComputeACB                                              *
* Function: Compute ACB excitation.                                     *
************************************************************************/
#include "macro.h"
#include "proto.h"
#include "struct.h"


void
FGV_MEM::ComputeACB (float *residualm, float *excitation, float *delay3,
		     float *residual, short guard, short *dpm,
		     float *accshift, float beta, short length, short rshift)
{
    //    static float buffer[ACBMemSize + SubFrameSize + EXTRA];
    short i;

    /* initialization of bufferm. Should be in init routine for implementation. */
    if (acbevrcFirstTime) {
	acbevrcFirstTime = 0;
	for (i = 0; i < ACBMemSize + SubFrameSize + EXTRA; i++)
	    bufferm[i] = 0.0;
    }

    if (data_packet.WB_MODE_BIT == 1)
	/* Calculate next adaptive codebook excitation with low cut off frequency */
	putacbc (excitation, excitation, 0, length, EXTRA, delay3, BLFREQ,
		 BLPRECISION);
    else
	/* Calculate next adaptive codebook excitation with low cut off frequency */
	putacbc (excitation, excitation, 0, length, EXTRA, delay3, BLFREQ_NB,
		 BLPRECISION);


    /* Create a target modifed residual buffer from the past modified residual */
    putacbc (bufferm + ACBMemSize, bufferm + ACBMemSize, 0, length, EXTRA,
	     delay3, 0.5, 3);

    /* Modify original to match the excitation */
    mod (residualm, accshift, beta, rshift, RRESOLUTION, excitation,
	 bufferm + ACBMemSize, residual, guard, dpm, delay3[1], length,
	 EXTRA);
    if (write_accshift) {
	fprintf (stdout, "%f\n", *accshift);

    }

    for (i = 0; i < ACBMemSize - length; i++)
	bufferm[i] = bufferm[i + length];
    for (i = 0; i < length; i++)
	bufferm[ACBMemSize - length + i] = residualm[i];


}
