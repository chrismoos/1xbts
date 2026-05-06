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
#include "UB_analysis_windows.h"
#include "struct.h"
#include "vectors.h"
#include "filt.h"
#include "vectors.h"

#include "proto.h"
#include "proto_new.h"

#define FDBACK 0.95
#define FDBACK_L 0.25


void
FGV_MEM::WB_EncParams (struct PARAM_FRAME_4GVLB *p_LB, float *in,
		       float *shift, struct UnQUANTIZED_HB *p_HB,
		       float *LSFdistance, int FirstTime)
{
    int i, N, j, k;
    const short lpcord = LPC_ORD_WB;

    int mshf, shift_sf;		/* mean of the shift and shifts for the subframe */

    float lsps[LPC_ORD_WB], temp;

    float lsfdist = 0, alfa, NoiseGain;
    int voiced;
    int subframe;
    float nrg[1], nrg_syn[1], Nrg[NUM_SUBFRAMES_WB],
	Nrg_syn[NUM_SUBFRAMES_WB], feedback, normFact;
    float noise_syn[UB_FRAMESIZE + LPC_ANA_OVERLAP];

    float lpc[7], noise_gain[2];
    float ninpf[140];

    struct POLEZERO_FILTER lpf_state =
	{ LPF_NR_ORD, LPF_NR_ORD, LPF_DR_ORD, 0, filt_mem };
    struct GEN_PULSES gen_pulses_dec = { mem1, rhpf_filt_mem1 };
    struct RES_DOWN_HB_28 res_down_dec = { state, memup, memdown };
    struct GEN_SHP_NOISE gen_shape_noise =
	{ rapf_filt_mem, &gen_pulses_dec, &res_down_dec, memdown1, stateExc,
noise_iir, stateExcW, &WB_EncParams_Seed };

  /**************************************************/

    float lpc_hb[LPC_ORD_WB], rcorr_hb[LPC_ORD_WB + 7], rc_hb[LPC_ORD_WB];

	/*****Initialize the memory for gen shaped noise*/
    if (FirstTime == 0) {
	for (i = 0; i < 6; i++)
	    mem1[i] = 0;


	rhpf_filt_mem1[0] = 0;
	rhpf_filt_mem1[1] = 0;
	for (i = 0; i < LEN_DN_HB - 2; i++)
	    state[i] = 0;
	for (i = 0; i < 4; i++)
	    memup[i] = 0;
	for (i = 0; i < 7; i++) {
	    memdown[i] = 0;
	    memdown1[i] = 0;
	}
	for (i = 0; i < 4; i++)
	    stateExc[i] = 0;
	for (i = 0; i < 18; i++)
	    rapf_filt_mem[i] = 0;
	for (i = 0; i < 54; i++)
	    stateExcW[i] = 0;

	for (i = 0; i < LPF_NR_ORD; i++) {
	    filt_mem[i] = 0.0;
	    saved_mem[i] = 0.0;
	}
	noise_iir[0] = 0;	//initialization bug-fix


	/* clear input memory */
	for (i = 0; i < FRAME_START_7; i++)
	    x_frame[i] = 0.0;
	/* clear filtered-input memory */
	for (i = 0; i < FRAME_START_7; i++)
	    x6_frame[i] = 0.0;

	/* zero first part of signal, because HB excitation is mostly zero */
	for (i = 0; i < 24; i++)
	    in[i] = 0.0;
	/* taper input samples */
	for (i = 0; i < 14; i++)
	    in[i + 24] *= win[i];

	GainState = 1e-3;
	wb_enc_previous_mode = 1;

    }

    /* copy the input to the location in the frame */
    vcpy (in, &(x_frame[FRAME_START_7]), UB_FRAMESIZE + LOOKAHEAD_7);




    for (i = 0; i < LPF_NR_ORD; i++) {
	filt_mem[i] = saved_mem[i];
    }

    /* Filter the present frame and save the state at the end of the present frame */
    polezero_filter (&(x_frame[FRAME_START_7]), &(x6_frame[FRAME_START_7]),
		     UB_FRAMESIZE, &(LPF_NR[0]), &(LPF_DR[0]), lpf_state);

    for (i = 0; i < LPF_NR_ORD; i++) {
	saved_mem[i] = filt_mem[i];
    }

    /* Filter the lookahead, but the state of the filter is overwritten */
    polezero_filter (&(x_frame[2 * UB_FRAMESIZE]),
		     &(x6_frame[2 * UB_FRAMESIZE]), LOOKAHEAD_7, &(LPF_NR[0]),
		     &(LPF_DR[0]), lpf_state);

    if (p_LB->Mode != 1) {

	mshf = (int) ((7 * (shift[0] + shift[1] + shift[2]) / 24) + 0.5);

	/*starting point for the lpc analysis frame is 0 to 153 - mshf */
	lpcanalys (&(lpc_hb[0]), &(rc_hb[0]), &(x_frame[FRAME_START_7 - mshf]), LPC_ORD_WB, UB_FRAMESIZE + LPC_ANA_OVERLAP, &(rcorr_hb[0]), &(win_lpc[0]));	//
	//lpcanalys(&(lpc_hb[0]), &(rc_hb[0]), &(x_frame[FRAME_START_7 - mshf]), LPC_ORD_WB, UB_FRAMESIZE + LPC_ANA_OVERLAP, &(rcorr_hb[0]));   //



	/* Expand bandwidth of the lpc coeffs */
	for (i = 0; i < LPC_ORD_WB; i++) {
	    lpc_hb[i] = lpc_hb[i] * pow (0.95, i + 1);
	}


	/* Convert into lsfs and populate the fiels in the output structures */
	a2lsp (p_HB->LSFs, &(lpc_hb[0]), LPC_ORD_WB);

	/* does not use gardner weights */
	lsp_weights (p_HB->LSFs, p_HB->LSFWeights, LPC_ORD_WB);


	/* LSF smoothing */
	if (FirstTime == 1) {
	    lsfdist = 0.0;
	    for (i = 0; i < LPC_ORD_WB; i++)
		lsfdist =
		    lsfdist +
		    (p_HB->LSFWeights[i] * (prev_lsps[i] - p_HB->LSFs[i]) *
		     (prev_lsps[i] - p_HB->LSFs[i]));

	    lsfdist = lsfdist * NFACT;
	    alfa =
		(1 - FDBACK_L) +
		(FDBACK_L / (1 + exp (-0.25 * (lsfdist - 20))));
	    for (i = 0; i < LPC_ORD_WB; i++)
		p_HB->LSFs[i] =
		    (alfa * p_HB->LSFs[i]) + (1 - alfa) * prev_lsps[i];
	}

	/* Update the LSFs for next frame smoothing */
	for (i = 0; i < LPC_ORD_WB; i++)
	    prev_lsps[i] = p_HB->LSFs[i];

	if (((p_LB->Mode == 4) || (p_LB->Mode == 3)) && (p_LB->Tilt > -0.8)
	    && (p_LB->PitchGain < 0.4) && (FirstTime == 1))
	    voiced = 0;
	else
	    voiced = 1;

	NoiseGain =
	    1 -
	    ((p_LB->PitchGain >
	      1 ? 1 : p_LB->PitchGain) * p_LB->PitchLag / (60 +
							   p_LB->PitchLag));
	NoiseGain = rint (NoiseGain * 100000) / 100000.0;	//added this to avoid 6th decimal place non-bit exactness


	lsp2a (lpc_hb, p_HB->LSFs, LPC_ORD_WB);

	gen_shaped_noise (p_LB->whtnd_la, voiced, NoiseGain, lpc_hb,
			  &gen_shape_noise, noise_syn);


	/* gain for the entire frame */
	ener_calc_win (&(x6_frame[FRAME_START_7]) - mshf, &(win[0]),
		       UB_FRAMESIZE + LPC_ANA_OVERLAP, &(nrg[0]));
	ener_calc_win (&(noise_syn[0]), &(win[0]),
		       UB_FRAMESIZE + LPC_ANA_OVERLAP, &(nrg_syn[0]));



	p_HB->GainFrame = pow (double (nrg[0] / nrg_syn[0]), 0.5);


	/* Gains for each subframe */
	for (subframe = 0; subframe < NUM_SUBFRAMES_WB; subframe++) {
	    shift_sf = (int) ((7 * (shift[offset_sel[subframe]]) / 8) + 0.5);
	    ener_calc (&(x6_frame[FRAME_START_7]) - shift_sf +
		       (subframe * SUBFR_SHIFT), &(subwin[0]), SUBWIN_LEN,
		       &(Nrg[subframe]));
	    ener_calc (&(noise_syn[0]) + (subframe * SUBFR_SHIFT),
		       &(subwin[0]), SUBWIN_LEN, &(Nrg_syn[subframe]));
	    p_HB->Gain[subframe] =
		pow (double (Nrg[subframe] / (Nrg_syn[subframe] + 1)), 0.5);

	    if (p_HB->Gain[subframe] < 10e-12) {
		p_HB->Gain[subframe] = 10e-12;
	    }

	}
      /************ find and update P{K}.Gain*********************/

	/* Adaptive recursive smoothing (more smoothing when gains don't fuctuate much) */

	feedback =
	    (log (p_HB->Gain[0]) - log (GainState)) * (log (p_HB->Gain[0]) -
						       log (GainState));

	for (subframe = 1; subframe < NUM_SUBFRAMES_WB; subframe++) {
	    feedback +=
		(log (p_HB->Gain[subframe]) -
		 log (p_HB->Gain[subframe - 1])) *
		(log (p_HB->Gain[subframe]) - log (p_HB->Gain[subframe - 1]));
	}
	feedback = 0.4 / (1 + 0.5 * feedback);

	normFact = 0;
	int transition = 0;

	if (wb_enc_previous_mode == 2 && ((p_LB->Mode == 4) || p_LB->Mode == 3))
	    transition = 1;

	for (subframe = 0; subframe < NUM_SUBFRAMES_WB; subframe++) {

	    if (transition == 0)
		p_HB->Gain[subframe] =
		    (1 - feedback) * p_HB->Gain[subframe] +
		    feedback * GainState;

	    GainState = p_HB->Gain[subframe];
	    normFact += p_HB->Gain[subframe];
	}

	for (subframe = 0; subframe < NUM_SUBFRAMES_WB; subframe++) {
	    p_HB->Gain[subframe] = p_HB->Gain[subframe] / normFact;

	}
	p_HB->GainFrame = p_HB->GainFrame * (0.8 + 0.5 * feedback);
	/*    if(transition==1)
	   p_HB->GainFrame=p_HB->GainFrame*0.8;
	 */ wb_enc_previous_mode = p_LB->Mode;


    }				// skip full-rate processing , only update x_frame, x6_frame buffers 

    /* update buffers for next frames */

    vcpy (&(x_frame[FRAME_START_7]), &(x_frame[0]), UB_FRAMESIZE);

    vcpy (&(x6_frame[FRAME_START_7]), &(x6_frame[0]), UB_FRAMESIZE);

    *(LSFdistance) = lsfdist;

}
