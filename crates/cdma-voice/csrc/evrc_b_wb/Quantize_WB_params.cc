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
#include <math.h>
#include "defines.h"
#include "struct.h"
#include "MSencode.h"
#include "Qflags.h"
#include "UB_lsf_cb.h"
#include "UB_gain_cb.h"
#include "codebooks_8R.h"

#define SPC  0.01114084601643

void
space_lsfs (float *lsfs, int order)
{

    float delta;
    int i, j, flag = 1;

    while (flag == 1) {
	flag = 0;
	for (i = 0; i <= order; i++) {
	    delta =
		i == 0 ? lsfs[0] : (i ==
				    order ? 0.5 - lsfs[i - 1] : (lsfs[i] -
								 lsfs[i -
								      1]));
	    if (delta < SPC) {
		flag = 1;
		delta = delta - SPC - SPC / 1000;
		if (i == order)
		    lsfs[i - 1] = lsfs[i - 1] + delta;
		else {
		    if (i == 0)
			lsfs[i] = lsfs[i] - delta;
		    else {
			lsfs[i - 1] = lsfs[i - 1] + delta / 2;
			lsfs[i] = lsfs[i] - delta / 2;
		    }
		}
	    }
	}
    }
}

void
determine_gain_weights (float *gain, float *weights, int dims)
{
    int j;
    float absgain;

    for (j = 0; j < dims; j++) {

	if (gain[j] > 1e-6)
	    absgain = fabs (gain[j]);
	else
	    absgain = 0.001;

	absgain = (pow (double (absgain), 0.9));

	weights[j] = 1 / absgain;
    }
}





void
FGV_MEM::Quantize_WB_Params (struct UnQUANTIZED_HB *p,
			     struct PARAM_FRAME_4GVLB *p_LB, float LSFDist,
			     struct QUANTIZED_HB *Qp,
			     struct HB_INDICES *indices, int FirstTime)
{

    float mod_lsfs[6];
    int indexLSF_stage1, indexLSF_stage2, indexGainFrame, indexGain,
	indexgain_stage1, indexgain_stage2;

    int QGF_dB;
    float GF_dB, RGF_dB;
    int i, j;
    int transition = 0;
    if ((p_LB->Mode == 4 || p_LB->Mode == 3) && quantize_wb_prev_mode == 2)
	transition = 1;
    quantize_wb_prev_mode = p_LB->Mode;


    if (BPF_FR > 0) {
	for (i = 0; i < LPC_ORD_WB1; i++) {
	    if (FirstTime == 0) {
		d_lsfs[i] = 0;
	    }
	    else {
		d_lsfs[i] = d_lsfs[i] / (1.5 + (0.1 * LSFDist));
	    }
	    mod_lsfs[i] = p->LSFs[i] + d_lsfs[i];
	}

	if (p_LB->Mode == 4 || p_LB->Mode == 64 || p_LB->Mode == 3) {



	    singlevectortest (&(mod_lsfs[0]), LPC_ORD_WB1, 256,
			      &(indices->indices_LSFs[0]), p->LSFWeights,
			      Qp->LSFs, &(CBLSF[0]));


	    determine_gain_weights (p->Gain, &(Unit_weights5[0]), 5);
	    determine_gain_weights (&(p->GainFrame), &(Unit_weights1), 1);


	    singlevectortest (p->Gain, 5, 16, &(indices->indices_gains[0]),
			      &(Unit_weights5[0]), Qp->Gain,
			      newGainCB_rs1_16);
	    singlevectortest_gain (&(p->GainFrame), 1, 16,
				   &(indices->indices_gainFrame),
				   &(Unit_weights1), &(Qp->GainFrame),
				   newGainFrameCB_rs1_16);









	}
	else {
	    if (p_LB->Mode == 2) {

		MSencode (&(mod_lsfs[0]), p->LSFWeights,
			  &(indices->indices_LSFs[0]),
			  &(indices->indices_LSFs[1]), Qp->LSFs, LPC_ORD_WB1,
			  256, 16, 8, &(CBLSF[0]), &(CBLSF2_QR[0]));

		determine_gain_weights (p->Gain, &(Unit_weights5[0]), 5);
		MSencode (p->Gain, &(Unit_weights5[0]),
			  &(indices->indices_gains[0]),
			  &(indices->indices_gains[1]), Qp->Gain, 5, 16, 16,
			  8, newGainCB_rs12_QR_cb1, newGainCB_rs12_QR_cb2);


		GF_dB = 20 * log10 (p->GainFrame);

		QGF_dB = (int) (((GF_dB + 32) / 0.6) + 0.5);

		if (QGF_dB < 0)
		    QGF_dB = 0;
		if (QGF_dB > 127)
		    QGF_dB = 127;

		indices->indices_gainFrame = QGF_dB;

		RGF_dB = ((float) QGF_dB * 0.6) - 32;
		Qp->GainFrame = pow (10.0, (RGF_dB / 20.0));



	    }
	    else {
		for (i = 0; i < LPC_ORD_WB1; i++) {
		    Qp->LSFs[i] = p->LSFs[i];
		}
	    }
	}
	space_lsfs (Qp->LSFs, LPC_ORD_WB1);





	for (i = 0; i < LPC_ORD_WB1; i++) {
	    d_lsfs[i] = Qp->LSFs[i] - p->LSFs[i];
	}
    }

}


