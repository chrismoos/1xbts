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

#include "macro.h"
#include "struct.h"
#include "rom.h"


void
FGV_MEM::UB_enc ()
{
    int k;

    int mshf = 0;


    /* Process lower band parameters for upper band analysis */

    if (celp_mdct_dec == 1 && data_packet.PACKET_RATE == 4)
	P_LB.Mode = 4;
    else
	P_LB.Mode = data_packet.PACKET_RATE;

    if (celp_mdct_dec == 0
	&& (data_packet.PACKET_RATE == 4 || data_packet.PACKET_RATE == 3))
	P_LB.PitchGain =
	    (ppvq[data_packet.ACBG_IDX[0]] + ppvq[data_packet.ACBG_IDX[1]] +
	     ppvq[data_packet.ACBG_IDX[2]]) / 3.0;
    else {
	if (celp_mdct_dec != 1) {
	    P_LB.PitchGain = 0;
	}

    }



    /* Analysis of the upper band */

    HB_INDICES enc_indices;
    QUANTIZED_HB Enc_Qp_HB;	// Structure to hold quantized  upper band parameters at the encoder
    UnQUANTIZED_HB_8thRate P_HB_8;
    int kk;

    float LSFdistance = 0;
    float tmpptr[160];


    for (k = 0; k < LOOKAHEAD_LEN * 7 / 8; k++)
	lookahead_UB[k] = buf_HB[7 * ibuf_len / 8 + hb_ana_delay + k];

    if (P_LB.Mode > 1) {

	mshf =
	    (int) ((7 * (frame_shift[0] + frame_shift[1] + frame_shift[2]) /
		    24) + 0.5);

    }
    else
	mshf = 0;



    declick_frame (buf_backup, buf_HB + hb_ana_delay, buf_HB + hb_ana_delay,
		   mshf, data_packet.PACKET_RATE);






    /* reset init if rate transition to/from 8th rate */
    if ((P_LB.Mode == 1 && lastrateE != 1)
	|| (P_LB.Mode != 1 && lastrateE == 1))
	initE = 0;



    if (P_LB.Mode > 1)		// do full/quarter rate UB processing
    {


	WB_EncParams (&P_LB, buf_HB, frame_shift, &P_HB, &LSFdistance, initE);
	/*Quantize upper band parameters */
	Quantize_WB_Params (&P_HB, &P_LB, LSFdistance, &Enc_Qp_HB,
			    &enc_indices, initE);

    }
    else			// 8th rate
    {
	/* 8th rate encoder */


    }

    initE = 1;


    if (P_LB.Mode == 1)		//pack parameters from full-band eight-rate
    {

    }
    else if (P_LB.Mode == 2)	//packing parameters after LB NELP frame
    {
	Bitpack (enc_indices.indices_LSFs[0], (unsigned short *) buf16, 8,
		 PktPtr);
	Bitpack (enc_indices.indices_LSFs[1], (unsigned short *) buf16, 4,
		 PktPtr);
	Bitpack (enc_indices.indices_gains[0], (unsigned short *) buf16, 4,
		 PktPtr);
	Bitpack (enc_indices.indices_gains[1], (unsigned short *) buf16, 4,
		 PktPtr);
	Bitpack (enc_indices.indices_gainFrame, (unsigned short *) buf16, 7,
		 PktPtr);
    }
    else if (P_LB.Mode == 4)	//packing paramters after LB CELP frame
    {
	Bitpack (enc_indices.indices_LSFs[0], (unsigned short *) buf16, 8,
		 PktPtr);

	Bitpack (enc_indices.indices_gains[0], (unsigned short *) buf16, 4,
		 PktPtr);
	Bitpack (enc_indices.indices_gainFrame, (unsigned short *) buf16, 4,
		 PktPtr);





	if (data_packet.Celp_Mdct_Flag == 0)	//CELP
	{
	    Bitpack (0, (unsigned short *) buf16, 1, PktPtr);	//170th bit is  music bit in WB mode
	    Bitpack (1, (unsigned short *) buf16, 1, PktPtr);	//171st  bit as the WB identifier
	}
	else if (data_packet.Celp_Mdct_Flag == 1)	//MDCT
	{
	    Bitpack (0, (unsigned short *) buf16, 1, PktPtr);	//dummy pack of 168th bit 
	    Bitpack (1, (unsigned short *) buf16, 1, PktPtr);	//packing 1 in the 169th bit to indicate WB MDCT frame
	    Bitpack (1, (unsigned short *) buf16, 1, PktPtr);	//170th bit is  music bit in WB mode
	    Bitpack (1, (unsigned short *) buf16, 1, PktPtr);	//171sh bit is EVRC-B/EVRC-WB mode bit
	}

    }


}
