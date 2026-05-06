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
/*  Memory Usage:                           				*/
/*      ROM:                            				*/
/*      Static/Global RAM:                      			*/
/*      Stack/Local RAM:                    				*/
/*----------------------------------------------------------------------*/

/*----------------------------------------------------------------------*/
/*  EVRC Decoder -- (called when no frame errors have occured.)         */
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
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "acelp_pf.h"          /* for ACELP */

extern void decode_rate_1(INT16*, INT16, INT16*);
void pit_shrp(INT16 *, INT16, INT16, INT16);
void Fix_delay_contour(INT32 *);


/*======================================================================*/
/*         ..Decode bitstream data.                                     */
/*----------------------------------------------------------------------*/
void decode_no_fer(
			   INT16 *codeBuf,
			   INT16 post_filter,
			   INT16 *outFbuf
)
{
	/*....(local) variables.... */
	register INT16 i, j, n;
	register INT16 *foutP;
	INT32 delayi[3];
	INT16 subframesize;
	INT16 sum1;
	INT16 sum_acb;

#define P75	24576
#define P20	6554

	/*....execute.... */
	for (i = 0; i < PACKWDSNUM; i++){
		PackedWords[i] = codeBuf[i];
		}

	/* Re-initialize PackWdsPtr */
	PackWdsPtr[0] = 16;
	PackWdsPtr[1] = 0;

	{
		fer_counter -= 1;
		if (fer_counter < 0)
			fer_counter = 0;
	}


	if (bit_rate == 1)
	{
		decode_rate_1(codeBuf, post_filter, outFbuf);
	}

	else
	{							/* If rate is 4kbps or 8kbps */

		if (bit_rate == 4)
			BitUnpack(&LPCflag, (UINT16 *) PackedWords, 1, PackWdsPtr);
		else
			LPCflag = 0;

		/* Bit-unpack the quantization indices */
		for (i = 0; i < knum; i++)
			BitUnpack(&SScratch[i], (UINT16 *) PackedWords,
					  lognsize[i], PackWdsPtr);

		{
			lspmaq_dec(ORDER, 1, knum, nsub, nsize, lsp, SScratch, bit_rate, lsptab);

                        /* Check for monotonic LSP */
			for (j = 1; j < ORDER; j++)
                            if (lsp[j] <= lsp[j - 1]) 
                            {
                                errorFlag = 1;
                                return;
                            }

                        /* Check for minimum separation of LSPs at the splits */
                        if( bit_rate == 3 )     /* Check Half Rate splits */
                        {
                            if(   (lsp[3] <= add(lsp[2],MIN_LSP_SEP))
                               || (lsp[6] <= add(lsp[5],MIN_LSP_SEP)))
                            {
                                errorFlag = 1;
                                return;
                            }
                        }
                        else                    
                        {                      /* Check Full Rate splits */
                            if(   (lsp[2] <= add(lsp[1],MIN_LSP_SEP))
                               || (lsp[4] <= add(lsp[3],MIN_LSP_SEP))
                               || (lsp[7] <= add(lsp[6],MIN_LSP_SEP)))
                            {
                                errorFlag = 1;
                                return;
                            }
                        }
		}


		BitUnpack(&idxppg, (UINT16 *) PackedWords, 7, PackWdsPtr);
		idxppg += DMIN;

		/* Check in delay is possible */
		if (idxppg > DMAX)
		{
			errorFlag = 1;
			return;
		}

		delay = idxppg;

        if (bit_rate == 4) {  /* check for full-rate */

            /* unpack delta_delay */
            BitUnpack(&idxppg, (UINT16 *) PackedWords, 5, PackWdsPtr);

#if ANSI_EVRC_DDELAY_CHECK
            /* check for out-of-range previous delay before Fix_delay_contour */
            if (idxppg != 0)
            {
                /* prev_delay = delay - delta_delta + 16 */
                n = sub(delay, sub(idxppg, 16));

                if (n < DMIN)
                {
                    errorFlag = 1;
                    return;
                }
                if (n > DMAX)
                {
                    errorFlag = 1;
                    return;
                }
            }
#endif

            /* Fix delay contour */
            if (fer_counter == 2 && idxppg != 0)
            {
                Fix_delay_contour(delayi);
            }
        }

		/* Smooth interpolation if the difference between delays is too big */
		if (abs(sub(delay, pdelayD)) > 15)
			pdelayD = delay;

		/* Update fer coefficients */
		ave_acb_gain = ave_fcb_gain = 0;

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

			/* Bandwidth expansion after frame erasure only if LPCflag is set */
			if (bit_rate == 4)
			{
				if (LPCflag && fer_counter == 2)
					weight(pci, pci, P75, ORDER);
			}

			Interpol_delay(delayi, &pdelayD, &delay, i);


			/* Un-Pack bits */
			/* ACB delay gain */
			BitUnpack(&idxppg, (UINT16 *) PackedWords, 3, PackWdsPtr);

			/* FCB shape index 1 */
			if (bit_rate == 4)
			{
				BitUnpack(SScratch, (UINT16 *) PackedWords, 8, PackWdsPtr);
				fcbIndexVector[0] = *SScratch;
				BitUnpack(SScratch, (UINT16 *) PackedWords, 8, PackWdsPtr);
				fcbIndexVector[1] = *SScratch;
				BitUnpack(SScratch, (UINT16 *) PackedWords, 8, PackWdsPtr);
				fcbIndexVector[2] = *SScratch;
				BitUnpack(SScratch, (UINT16 *) PackedWords, 11, PackWdsPtr);
				fcbIndexVector[3] = *SScratch;
				/* FCB gain index */
				BitUnpack(&idxcbg, (UINT16 *) PackedWords, 5, PackWdsPtr);
			}
			else
			{
				BitUnpack(&idxcb, (UINT16 *) PackedWords, 10, PackWdsPtr);
				/* FCB gain index */
				BitUnpack(&idxcbg, (UINT16 *) PackedWords, 4, PackWdsPtr);
			}

			/* Compute adaptive codebook contribution */
			{
				sum_acb = ppvq[idxppg];
				ave_acb_gain = add(ave_acb_gain, mult_r(sum_acb, 10923));	/* (10923=1/NoOfSubFrames) */
			}

			acb_excitation(PitchMemoryD + ACBMemSize, sum_acb, delayi,
						   PitchMemoryD, subframesize);

			/* Compute fixed codebook contribution */
			ave_fcb_gain = add(ave_fcb_gain, mult_r(gnvq[idxcbg], 1365));	/* (1365=(1/3)/8) */

			/* Compute fixed codebook contribution */
			if ((bit_rate == 4))	/* full rate */
                dec8_35(fcbIndexVector, Scratch, subframesize);

			if ((bit_rate == 3))	/* half rate */
                dec3_10(idxcb, Scratch, subframesize);

			{
				if (sum_acb > 14744)
					sum_acb = 14744;
				if (sum_acb < 3276)
					sum_acb = 3276;
				sum_acb = shl(sum_acb, 1);


				/* get intrpolated delay for this subframe */
				/* n = extract_h(L_add(L_shr(L_add(delayi[1], delayi[0]), 1), 32768)); */
				/* n = extract_h(L_add(L_shr(L_add(delayi[1], delayi[0]), 1), 0x00008000L)); */
				n = round32(L_shr(L_add(delayi[1], delayi[0]), 1));
				if (n > subframesize)
					n = 200;

				/* To scale down by 8 to offset (sum1=gnvq[]),
				 * which has been scaled up by 8 */
				for (j = 0; j < SubFrameSize + 6; j++)
					Scratch[j] = shl(Scratch[j], 11);

				pit_shrp(Scratch, n, sum_acb, subframesize);

				sum1 = gnvq[idxcbg];
				for (j = 0; j < subframesize; j++)
					PitchMemoryD[j + ACBMemSize] = add(PitchMemoryD[j + ACBMemSize], mult_r(sum1, Scratch[j]));
			}

			{
				FadeScale = add(FadeScale, P20);
			}

			for (j = 0; j < ACBMemSize; j++)
				PitchMemoryD[j] = PitchMemoryD[j + subframesize];


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
			for (j = 0; j < subframesize; j++){
				*foutP++ = shl(DECspeechPF[j],1); /* adjust scaling */
				}
		}
		pdelayD = delay;
	}							/* Ends case for rate=4kbps or 8kbps */

	/* update decoder varaibles */
	for (i = 0; i < ORDER; i++)
		OldlspD[i] = lsp[i];
	lastrateD = bit_rate;

	decode_fcnt++;
	last_fer_flag = fer_flag;
}

void Fix_delay_contour(INT32 *delayi)
{
register int i, j;
INT16 subframesize;

		/* Fix delay countour of previous erased frame */
			j = sub(idxppg, 16);
			pdelayD_back = pdelayD;
			pdelayD = sub(delay, j);
			if (abs(sub(pdelayD, pdelayD_back)) > 15)
				pdelayD_back = pdelayD;

			for (i = 0; i < ACBMemSize; i++)
				PitchMemoryD[i] = PitchMemoryD_back[i];

#if 1
			for (i = 0; i < NoOfSubFrames; i++)
			{
				if (i < 2)
					subframesize = SubFrameSize - 1;
				else
					subframesize = SubFrameSize;

				/* Interpolate delay */
				Interpol_delay(delayi, &pdelayD_back, &pdelayD, i);

				/* Compute adaptive codebook contribution */
				acb_excitation(PitchMemoryD + ACBMemSize, ave_acb_gain, delayi,
							   PitchMemoryD, subframesize);

				for (j = 0; j < ACBMemSize; j++)
					PitchMemoryD[j] = PitchMemoryD[j + subframesize];
			}
#endif
}

