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


#include <stdio.h>

#include "globs.h"
#include "rom.h"
#include "proto.h"
#include "struct.h"
#include "proto_new.h"
#include "UB_analysis_windows.h"

void
FGV_MEM::noise_synthesis (struct QUANTIZED_HB p, float *outbuf,
			  short first_time)
{

    int i, j;
    float *noise = new float[154];

    float tmplsf[LPC_ORD_WB], Acoef[LPC_ORD_WB], NoiseGain, s1, s2;
    short voiced;
    float op[UB_FRAMESIZE + OVERLAP_7];

    float beta, betaGain, betaGainFrame;
    static float Prev_LSFs[LPC_ORD_WB], lsf_dist;
    static float smoothGain, smoothGainFrame;
    static int prevRate;

    float Tilt;

    /* setting up and initializing structure for noise generator */
    GEN_PULSES gen_pulses_dec = { mem1, rhpf_filt_mem1 };
    RES_DOWN_HB_28 res_down_dec = { state, memup, memdown };
    GEN_SHP_NOISE gen_shape_noise =
	{ rapf_filt_mem, &gen_pulses_dec, &res_down_dec, memdown1, stateExc,
noise_iir, stateExcW, &noise_synthesis_Seed };

    if (first_time == 0) {
	for (i = 0; i < 6; i++)
	    mem1[i] = 0;
	//first_time = 0;
	rhpf_filt_mem1[0] = 0;
	rhpf_filt_mem1[1] = 0;
	for (i = 0; i < 24; i++)
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

	for (i = 0; i < OVERLAP_7; i++)
	    Synstate[i] = 0;

	noise_iir[0] = 0;
	prev_mode = 0;

	for (i = 0; i < LPC_ORD_WB; i++) {
	    Prev_LSFs[i] = p.LSFs[i];
	}
	lsf_dist = 0;

	smoothGain = p.Gain[0];
	prevRate = 8;
	GainState = 1e-3;

    }

    /*Calculate the change in LSFs and the GainFrame */
    lsf_dist = 0.000001;
    for (i = 0; i < LPC_ORD_WB; i++)
	lsf_dist += (p.LSFs[i] - Prev_LSFs[i]) * (p.LSFs[i] - Prev_LSFs[i]);




    /*Smooth the parameters if no rate transitions. Smoothing of the LSFs is stronger for Quarter rate */
    if (data_packet.PACKET_RATE == 2 && prevRate == 2) {
	beta = (100 / (pow (lsf_dist * NFACT, -2) + 100));
	p.Gain[0] = (0.12 * smoothGain + 0.88 * p.Gain[0]);

	for (i = 1; i < 4; i++)
	    p.Gain[i] = (0.88 * p.Gain[i] + 0.12 * p.Gain[i - 1]);
	smoothGain = p.Gain[4];
    }
    else {
	if ((data_packet.PACKET_RATE == 4) && prevRate == 4) {
	    beta = (400 / (pow (lsf_dist * NFACT, -2) + 400));
	    p.Gain[0] = (0.0 * smoothGain + 1 * p.Gain[0]);
	    for (i = 1; i < 4; i++)
		p.Gain[i] = (1 * p.Gain[i] + 0.0 * p.Gain[i - 1]);
	    smoothGain = p.Gain[4];
	}
	else {
	    beta = 1.0;
	    betaGainFrame = 1.0;
	    lsf_dist = 0;
	}
    }



    UB_DTX (&p);

    /* after erasure, in case of large pitch gain,and assuming the frame 
       before the previous wasn't 8th rate, be careful with HB gain       */
    if (data_packet.PACKET_RATE == 4 && prevRate == 14
	&& p.GainFrame > prev_GainFrame && prev_GainFrame > 0.0) {
	float tmpGain;

	tmpGain = p.GainFrame / (2.0 + 2.0 * PitchGain);
	if (tmpGain < prev_GainFrame)
	    p.GainFrame = prev_GainFrame;
	else
	    p.GainFrame = tmpGain;
    }

    prevRate = data_packet.PACKET_RATE;

    for (i = 0; i < (UB_FRAMESIZE + OVERLAP_7); i++)
	op[i] = 0;

    // invoke erasure processing. In case of erasure, the quantized upperband parameters are updated
    WB_Erasure_processing (&p, data_packet.PACKET_RATE, first_time);


    for (i = 0; i < LPC_ORD_WB; i++)
	tmplsf[i] = ((beta) * p.LSFs[i] + (1 - beta) * Prev_LSFs[i]);




    /*derive parameters from unpacked narrowband received packets */
    /* Derive the tilt parameter */
    float refl[10], pci_temp[11];

    lsp2a (pci_temp, lsp, ORDER);
    LPC_pred2refl (pci_temp, refl, ORDER);


    Tilt = refl[0];



    lsp2a (Acoef, tmplsf, LPC_ORD_WB);

    /*smoothing of the LSFs and the gainframe */

    for (i = 0; i < LPC_ORD_WB; i++) {
	Prev_LSFs[i] = ((beta) * p.LSFs[i] + (1 - beta) * Prev_LSFs[i]);
    }

    if (first_time == 1) {
	smoothGain = p.Gain[4];
    }



    if (((data_packet.PACKET_RATE == 4)) && (Tilt > -0.8)
	&& (PitchGain < 0.4)
	&& (first_time == 1))
	voiced = 0;
    else
	voiced = 1;


    /*voicing decision under erasure */

    if (data_packet.PACKET_RATE == 14) {
	if (first_time == 0) {
	    voiced = 0;
	}
	else {
	    if ((prev_mode1 >= 4) && (Tilt > -0.8)
		&& (PitchGain < 0.4))
		voiced = 0;
	    else
		voiced = 1;

	}
    }


    //less noise when voiced with low pitch frequencies (male voices)
    NoiseGain = 1 - (Min (PitchGain, 1) * delay / (60 + delay));
    NoiseGain = rint (NoiseGain * 100000) / 100000.0;	// added this to avoid 6th decimal place non-bit exactness


    gen_shaped_noise (&LB_Excitation[0], voiced, NoiseGain, Acoef,
		      &gen_shape_noise, noise);


    for (j = 0; j < NUM_SUBFRAMES_WB; j++) {
	for (i = 0; i < SUBWIN_LEN; i++)
	    op[i + (j * SUBFR_SHIFT)] =
		op[i + (j * SUBFR_SHIFT)] + noise[i + (j * SUBFR_SHIFT)]
		* subwin[i] * subwin[i] * p.Gain[j];
    }
    s1 = 0;
    s2 = 0;
    for (i = 0; i < (UB_FRAMESIZE + OVERLAP_7); i++) {
	s1 = s1 + (pow ((noise[i] * win[i]), 2));
	s2 = s2 + op[i] * op[i];
    }
    for (i = 0; i < (UB_FRAMESIZE + OVERLAP_7); i++) {
	op[i] = op[i] * p.GainFrame * sqrt (s1 / (s2 + 1));	//noise_win
    }


    for (i = 0; i < OVERLAP_7; i++)
	op[i] = op[i] + Synstate[i];

    for (i = 0; i < (UB_FRAMESIZE); i++)
	outbuf[i] = op[i];

    /* Updating state for overlap add */
    for (i = 0; i < OVERLAP_7; i++)
	Synstate[i] = op[UB_FRAMESIZE + i];


    prev_mode1 = data_packet.PACKET_RATE;


    delete[]noise;
}
