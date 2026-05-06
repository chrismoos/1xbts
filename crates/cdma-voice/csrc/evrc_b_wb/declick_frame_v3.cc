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
#include <stdlib.h>
#include "defines.h"
#include "struct.h"		//includes above
#include "filt.h"
#include "vectors.h"


#define AF 0.7

void fwd_smooth (float *in, float initial, float decay, float *out,
		 int NData);
void bck_smooth (float *in, float decay, float *out, int NData);
void log_envelope_1KHz (float *in, float *out, int *nxt_str, int NData,
			int start, int factor, int fr_size);
void
return_largest (float a, float b, float c, float *largest)
{
    if (a >= b)
	*largest = a >= c ? a : c;
    else
	*largest = b >= c ? b : c;
}

void
return_largest_4 (float a, float b, float c, float d, float *largest)
{
    *largest = a;
    if (*largest < b)
	*largest = b;
    if (*largest < c)
	*largest = c;
    if (*largest < d)
	*largest = d;

}

void
FGV_MEM::declick_frame (float *xLB_frame, float *xHB_frame, float *dcl_frame,
			int mshf, short mode)
{


    float LBenvLP_frame[LB_FRAMESIZE + LOOKAHEAD_8 - 1],
	HBenvLP_frame[UB_FRAMESIZE + LOOKAHEAD_7 - 1];
    float LBenvLP_BW_frame[LB_FRAMESIZE + LOOKAHEAD_8 - 1],
	HBenvLP_BW_frame[UB_FRAMESIZE + LOOKAHEAD_7 - 1], lergers;

    float LBenvLPdB_frame[(LB_FRAMESIZE + LOOKAHEAD_8 - 1) / 8 + 1],
	HBenvLPdB_frame[(UB_FRAMESIZE + LOOKAHEAD_7 - 1) / 7 + 1];
    float LBenvLP_BWdB_frame[(LB_FRAMESIZE + LOOKAHEAD_8 - 1) / 8 + 1],
	HBenvLP_BWdB_frame[(UB_FRAMESIZE + LOOKAHEAD_7 - 1) / 7 + 1];
    float attn_frame[DS_ATTN_SIZE],
	x_attn_frame[2 * UB_FRAMESIZE + LOOKAHEAD_7 - 1];

    float LB2nd_der_frame[DS_FRAME_SIZE], HB2nd_der_frame[DS_FRAME_SIZE],
	HB2nd_der2_frame[DS_FRAME_SIZE], temp, temp1, temp2, temp_str[2];
    int i, k, n;


    if (declick_frame_FirstTime == 0) {	/* allocate memory for static vaiables and initialize the variables */


	LB2nd_der_frame_save = 0;

	start_dsLP = 0;
	start_dsLP_BW = 0;
	start_dsHP = 0;
	start_dsHP_BW = 0;

	smooth_LB_ini = 0;	/*for the forward smoothing, this the initalization of the smoothing parameter */
	smooth_HB_ini = 0;

	LB_filter_state.mem = (double *) calloc (3, sizeof (double));

	LB_filter_state.order = 3;
	LB_filter_state.numord = 3;
	LB_filter_state.denord = 1;

	HB_filter_state.mem = (double *) calloc (2, sizeof (double));
	HB_filter_state.order = 2;
	HB_filter_state.numord = 2;
	HB_filter_state.denord = 2;



    }
    if (declick_frame_FirstTime == 0) {

	polezero_filter (&(xLB_frame[0]), &(xLB_hpf_frame[0]),
			 LB_FRAMESIZE + LOOKAHEAD_8 - 1, &(LB_FILTER_NR[0]),
			 &(LB_FILTER_DR[0]), LB_filter_state);

	polezero_filter (&(xHB_frame[0]), &(xHB_hpf_frame[0]),
			 UB_FRAMESIZE + LOOKAHEAD_7 - 1, &(HB_FILTER_NR[0]),
			 &(HB_FILTER_DR[0]), HB_filter_state);
	declick_frame_FirstTime = 1;
    }
    else {
	polezero_filter (&(xLB_frame[LOOKAHEAD_8 - 1]),
			 &(xLB_hpf_frame[LOOKAHEAD_8 - 1]), LB_FRAMESIZE,
			 &(LB_FILTER_NR[0]), &(LB_FILTER_DR[0]),
			 LB_filter_state);
	polezero_filter (&(xHB_frame[LOOKAHEAD_7 - 1]),
			 &(xHB_hpf_frame[LOOKAHEAD_7 - 1]), UB_FRAMESIZE,
			 &(HB_FILTER_NR[0]), &(HB_FILTER_DR[0]),
			 HB_filter_state);

    }




    /* Forward and backward smoothing for the lower band */
    fwd_smooth (&(xLB_hpf_frame[0]), smooth_LB_ini, DECAY_LB,
		&(LBenvLP_frame[0]), LB_FRAMESIZE + LOOKAHEAD_8 - 1);
    smooth_LB_ini = (int) LBenvLP_frame[LB_FRAMESIZE - 1];



    bck_smooth (&(xLB_hpf_frame[0]), DECAY_LB, &(LBenvLP_BW_frame[0]),
		LB_FRAMESIZE + LOOKAHEAD_8 - 1);




    /* Forward and backward smoothing for the upper band */
    fwd_smooth (&(xHB_hpf_frame[0]), smooth_HB_ini, DECAY_HB,
		&(HBenvLP_frame[0]), UB_FRAMESIZE + LOOKAHEAD_7 - 1);
    smooth_HB_ini = (int) HBenvLP_frame[UB_FRAMESIZE - 1];
    bck_smooth (&(xHB_hpf_frame[0]), DECAY_HB, &(HBenvLP_BW_frame[0]),
		UB_FRAMESIZE + LOOKAHEAD_7 - 1);




    /* Down smple to 1KHz and convert to logarithm */
    log_envelope_1KHz (HBenvLP_BW_frame, HBenvLP_BWdB_frame, &(start_dsHP_BW),
		       UB_FRAMESIZE + LOOKAHEAD_7 - 1, start_dsHP_BW,
		       HB_DEC_FAC, UB_FRAMESIZE);
    log_envelope_1KHz (LBenvLP_frame, LBenvLPdB_frame, &(start_dsLP),
		       LB_FRAMESIZE + LOOKAHEAD_8 - 1, start_dsLP, LB_DEC_FAC,
		       LB_FRAMESIZE);
    log_envelope_1KHz (HBenvLP_frame, HBenvLPdB_frame, &(start_dsHP),
		       UB_FRAMESIZE + LOOKAHEAD_7 - 1, start_dsHP, HB_DEC_FAC,
		       UB_FRAMESIZE);
    log_envelope_1KHz (LBenvLP_BW_frame, LBenvLP_BWdB_frame, &(start_dsLP_BW),
		       LB_FRAMESIZE + LOOKAHEAD_8 - 1, start_dsLP_BW,
		       LB_DEC_FAC, LB_FRAMESIZE);






    /* Detection */
    for (i = 0; i < DS_FRAME_SIZE; i++) {
	if (i < 4) {
	    temp =
		LBenvLPdB_frame[i + 1] - LBenvLPdB_frame_save[i] +
		LBenvLP_BWdB_frame[i] - LBenvLP_BWdB_frame[i + 4];
	}
	else {
	    temp =
		LBenvLPdB_frame[i + 1] - LBenvLPdB_frame[i - 3] +
		LBenvLP_BWdB_frame[i] - LBenvLP_BWdB_frame[i + 4];
	}
	LB2nd_der_frame[i] = temp >= 0 ? 0.5 * temp : 0;
    }



    for (i = 0; i < 4; i++)
	LBenvLPdB_frame_save[i] = LBenvLPdB_frame[i + 17];


    for (i = 0; i < DS_FRAME_SIZE; i++) {
	if (i < 4) {
	    temp1 =
		(HBenvLPdB_frame[i + 1] - HBenvLPdB_frame_save[i]) >
		0 ? (HBenvLPdB_frame[i + 1] - HBenvLPdB_frame_save[i]) : 0;
	    temp2 =
		(HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4]) >
		0 ? (HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4]) : 0;

	}
	else {
	    temp1 =
		(HBenvLPdB_frame[i + 1] - HBenvLPdB_frame[i - 3]) >
		0 ? (HBenvLPdB_frame[i + 1] - HBenvLPdB_frame[i - 3]) : 0;
	    temp2 =
		(HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4]) >
		0 ? (HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4]) : 0;
	}
	HB2nd_der_frame[i] = pow (double (temp1 * temp2), 0.5);
    }
    for (i = 0; i < 4; i++)
	HBenvLPdB_frame_save[i] = HBenvLPdB_frame[i + 17];



    /*max smoothing */
    for (i = 0; i < DS_FRAME_SIZE; i++) {
	if (i == 0)
	    return_largest (LB2nd_der_frame_save, LB2nd_der_frame[i],
			    LB2nd_der_frame[i + 1], &temp_str[0 % 2]);
	else {
	    if (i < DS_FRAME_SIZE - 1)
		return_largest (LB2nd_der_frame[i - 1], LB2nd_der_frame[i],
				LB2nd_der_frame[i + 1], &temp_str[i % 2]);
	    else
		return_largest (LB2nd_der_frame[i - 1], LB2nd_der_frame[i], 0,
				&temp_str[i % 2]);
	}
	if (i > 0)
	    LB2nd_der_frame[i - 1] = temp_str[(i - 1) % 2];

    }
    LB2nd_der_frame[DS_FRAME_SIZE - 1] = temp_str[(DS_FRAME_SIZE - 1) % 2];
    LB2nd_der_frame_save = LB2nd_der_frame[DS_PRES_SIZE - 1];


    for (i = DS_PRES_SIZE + comp_shift; i < DS_ATTN_SIZE; i++) {
	pulse_msr[i] =
	    (HB2nd_der_frame[i - DS_PRES_SIZE] -
	     LB2nd_der_frame[i - DS_PRES_SIZE]) >
	    0 ? (HB2nd_der_frame[i - DS_PRES_SIZE] -
		 LB2nd_der_frame[i - DS_PRES_SIZE]) : 0;
    }
    pulse_msr[0] = 0;
    pulse_msr[1] = 0;		//pulse_msr[DS_ATTN_SIZE-2]=0;pulse_msr[DS_ATTN_SIZE-1]=0;


    for (i = 0; i < DS_FRAME_SIZE; i++) {
	if (i < 4) {
	    temp =
		HBenvLPdB_frame[i + 1] - HBenvLPdB_frame_save2[i] +
		HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4];
	}
	else {
	    temp =
		HBenvLPdB_frame[i + 1] - HBenvLPdB_frame[i - 3] +
		HBenvLP_BWdB_frame[i] - HBenvLP_BWdB_frame[i + 4];
	}
	HB2nd_der2_frame[i] = temp >= 0 ? 0.5 * temp : 0;
    }
    for (i = 0; i < 4; i++)
	HBenvLPdB_frame_save2[i] = HBenvLPdB_frame[i + 17];

    for (i = DS_PRES_SIZE + comp_shift; i < DS_ATTN_SIZE; i++) {
	excess_lvl_frame[i] =
	    (HB2nd_der2_frame[i - DS_PRES_SIZE] -
	     LB2nd_der_frame[i - DS_PRES_SIZE]) >
	    0 ? (HB2nd_der2_frame[i - DS_PRES_SIZE] -
		 LB2nd_der_frame[i - DS_PRES_SIZE]) : 0;
    }


    for (k = 40; k < DS_ATTN_SIZE; k++)
	excess_lvl_frame[k] = excess_lvl_frame[k] * pow (AF, k - 40);

    for (i = 0; i < DS_ATTN_SIZE; i++)
	attn_frame[i] = 0.0;



    for (i = 2; i < DS_ATTN_SIZE - 2; i++) {
	if (pulse_msr[i] > THRESHOLD_DCL) {
	    return_largest_4 (pulse_msr[i - 2], pulse_msr[i - 1],
			      pulse_msr[i + 1], pulse_msr[i + 2], &lergers);
	    if (pulse_msr[i] > lergers) {
		for (k = -2; k <= +2; k = k + 1) {
		    attn_frame[i + k] =
			20 * (1 -
			      (2 / (1 + exp (excess_lvl_frame[i + k] / 10))));

		}
	    }
	}
    }

    if (pulse_msr[DS_ATTN_SIZE - 2] > THRESHOLD_DCL) {
	return_largest (pulse_msr[DS_ATTN_SIZE - 4],
			pulse_msr[DS_ATTN_SIZE - 3],
			pulse_msr[DS_ATTN_SIZE - 1], &lergers);
	if (pulse_msr[DS_ATTN_SIZE - 2] > lergers) {
	    for (k = -2; k <= +1; k = k + 1) {
		attn_frame[DS_ATTN_SIZE - 2 + k] =
		    20 * (1 -
			  (2 /
			   (1 +
			    exp (excess_lvl_frame[DS_ATTN_SIZE - 2 + k] /
				 10))));
	    }
	}
    }

    if (pulse_msr[DS_ATTN_SIZE - 1] > THRESHOLD_DCL) {
	return_largest (pulse_msr[DS_ATTN_SIZE - 3],
			pulse_msr[DS_ATTN_SIZE - 2], 0, &lergers);
	if (pulse_msr[DS_ATTN_SIZE - 2] > lergers) {
	    for (k = -2; k <= 0; k = k + 1) {
		attn_frame[DS_ATTN_SIZE - 2 + k] =
		    20 * (1 -
			  (2 /
			   (1 +
			    exp (excess_lvl_frame[DS_ATTN_SIZE - 2 + k] /
				 10))));
	    }
	}
    }




    for (i = 0; i < 2 * UB_FRAMESIZE + LOOKAHEAD_7 - 1; i++) {
	x_attn_frame[i] = 0.0;
    }
    for (i = 0; i < DS_ATTN_SIZE; i++) {
	if (attn_frame[i] > 0) {
	    for (k = -6; k <= 6; k++) {
		x_attn_frame[7 * (i + 1) - 1 + k] +=
		    Gain_window[k + 6] * attn_frame[i];
	    }
	}

    }



    for (i = 0; i < UB_FRAMESIZE + LOOKAHEAD_7 - 1; i++) {
	dcl_frame[i] =
	    xHB_frame[i] * pow (10.0,
				double (-x_attn_frame[i + UB_FRAMESIZE] /
					20));
    }




    /*update for next frame */
    vcpy (&(xLB_hpf_frame[LB_FRAMESIZE]), &(xLB_hpf_frame[0]),
	  LOOKAHEAD_8 - 1);
    vcpy (&(xHB_hpf_frame[UB_FRAMESIZE]), &(xHB_hpf_frame[0]),
	  LOOKAHEAD_7 - 1);
    vcpy (&(pulse_msr[DS_PRES_SIZE]), &(pulse_msr[0]), DS_PRES_SIZE + 5);
    vcpy (&(excess_lvl_frame[DS_PRES_SIZE]), &(excess_lvl_frame[0]),
	  DS_PRES_SIZE + 5);


    /*update the shift parameter so that it can be used in the next frame */
    comp_shift = (int) (((float) (-mshf + 14) / 7) + 0.5);	//-7
    comp_shift = comp_shift < 5 ? comp_shift : 5;
    comp_shift = comp_shift > 0 ? comp_shift : 0;


}
