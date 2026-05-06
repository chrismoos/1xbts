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
    "$Id: ppp.cc,v 1.10.22.3 2007/06/24 06:43:16 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/* Modified PPP for EVRC shell */
#include "defines.h"
#include "struct.h"
#include "filt.h"
#include "globs.h"
#include "proto.h"
int F_BAND[NUM_FIXED_BAND + 1] =
    { 0, 200, 300, 400, 500, 600, 850, 1000, 1200, 1400, 1600,
    1850, 2100, 2375, 2650, 2950, 3250, 4000
};

/* Band locations in the multi-band alignment approach for FPPP */

void
FGV_MEM::ppp_full_encoder (DTFS * CWout, DTFS CWin, const float *curr_lsp)
{
    DTFS target;
    int i;
    float tmplpc[ORDER], w;

    *CWout = CWin;

    //Amplitude Quantization
    CWout->car2pol ();
    //CWout->quant_cw_memless(curr_lsp);
    CWout->quant_cw_memless (curr_lsp, A_POWER_IDX, A_AMP_IDX);

    target = *CWout;

    for (i = 0; i <= CWout->lag >> 1; i++)
	CWout->b[i] = 0.0;
    CWout->pol2car ();
    target.pol2car ();

    for (i = 0, w = 1.0; i < ORDER; i++)
	tmplpc[i] = curr_lsp[i] * (w *= 0.8);
    target.poleFilter (tmplpc, ORDER);
    CWout->poleFilter (tmplpc, ORDER);

    F_ROT_IDX = CWout->glbl_alignment_FPPP (target, 128);
    assert (F_ROT_IDX >= 0 && F_ROT_IDX < 128);
    CWout->phaseShift (-TWO_PI * F_ROT_IDX / 128.0);


    //NEED TO TAKE OUT THIS PIECE OF INEFFICIENT CODE EVENTUALLY
    //IT IS TEMPORARILY SITTING HERE TO PRESERVE BIT EXACTNESS DURING PACKETIZATION
    for (i = 0; i < NUM_FIXED_BAND; i++) {
	if (i < 3) {
	    F_ALIGN_IDX[i] =
		(int) CWout->alignment_band (target, 0.0, (float) F_BAND[i],
					     (float) F_BAND[i + 1],
					     32.0 / CWout->lag, 1.0);
	    if (F_ALIGN_IDX[i] == 9999)
		F_ALIGN_IDX[i] = 0;
	    assert (F_ALIGN_IDX[i] >= 0 && F_ALIGN_IDX[i] < 32);
	}
	else {
	    F_ALIGN_IDX[i] =
		(int) CWout->alignment_band (target, 0.0, (float) F_BAND[i],
					     (float) F_BAND[i + 1],
					     32.0 / CWout->lag, 0.5);
	    if (F_ALIGN_IDX[i] == 9999)
		F_ALIGN_IDX[i] = 0;
	    assert (F_ALIGN_IDX[i] >= 0 && F_ALIGN_IDX[i] < 64);
	}
    }

    for (i = 0; i < NUM_FIXED_BAND; i++) {
	float tmp;
	int j;

	if (i < 3) {
	    tmp = 32.0 / CWout->lag;
	    tmp *= CWout->lag / 2.0;
	    for (tmp = -tmp, j = 0; j < F_ALIGN_IDX[i]; j++, tmp += 1.0);
	    CWout->phaseShift_band (-TWO_PI * tmp / CWout->lag,
				    (float) F_BAND[i], (float) F_BAND[i + 1]);
	}
	else {
	    tmp = 32.0 / CWout->lag;
	    tmp *= CWout->lag / 2.0;
	    for (tmp = -tmp, j = 0; j < F_ALIGN_IDX[i]; j++, tmp += 0.5);
	    CWout->phaseShift_band (-TWO_PI * tmp / CWout->lag,
				    (float) F_BAND[i], (float) F_BAND[i + 1]);
	}
    }
    CWout->zeroFilter (tmplpc, ORDER);
}

void
FGV_MEM::ppp_full_decoder (DTFS * CWout, const float *curr_lsp)
{
    int i;
    float tmplpc[ORDER], w;

    //Amplitude Dequantization
    //CWout->dequant_cw_memless();

    CWout->dequant_cw_memless (A_POWER_IDX, A_AMP_IDX);

    for (i = 0; i <= CWout->lag >> 1; i++)
	CWout->b[i] = 0.0;
    CWout->pol2car ();

    for (i = 0, w = 1.0; i < ORDER; i++)
	tmplpc[i] = curr_lsp[i] * (w *= 0.8);
    CWout->poleFilter (tmplpc, ORDER);

    CWout->phaseShift (-TWO_PI * F_ROT_IDX / 128.0);


    //NEED TO TAKE OUT THIS PIECE OF INEFFICIENT CODE EVENTUALLY
    //IT IS TEMPORARILY SITTING HERE TO PRESERVE BIT EXACTNESS DURING PACKETIZATION
    for (i = 0; i < NUM_FIXED_BAND; i++) {
	float tmp;
	int j;

	if (i < 3) {
	    tmp = 32.0 / CWout->lag;
	    tmp *= CWout->lag / 2.0;
	    for (tmp = -tmp, j = 0; j < F_ALIGN_IDX[i]; j++, tmp += 1.0);
	    CWout->phaseShift_band (-TWO_PI * tmp / CWout->lag,
				    (float) F_BAND[i], (float) F_BAND[i + 1]);
	}
	else {
	    tmp = 32.0 / CWout->lag;
	    tmp *= CWout->lag / 2.0;
	    for (tmp = -tmp, j = 0; j < F_ALIGN_IDX[i]; j++, tmp += 0.5);
	    CWout->phaseShift_band (-TWO_PI * tmp / CWout->lag,
				    (float) F_BAND[i], (float) F_BAND[i + 1]);
	}
    }

    CWout->zeroFilter (tmplpc, ORDER);
}

