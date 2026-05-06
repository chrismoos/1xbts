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
    "$Id: rom.cc,v 1.6.22.3 2007/06/24 06:43:18 apadmana Exp $";

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
/*  Module:     rom.c                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/
#include  "macro.h"

/*======================================================================*/
/*         ..Globals.                                                   */
/*----------------------------------------------------------------------*/

/* ROM tables */


long Base18[SubFrameSize / SPACING] = { 1, 18, 324, 5832, 104976, 1889568 };

/* Quantization table for pitch gain */
float ppvq[ACBGainSize] = { 0.0, 0.3, 0.55, 0.7, 0.8, 0.9, 1.0, 1.2 };
float gnvq_8[maxFCBGainSize] = {
    1.284025417f, 1.648721271f, 2.117000017f, 2.718281828f,
    3.490342957f, 4.481689070f, 5.754602676f, 7.389056099f,
    9.487735836f, 12.182493961f, 15.642631884f, 20.085536923f,
    25.790339917f, 33.115451959f, 42.521082000f, 54.598150033f,
    70.105412347f, 90.017131301f, 115.584284527f, 148.413159103f,
    190.566268459f, 244.691932264f, 314.190660286f, 403.428793493f,
    518.012824668f, 665.141633044f, 854.058762526f, 1096.633158428f,
    1408.104848205f, 1808.042414456f, 2321.572414611f, 2980.957987042f
};				/* Quantization table for fcb gain */
float gnvq_4[maxFCBGainSize] = {
    1.648721271f, 2.718281828f, 4.481689070f, 7.389056099f,
    12.182493961f, 20.085536923f, 33.115451959f, 54.598150033f,
    90.017131301f, 148.413159103f, 244.691932264f, 403.428793493f,
    665.141633044f, 1096.633158428f, 1808.042414456f, 2980.957987042f
};				/* Quantization table for fcb gain */

short nsize8[2] = { 16, 16 };
short lognsize8[2] = { 4, 4 };	/* c.b. size of each sub-matrix   */
short nsub8[2] = { 5, 5 };	/* Vector size of each sub-matrix */


float rnd_delay[NoOfSubFrames + 2] = { 55.0, 80.0, 39.0, 71.0, 33.0 };

/* QC quarter rate quantizer */
short nsize16[2] = { 256, 256 };
short nsub16[2] = { 5, 5 };
short lognsize16[2] = { 8, 8 };

/* New AT&T half-rate quantizer */
short nsize22[3] = { 128, 128, 256 };
short nsub22[3] = { 3, 3, 4 };
short lognsize22[3] = { 7, 7, 8 };

/* New AT&T full-rate quantizer */
short nsize28[4] = { 64, 64, 512, 128 };
short lognsize28[4] = { 6, 6, 9, 7 };
short nsub28[4] = { 2, 2, 3, 3 };

#include "lspq.dat"
