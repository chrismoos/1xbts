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


/* for ER upper band extension */


#include "struct.h"

#include "defines.h"

#include "proto.h"
#include "proto_new.h"

#define M 5

#define SF 54
void
FGV_MEM::UB_DTX (struct QUANTIZED_HB *p)
{
    int i, j;
    float H[SF];
    float tmplsf[LPC_ORD_WB], lpc_hb[LPC_ORD_WB], lpc_gain;
    static long seed_ub = 0;
    static short ft = 0;
    static int EC = 0;
    static float prev_prev_NELP_LSFs[LPC_ORD_WB];
    static float prev_NELP_LSFs[LPC_ORD_WB];
    static float smooth_gf = 0.0;
    static float ScaleFac[5] = { 1.0, 1.0, 1.0, 1.0, 1.0 };
    static float frac[32] =
	{ 1.0 / 32.0, 2.0 / 32.0, 3.0 / 32.0, 4.0 / 32.0, 5.0 / 32.0,
6.0 / 32.0, 7.0 / 32.0, 8.0 / 32.0, 9.0 / 32.0, 10.0 / 32.0, 11.0 / 32.0, 12.0 / 32.0,
13.0 / 32.0, 14.0 / 32.0, 15.0 / 32.0, 0.5, 17.0 / 32.0, 18.0 / 32.0, 19.0 / 32.0,
20.0 / 32.0, 21.0 / 32.0, 22.0 / 32.0, 23.0 / 32.0, 24.0 / 32.0, 25.0 / 32.0,
26.0 / 32.0, 27.0 / 32.0, 28.0 / 32.0, 29.0 / 32.0, 30.0 / 32.0, 31.0 / 32.0, 1.0 };

    float attn_fac = 0.6;

    if ((ft == 0) && (data_packet.PACKET_RATE == 1)) {
	smooth_gf = attn_fac * prev_GainFrame;
	for (i = 0; i < LPC_ORD_WB; i++)
	    prev_prev_NELP_LSFs[i] = prev_NELP_LSFs[i] = prev_LSFs[i];
	ft = 1;
    }

    if ((data_packet.PACKET_RATE == 1) && (lastrateD == 2)) {
	smooth_gf = 0.1 * (attn_fac * prev_GainFrame) + 0.9 * smooth_gf;

	for (i = 0; i < LPC_ORD_WB; i++)
	    tmplsf[i] = prev_LSFs[i] / (2.0 * Pi);
	lsp2a (lpc_hb, tmplsf, LPC_ORD_WB);

	ImpulseRzp1 (H, lpc_hb, lpc_hb, 1.0, 1.0, LPC_ORD_WB, SF);

	/* Get energy of H */
	for (i = 0, lpc_gain = 0; i < SF; i++)
	    lpc_gain += H[i] * H[i];
	lpc_gain = 10 * log10 (lpc_gain);




	//if (lpc_gain < 1.0)
	{
	    for (i = 0; i < LPC_ORD_WB; i++) {
		prev_prev_NELP_LSFs[i] = prev_NELP_LSFs[i];
		prev_NELP_LSFs[i] = prev_LSFs[i];
	    }
	}

    }

    if (data_packet.PACKET_RATE == 1) {

	EC++;
	if (EC > 32)
	    EC = 32;


	for (i = 0; i < M; i++)
	    p->Gain[i] = 1.0;

	p->GainFrame = smooth_gf + 0.01 * ran_g (&seed_ub);



	for (i = 0; i < LPC_ORD_WB; i++)
	    p->LSFs[i] =
		(prev_prev_NELP_LSFs[i] * (1.0 - frac[EC - 1])) +
		(prev_NELP_LSFs[i] * frac[EC - 1]);

    }
    else
	EC = 0;

}
