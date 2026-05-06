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

#ifdef WIN32
#pragma warning( once : 4305 )
#endif

#define BUFFER_LENGTH_7 350
#define FRAME_START_7   140
#define LOOKAHEAD_7     70
#define FRAME_START_8   160
#define FRAME_SIZE_8    160
#define LOOKAHEAD_8     80
#define LPC_ANA_OVERLAP 14

#define NUM_SUBFRAMES_WB 5
#define SUBWIN_LEN 42
#define SUBFR_SHIFT 28

/*Filter orders for wideband analyisis*/
#define LPF_NR_ORD 4
#define LPF_DR_ORD 4
#define LPC_ORD_WB 6

/*Filter orders for declicking*/
#define LB_FILTER_NRORD 3
#define LB_FILTER_DRORD 1

#define HB_FILTER_NRORD 2
#define HB_FILTER_DRORD 2


/* Parameters for the declicking algorithm*/
#define DECAY_LB 0.95
#define DECAY_HB 0.94

#define LB_DEC_FAC 8
#define HB_DEC_FAC 7
#define DS_FRAME_SIZE 25
#define DS_PRES_SIZE 20
#define THRESHOLD_DCL 6
#define DS_ATTN_SIZE 45


static float LPF_NR[5] = { 0.7400, 2.8971, 4.3145, 2.8971, 0.7400 };
static float LPF_DR[5] = { 1.0000, 3.3250, 4.2516, 2.4681, 0.5460 };
static int offset_sel[5] = { 0, 0, 1, 2, 2 };

static float LB_FILTER_NR[4] = { 1, 0.96, -0.96, -1 };
static float LB_FILTER_DR[2] = { 1, -0.5 };
static float HB_FILTER_NR[3] = { 0.5, 1, 0.5 };
static float HB_FILTER_DR[3] = { 1, 0.5, 0.3 };

static float Gain_window[13] = { 0.049516, 0.188255, 0.388740, 0.611260,
    0.811745, 0.950484, 1.000000, 0.950484, 0.811745, 0.611260, 0.388740,
	0.188255, 0.049516
};