float H_ROT = 0.0;
float H_ALIGN[7];
int H_BAND[7 + 1] = { 0, 440, 880, 1400, 2050, 2700, 3350, 4000 };


bool
FGV_MEM::ppp_quarter_encoder (DTFS * currCW_q, DTFS prevCW, DTFS currCW,
			      const float *curr_lsp, DTFS * targetCW)
{
    assert (pdelay == (float) prevCW.lag);	//Assertion may fail if delay_adjustment is on
    *currCW_q = currCW;

    //extern float Excitation2[ACBMemSize];
    DTFS TMP, TMP2;
    float tmp, temp_pl = prevCW.lag, temp_l = currCW_q->lag;
    float temp_acb[ACBMemSize + SubFrameSize + 10], temp_res[160], delayD3[3];
    int i, j, l = currCW_q->lag;
    short subframesize;
    bool returnFlag = TRUE;

    //Extending the ACB memory
    for (i = 0; i < ACBMemSize; i++)
	temp_acb[i] = Excitation2[i];
    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	// Interpolate delay
	Interpol_delay (delayD3, &temp_pl, &temp_l, i);
	// Compute adaptive codebook contribution
	acb_excitation (temp_acb + ACBMemSize, 1, delayD3, temp_acb,
			subframesize);

	for (j = 0; j < subframesize; j++)
	    temp_res[j + i * (SubFrameSize - 1)] = temp_acb[j + ACBMemSize];
	for (j = 0; j < ACBMemSize; j++)
	    temp_acb[j] = temp_acb[j + subframesize];
    }

    //Perform extraction on the ACB extended memory
    //Using temp_acb as a temporary storage for the extracted prototype
    ppp_extract_pitch_period (temp_res, temp_acb, l);
    TMP.to_fs (temp_acb, l);

    //Ensure the extracted prototype is time-synchronous to the
    //last l samples of the frame. 
    TMP2.to_fs (temp_res + FSIZE - l, l);
    tmp = TMP2.alignment_extract (TMP, 0.0, curr_lsp);
    TMP.phaseShift (TWO_PI * tmp / TMP.lag);

    //Amplitude Quantization
    currCW_q->car2pol ();
//  returnFlag = currCW_q->quant_cw(prevCW.lag, curr_lsp);
    returnFlag =
	currCW_q->quant_cw (prevCW.lag, curr_lsp, POWER_IDX, AMP_IDX,
			    lastLgainE, lastHgainE, lasterbE);

    *targetCW = *currCW_q;

    //Copying phase spectrum over
    TMP.car2pol ();
    for (i = 0; i <= currCW_q->lag >> 1; i++)
	currCW_q->b[i] = TMP.b[i];
    currCW_q->pol2car ();
    targetCW->pol2car ();

    tmp = currCW_q->alignment_fine (*targetCW, 0.0);
    targetCW->phaseShift (TWO_PI * tmp / targetCW->lag);

    return returnFlag;
}

void
FGV_MEM::ppp_quarter_decoder (DTFS * currCW_q, DTFS prevCW,
			      const float *curr_lsp)
{
    DTFS TMP, TMP2;
    float tmp, temp_pl = prevCW.lag, temp_l = currCW_q->lag;
    float temp_acb[ACBMemSize + SubFrameSize + 10], temp_res[160], delayD3[3];
    int i, j, l = currCW_q->lag;
    short subframesize;

    //Extending the ACB memory
    for (i = 0; i < ACBMemSize; i++)
	temp_acb[i] = PitchMemoryD[i];
    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	// Interpolate delay
	Interpol_delay (delayD3, &temp_pl, &temp_l, i);
	// Compute adaptive codebook contribution
	acb_excitation (temp_acb + ACBMemSize, 1, delayD3, temp_acb,
			subframesize);

	for (j = 0; j < subframesize; j++)
	    temp_res[j + i * (SubFrameSize - 1)] = temp_acb[j + ACBMemSize];
	for (j = 0; j < ACBMemSize; j++)
	    temp_acb[j] = temp_acb[j + subframesize];
    }

    //Perform extraction on the ACB extended memory
    //Using temp_acb as a temporary storage for the extracted prototype
    ppp_extract_pitch_period (temp_res, temp_acb, l);
    TMP.to_fs (temp_acb, l);

    //Ensure the extracted prototype is time-synchronous to the
    //last l samples of the frame. 
    TMP2.to_fs (temp_res + FSIZE - l, l);
    tmp = TMP2.alignment_extract (TMP, 0.0, curr_lsp);
    TMP.phaseShift (TWO_PI * tmp / l);

    //Amplitude Dequantization
    // currCW_q->dequant_cw(prevCW.lag);
    currCW_q->dequant_cw (prevCW.lag, POWER_IDX, AMP_IDX, lastLgainD,
			  lastHgainD, lasterbD);

    //Copying phase spectrum over
    TMP.car2pol ();
    for (i = 0; i <= currCW_q->lag >> 1; i++)
	currCW_q->b[i] = TMP.b[i];
    currCW_q->pol2car ();
}
