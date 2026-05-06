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

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include "WB_Buffer_sizes_8thRate.h"
#include "macro_new.h"
#include "WB_analysis_windows_8thRate.h"
#include "vectors.h"
#include "filt.h"
//#include "lpcana_ExtWin.h"
#include "proto.h"
#include "globs.h"
#include "struct.h"
//#include "WB_Analysis_struc.h"
#include "proto_new.h"

#define FDBACK 0.95

/*Calculate the standard deviation*/
void
standard_dev (float *input, float *sd, int length)
{
    float mn = 0.0;
    int i;

    for (i = 0; i < length; i++)
	mn = mn + input[i];

    mn = mn / length;

    *sd = 0;
    for (i = 0; i < length; i++)
	*sd += (input[i] - mn) * (input[i] - mn);
    *sd = *sd / (length - 1);
    *sd = sqrt (*sd);


}


/*NOTE that this code performs the analysis for a single frame*/

void
FGV_MEM::WB_EncParams_8thRate (struct PARAM_FRAME_4GVLB *p_LB, float *in,
			       struct UnQUANTIZED_HB_8thRate *pHB_8,
			       int FirstTime)
{


    int i, N, j;
    short lpcord = LPC_ORD_WB_8R;

    //static int FirstTime=0;

    //static float x14_frame[BUFFER_LENGTH_8R];  /* Vector to hold the filtered input signal*/

    //float *x_frame_pr, *x6_frame_pr;
    float lsps_8[LPC_ORD_WB_8R];

    //static float prev_lsps_ER[LPC_ORD_WB_8R];


    float feedback;
    float tmp[BUFFER_LENGTH_8R + 100];


    struct POLEZERO_FILTER x14_whiten;

    float lpc_hb_8R[LPC_ORD_WB_8R + 1], rcorr_hb_8R[LPC_ORD_WB_8R + 16],
	rc_hb_8R[LPC_ORD_WB_8R + 2];
    float dummy_nr[1] = { 1.0 };

    //static double lpf_filt_mem1[LPF_8R_NR_ORD];
    struct POLEZERO_FILTER lpf_state =
	{ LPF_8R_NR_ORD, LPF_8R_NR_ORD, LPF_8R_NR_ORD, 0, lpf_filt_mem1 };

    if (FirstTime == 0) {
	for (i = 0; i < LPF_8R_NR_ORD; i++)
	    lpf_filt_mem1[i] = 0.0;
	for (i = 0; i < LPC_ORD_WB_8R; i++)
	    prev_lsps_ER[i] = 0.0;
	for (i = 0; i < FRAME_LENGTH_8R + OVERLAP_8R; i++)
	    x14_frame[i] = 0.0;
    }

    /* initalize other filters and ensure they have zero memory every frame */
    x14_whiten.mem = (double *) calloc (LPC_ORD_WB_8R, sizeof (double));
    x14_whiten.denord = 0;
    x14_whiten.numord = LPC_ORD_WB_8R;
    x14_whiten.order = LPC_ORD_WB_8R;






    /*The first time the filtering is done for the entire frame, including the lookahead */
    if (FirstTime == 0) {
	polezero_filter (&(in[0]), &(x14_frame[0]),
			 FRAME_LENGTH_8R + OVERLAP_8R, &(LPF_8R_NR[0]),
			 &(LPF_8R_DR[0]), lpf_state);
    }
    else {
	polezero_filter (&(in[OVERLAP_8R]), &(x14_frame[OVERLAP_8R]),
			 FRAME_LENGTH_8R, &(LPF_8R_NR[0]), &(LPF_8R_DR[0]),
			 lpf_state);
    }



    /*starting point for the lpc analysis frame is 0  */
    lpcanalys (&(lpc_hb_8R[1]), &(rc_hb_8R[0]), &(x14_frame[0]),
	       LPC_ORD_WB_8R, FRAME_LENGTH_8R + OVERLAP_8R, &(rcorr_hb_8R[0]),
	       &(win_lpc_8[0]));

    /* Expand bandwidth of the lpc coeffs */
    for (i = 1; i <= LPC_ORD_WB_8R; i++) {
	lpc_hb_8R[i] = lpc_hb_8R[i] * pow (0.95, i);
    }
    lpc_hb_8R[0] = 1.0;

    /* whiten the input frame and calculate the standard deviation */

    polezero_filter (&(x14_frame[0]), &(tmp[0]), BUFFER_LENGTH_8R,
		     &(lpc_hb_8R[0]), &(dummy_nr[0]), x14_whiten);



    standard_dev (&(tmp[OVERLAP_8R / 2]), &(pHB_8->Gain), FRAME_LENGTH_8R);



    /* Convert into lsfs and populate the fields in the output structures */
    a2lsp (&(lsps_8[0]), &(lpc_hb_8R[1]), LPC_ORD_WB_8R);
    lsp_weights (&(lsps_8[0]), pHB_8->LSFWeights_8, LPC_ORD_WB_8R);



    /* LSF smoothing, unless rate transition or first frame ever */
    if (FirstTime == 1) {
	feedback = 0.2;

	for (i = 0; i < LPC_ORD_WB_8R; i++)
	    lsps_8[i] =
		feedback * lsps_8[i] + (1 - feedback) * prev_lsps_ER[i];

    }

    for (i = 0; i < LPC_ORD_WB_8R; i++)
	prev_lsps_ER[i] = lsps_8[i];

    for (i = 0; i < LPC_ORD_WB_8R; i++) {
	pHB_8->LSFs_8[i] = lsps_8[i];
    }


/* code for writing out LSF and gain training data */



    vcpy (&(x14_frame[FRAME_LENGTH_8R]), &(x14_frame[0]), OVERLAP_8R);
    free (x14_whiten.mem);

}


