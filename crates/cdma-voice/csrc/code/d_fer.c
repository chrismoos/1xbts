/**********************************************************************
Each of the companies; Lucent, Motorola, Nokia, and Qualcomm (hereinafter 
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
all Normative portions of the Enhanced Variable Rate Codec (EVRC) as 
it exists on the date of submittal of this form, should the EVRC be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC or the field of use of practice of the 
EVRC Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
 
/*======================================================================*/
/*     Enhanced Variable Rate Codec - Bit-Exact C Specification         */
/*     Copyright (C) 1997-1998 Telecommunications Industry Association. */
/*     All rights reserved.                                             */
/*----------------------------------------------------------------------*/
/* Note:  Reproduction and use of this software for the design and      */
/*     development of North American Wideband CDMA Digital              */
/*     Cellular Telephony Standards is authorized by the TIA.           */
/*     The TIA does not authorize the use of this software for any      */
/*     other purpose.                                                   */
/*                                                                      */
/*     The availability of this software does not provide any license   */
/*     by implication, estoppel, or otherwise under any patent rights   */
/*     of TIA member companies or others covering any use of the        */
/*     contents herein.                                                 */
/*                                                                      */
/*     Any copies of this software or derivative works must include     */
/*     this and all other proprietary notices.                          */
/*======================================================================*/
/*  EVRC Decoder -- (called when frame errors have occured.)            */
/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/

#include  <stdio.h>
#include  <string.h>
#include  <stdlib.h>

#include "mathadv.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"

#include  "d_globs.h"
#include  "fer.h"
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"

extern void decode_rate_1(INT16*, INT16, INT16*);

#if ANSI_EVRC_LSP_EXPANSION
INT16 spread_lsp[ORDER] = { /* same as initial LSP values */
    1573,
    3146,
    4719,
    6291,
    7864,
    9437,
    11010,
    12583,
    14156,
    15729
};
#endif

/*======================================================================*/
/*         ..Decode bitstream data.                                     */
/*----------------------------------------------------------------------*/
void decode_fer(
			   INT16 post_filter,
			   INT16 *outFbuf
)
{
	/*....(static) variables.... */
#if ANSI_EVRC_SEED
#define Seed (evrc_current_context()->decoder.d_fer_Seed)
#else
    INT16 Seed;
#endif

	/*....(local) variables.... */
	register INT16 i, j;
	register INT16 *foutP;
	INT32 delayi[3];
	INT16 subframesize;
	INT16 sum_acb;

#define P75	24576
#define P15	4915
#define P20	6554
#define P05	1638
#define P10	3277

	/*....execute.... */

	if (fer_counter == 3)
		ave_acb_gain = mult_r(ave_acb_gain, P75);
#if ANSI_EVRC_SEED
        else
        {
                Seed = shl(OldlspD[ORDER - 1], 1);      /* A random number */
        }
#endif
       
	fer_counter = 3;

#if ANSI_EVRC_LSP_EXPANSION
        /* Frame Erasure - Use old LSPs with bandwidth expansion */
        /* lsp[j] = (OldlspD[j]-(0.125*OldlspD[j])) + 0.125*mean_lsf[j]; */
        /*        = (0.875*OldlspD[j] + 0.125*mean_lsf[j]); */

#define SPREAD 0x1000 /* 0.125 */
        {
            INT32 L_temp;

            /* Frame Erasure - Use old LSPs with bandwidth expansion */
            for (j = 0; j < ORDER; j++)
            {
                L_temp = L_deposit_h(OldlspD[j]);
                L_temp = L_msu(L_temp, OldlspD[j], SPREAD);
                L_temp = L_mac(L_temp, spread_lsp[j], SPREAD);
                lsp[j] = round32(L_temp);
            }
        }
#endif

	if (bit_rate == 1)
	{
		decode_rate_1((INT16 *) NULL, post_filter, outFbuf);
	}

	else
	{							/* If rate is 4kbps or 8kbps */
	        if (bit_rate == 4)
                {
                        for (i = 0; i < ACBMemSize; i++)
                                PitchMemoryD_back[i] = PitchMemoryD[i];
                }


#if !ANSI_EVRC_LSP_EXPANSION
		/* Frame Erasure - Use old LSPs */
		for (j = 0; j < ORDER; j++)
			lsp[j] = OldlspD[j];
#endif

		delay = pdelayD;

		foutP = outFbuf;
		for (i = 0; i < NoOfSubFrames; i++)
		{
			if (i < 2)
				subframesize = SubFrameSize - 1;
			else
				subframesize = SubFrameSize;

			Interpol(lspi, OldlspD, lsp, i, ORDER);

			/* Convert lsp to PC */
			lsp2a(pci, lspi);

			Interpol_delay(delayi, &pdelayD, &delay, i);

			if (ave_acb_gain < P15)		/* P15=(0.3/2) */
			{
				delayi[0] = L_deposit_h(rnd_delay[i]);
				delayi[1] = L_deposit_h(rnd_delay[i + 1]);
				delayi[2] = L_deposit_h(rnd_delay[i + 2]);
			}


			/* Compute adaptive codebook contribution */
			sum_acb = ave_acb_gain;

			acb_excitation(PitchMemoryD + ACBMemSize, sum_acb, delayi,
						   PitchMemoryD, subframesize);


			/* Add fading */
			{
				for (j = 0; j < subframesize; j++)
					PitchMemoryD[ACBMemSize + j] = mult_r(PitchMemoryD[ACBMemSize + j], FadeScale);
				FadeScale = sub(FadeScale, P05);
				if (FadeScale < 0)
					FadeScale = 0;
			}


			for (j = 0; j < ACBMemSize; j++)
				PitchMemoryD[j] = PitchMemoryD[j + subframesize];

			{
				/* Use gaussian noise */
				if (ave_acb_gain < P20)		/* P20=(0.4/2) */
				{
#if !ANSI_EVRC_SEED
                    Seed = shl(lsp[ORDER - 1], 1);  /* A random number */
#endif
					for (j = 0; j < subframesize; j++)
						PitchMemoryD[ACBMemSize + j] = add(PitchMemoryD[ACBMemSize + j], round32(L_mult(ran_g(&Seed), mult_r(P10, ave_fcb_gain))));

				}
			}

			/* Synthesis of decoder output signal and postfilter output signal */
			iir(DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory, ORDER, subframesize);

			/* Postfilter */
			if (post_filter)
			{
				if (bit_rate == 4)
					apf(DECspeech, pci, DECspeechPF, L_shr(L_add(delayi[0], delayi[1]), 1), ALPHA, BETA, U, AGC, LTGAIN, ORDER, subframesize, bit_rate);
				else
					apf(DECspeech, pci, DECspeechPF, L_shr(L_add(delayi[0], delayi[1]), 1), HALF_ALPHA, BETA, HALF_U, AGC, LTGAIN, ORDER, subframesize, bit_rate);
			}
			else
			{
				for (j = 0; j < subframesize; j++)
					DECspeechPF[j] = DECspeech[j];
			}

			/* Write p.f. decoder output and variables to files */
			for (j = 0; j < subframesize; j++)
				*foutP++ = shl(DECspeechPF[j],1); /* adjust scaling */
		}
		pdelayD = delay;
	}							/* Ends case for rate=4kbps or 8kbps */

	/* update decoder varaibles */
	for (i = 0; i < ORDER; i++)
		OldlspD[i] = lsp[i];
	lastrateD = bit_rate;

	decode_fcnt++;
	last_fer_flag = fer_flag;
#if ANSI_EVRC_SEED
#undef Seed
#endif
}