#define MIN_LOG_ENERGY_FB_ER 0
#define MAX_LOG_ENERGY_FB_ER 3.44
void
FGV_MEM::ER_Gain_Q (float Un_gain, short *index)
{
    int i;
    float sum, D, tmp;

    Un_gain = log10 (Un_gain);
    if (data_packet.WB_MODE_BIT == 1) {

	for (i = 0, sum = 1e30; i < NUM_Q_LEVELS; i++) {
	    D = Un_gain - (MIN_LOG_ENERGY_FB_ER +
			   (float) i * (MAX_LOG_ENERGY_FB_ER -
					MIN_LOG_ENERGY_FB_ER) / NUM_Q_LEVELS);
	    tmp = D * D;
	    if (tmp < sum) {
		sum = tmp;
		*index = i;
	    }
	}
    }
    else {
	for (i = 0, sum = 1e30; i < NUM_Q_LEVELS_NB; i++) {
	    D = Un_gain - (MIN_LOG_ENERGY_FB_ER +
			   (float) i * (MAX_LOG_ENERGY_FB_ER -
					MIN_LOG_ENERGY_FB_ER) /
			   NUM_Q_LEVELS_NB);
	    tmp = D * D;
	    if (tmp < sum) {
		sum = tmp;
		*index = i;
	    }
	}

    }

}

void
FGV_MEM::Quantize_WB_params_8R (struct UnQUANTIZED_HB_8thRate *P_HB_8R,
				struct HB_INDICES *indices)
{

    float wt, reconst_lsfs[10], reconst_gain;



    MSencode (P_HB_8R->LSFs_8, P_HB_8R->LSFWeights_8,
	      &(indices->indices_LSFs[0]), &(indices->indices_LSFs[1]),
	      reconst_lsfs, 10, 32, 16, 4, &(msvq_lsfCB1_8R[0]),
	      &(msvq_lsfCB2_8R[0]));


    /* reduce all 8th rate levels by 3 dB */
    P_HB_8R->Gain *= 0.7;

    ER_Gain_Q (P_HB_8R->Gain, &(indices->indices_gains[0]));

}

void
FGV_MEM::dequantize_WBparams (struct QUANTIZED_HB *DecQp,
			      struct HB_INDICES indices, short mode)
{

    float mod_lsfs[6];
    int indexLSF_stage1, indexLSF_stage2, indexGainFrame, indexGain,
	indexgain_stage1, indexgain_stage2;

    int QGF_dB;
    float GF_dB, RGF_dB;
    int i, j;



    if (mode == 4 || mode == 64) {



	SingleStageDecode (indices.indices_LSFs[0], LPC_ORD_WB1, DecQp->LSFs,
			   &(CBLSF[0]));

	SingleStageDecode (indices.indices_gainFrame, 1, &(DecQp->GainFrame),
			   newGainFrameCB_rs1_16);
	SingleStageDecode (indices.indices_gains[0], 5, DecQp->Gain,
			   newGainCB_rs1_16);











    }
    else {
	if (mode == 2) {

	    MSdecode (indices.indices_LSFs[0], indices.indices_LSFs[1],
		      DecQp->LSFs, LPC_ORD_WB1, &(CBLSF[0]), &(CBLSF2_QR[0]));
	    MSdecode (indices.indices_gains[0], indices.indices_gains[1],
		      DecQp->Gain, 5, newGainCB_rs12_QR_cb1,
		      newGainCB_rs12_QR_cb2);


	    QGF_dB = indices.indices_gainFrame;

	    RGF_dB = ((float) QGF_dB * 0.6) - 32;
	    DecQp->GainFrame = pow (10.0, (RGF_dB / 20.0));



	}
    }
    space_lsfs (DecQp->LSFs, LPC_ORD_WB1);


}

void
FGV_MEM::dequantize_WBparams_8R (struct UnQUANTIZED_HB_8thRate *DecQp_HB,
				 struct HB_INDICES indices)
{

    MSdecode (indices.indices_LSFs[0], indices.indices_LSFs[1],
	      DecQp_HB->LSFs_8, 10, &(msvq_lsfCB1_8R[0]),
	      &(msvq_lsfCB2_8R[0]));
    DecQp_HB->Gain =
	MIN_LOG_ENERGY_FB_ER +
	(float) (indices.indices_gains[0]) * (MAX_LOG_ENERGY_FB_ER -
					      MIN_LOG_ENERGY_FB_ER) /
	NUM_Q_LEVELS;
    DecQp_HB->Gain = pow (10.0, double (DecQp_HB->Gain));

}
