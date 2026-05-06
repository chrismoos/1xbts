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

/**********************************************************************/
/* Enhanced Variable Rate Speech Codec -                              */
/*     Option Candidate for North American Wideband CDMA Digital      */
/*     Cellular Telephony.                                            */
/*                                                                    */
/* (C) Copyright 1994-1995, QUALCOMM Incorporated                     */
/* QUALCOMM Incorporated                                              */
/* 6455 Lusk Blvd                                                     */
/* San Diego, CA 92121                                                */
/*                                                                    */
/* Note:  Reproduction and use of this software for the design and    */
/*     development of North American Wideband CDMA Digital            */
/*     Cellular Telephony Standards is authorized by                  */
/*     QUALCOMM Incorporated.  QUALCOMM Incorporated does not         */
/*     authorize the use of this software for any other purpose.      */
/*                                                                    */
/*     The availability of this software does not provide any license */
/*     by implication, estoppel, or otherwise under any patent rights */
/*     of QUALCOMM Incorporated or others covering any use of the     */
/*     contents herein.                                               */
/*                                                                    */
/*     Any copies of this software or derivative works must include   */
/*     this and all other proprietary notices.                        */
/**********************************************************************/
/* rate_integrate.h - basic structures for celp coder                 */
/**********************************************************************/

#ifndef _RDA_H_
#define _RDA_H_

#include <stdio.h>


			/* a hangover can occur                     */
static int hangover0FB[TLEVELS] = { 7, 7, 7, 3, 0, 0, 0, 0 };
static int hangover1FB[TLEVELS] = { 4, 4, 4, 4, 4, 4, 4, 4 };
static int hangover0[TLEVELS] = { 5, 5, 4, 3, 0, 0, 0, 0 };
static int hangover1[TLEVELS] = { 4, 4, 2, 2, 0, 0, 0, 0 };
static int hangover2[TLEVELS] = { 4, 3, 2, 1, 0, 0, 0, 0 };

			/* hangover as a function of signal to noise */
			/*ratio                                      */

static float THRESH_SNR0_FB[FREQBANDS][TLEVELS][2] = {
/* low band (0-2 kHz) thresholds */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4, 20.9},
     {11.0 * 2, 25.5 * 2},
     {31.6 * 2, 79.6 * 2}},
/* mid band (2-4 kHz) thresholds, these are same as above */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4, 20.9},
     {11.0 * 2, 25.5 * 2},
     {31.6 * 2, 79.6 * 2}},
/* high band (4-7 kHz) thresholds, these are same as above */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4, 20.9},
     {11.0 * 2, 25.5 * 2},
     {31.6 * 2, 79.6 * 2}}
};
static float THRESH_SNR0[FREQBANDS_NB][TLEVELS][2] = {
/* low band thresholds */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4, 20.9},
     {11.0 * 2, 25.5 * 2},
     {31.6 * 2, 79.6 * 2}},
/* high band thresholds, these are same as above */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4, 20.9},
     {11.0 * 2, 25.5 * 2},
     {31.6 * 2, 79.6 * 2}}
};
static float THRESH_SNR1[FREQBANDS][TLEVELS][2] = {
/* low band thresholds */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4 * 1.2, 20.9 * 1.2},
     {11.0 * 3, 25.5 * 3},
     {31.6 * 4, 79.6 * 4}},
/* high band thresholds, these are same as above */
    {{7.0, 9.0},
     {7.0, 12.6},
     {8.0, 17.0},
     {8.6, 18.5},
     {8.9, 19.4},
     {9.4 * 1.2, 20.9 * 1.2},
     {11.0 * 3, 25.5 * 3},
     {31.6 * 4, 79.6 * 4}}
};
static float THRESH_SNR2[FREQBANDS][TLEVELS][2] = {
/* low band thresholds */
    {{7.0, 9.0},
     {8.7, 15.8},
     {9.0, 18.5},
     {10.0, 20.0},
     {10.6, 22.8},
     {13.8, 31.6},
     {50.2, 158.5},
     {200, 631}},
/* high band thresholds, these are same as above */
    {{7.0, 9.0},
     {8.7, 15.8},
     {9.0, 18.5},
     {10.0, 20.0},
     {10.6, 22.8},
     {13.8, 31.6},
     {50.2, 158.5},
     {200, 631}}
};

static float LOWEST_LEVEL[FREQBANDS] = { 10.0 * 16, 5.0 * 16 };


static float rate_filt[FREQBANDS][FILTERORDER] = {
    {				/* .2-2K bandpass filter, cut low end for noise immunity */
     -5.557699E-002,
     -7.216371E-002,
     -1.036934E-002,
     2.344730E-002,
     -6.071820E-002,
     -1.398958E-001,
     -1.225667E-002,
     2.799153E-001,
     4.375000E-001,
     2.799153E-001,
     -1.225667E-002,
     -1.398958E-001,
     -6.071820E-002,
     2.344730E-002,
     -1.036934E-002,
     -7.216371E-002,
     -5.557699E-002,
     },
    {
     /* 2-4k HPF */
     -1.229538E-002,
     4.376551E-002,
     1.238467E-002,
     -6.243877E-002,
     -1.244865E-002,
     1.053678E-001,
     1.248720E-002,
     -3.180645E-001,
     4.875000E-001,
     -3.180645E-001,
     1.248720E-002,
     1.053678E-001,
     -1.244865E-002,
     -6.243877E-002,
     1.238467E-002,
     4.376551E-002,
     -1.229538E-002}
};
#endif