void
FGV_MEM::WB_DecParams_8thRate (struct UnQUANTIZED_HB_8thRate *pHB_8,
			       float *out, int FirstTime, int erasure_8R)
{

    int i, N, j;
    short lpcord = LPC_ORD_WB_8R;

    //static int FirstTime=0;

    float lsps_8[LPC_ORD_WB_8R];


    float tmp[BUFFER_LENGTH_8R + 100];
    float random[BUFFER_LENGTH_8R + 100];

    //static long WB_DecParams_8thRate_Seed=0;

    struct POLEZERO_FILTER noise_color;

    float lpc_hb_8R[LPC_ORD_WB_8R + 1];
    float dummy_nr[1] = { 1.0 };

    //static double hpf_filt_mem1[LPF_8R_NR_ORD];
    struct POLEZERO_FILTER hpf_state =
	{ LPF_8R_NR_ORD, LPF_8R_NR_ORD, LPF_8R_NR_ORD, 0, hpf_filt_mem1 };

    //static float prev_lsfs[10], prev_gain;
    /* Allocation of memory for the low pass filter only once */

    if (FirstTime == 0) {
	for (i = 0; i < 10; i++)
	    prev_lsfs[i] = 0.0;
	prev_gain = 0;
    }

    /* initalize other filters and ensure they have zero memory every frame */
    noise_color.mem = (double *) calloc (LPC_ORD_WB_8R, sizeof (double));
    noise_color.denord = LPC_ORD_WB_8R;
    noise_color.numord = 0;
    noise_color.order = LPC_ORD_WB_8R;
    if (erasure_8R == 1) {	//8th rate erasure processing

	pHB_8->Gain = 0.9 * prev_gain;
	for (i = 0; i < 10; i++) {
	    pHB_8->LSFs_8[i] = (0.8 * prev_lsfs[i]) + (0.01 * (i + 1));
	}
    }


    for (i = 0; i < LPC_ORD_WB_8R; i++) {
	lsps_8[i] = pHB_8->LSFs_8[i];
    }

    lsp2a (&(lpc_hb_8R[1]), &(lsps_8[0]), LPC_ORD_WB_8R);
    lpc_hb_8R[0] = 1.0;




    /* Update the LSFs for next frame smoothing */

    for (i = 0; i < BUFFER_LENGTH_8R + 100; i++)
	random[i] = pHB_8->Gain * ran_g (&WB_DecParams_8thRate_Seed);


    polezero_filter (&(random[0]), &(tmp[0]), BUFFER_LENGTH_8R + 100,
		     &(dummy_nr[0]), &(lpc_hb_8R[0]), noise_color);



    /* high-pass filter */
    polezero_filter (tmp, tmp, BUFFER_LENGTH_8R + 100, LPF_8R_NR, LPF_8R_DR,
		     hpf_state);

    /* Updating state for overlap add */
    for (i = 0; i < 280; i++)
	out[i] = tmp[52 + i] * win_8R[i];

    for (i = 0; i < 28; i++)
	out[i] += Synstate14[i] * win_8R[280 + i];

    for (i = 0; i < 28 + 48; i++)
	Synstate14[i] = tmp[280 + 52 + i];

    for (i = 0; i < LPC_ORD_WB_8R; i++) {	//back up parameters in case of erasures
	prev_lsfs[i] = pHB_8->LSFs_8[i];
    }
    prev_gain = pHB_8->Gain;


    free (noise_color.mem);
}
