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
/*  EVRC Decoder -- Eighth rate (rate = 1)                              */
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

/*======================================================================*/
/*         ..Decode bitstream data.                                     */
/*----------------------------------------------------------------------*/
void decode_rate_1(
			   INT16 *codeBuf,
			   INT16 post_filter,
			   INT16 *outFbuf
)
{
	/*....(local) variables.... */
	register INT16 i, j;
	register INT16 *foutP;
	INT32 delayi[3];
	INT16 subframesize;

		delayi[0] = L_deposit_h(DMIN);
		delayi[1] = delayi[0];
		delayi[2] = delayi[1];

#if ANSI_EVRC_LSP_EXPANSION
        if (!fer_flag)
#else
		if (fer_flag)
                { 
                    for (j = 0; j < ORDER; j++)
                        lsp[j] = OldlspD[j];
                }

		/* Bit-unpack the quantization indices */
		else
#endif
		{
                    for (i = 0; i < 2; i++)
			BitUnpack(&SScratch[i], (UINT16 *) PackedWords, lognsize8[i], PackWdsPtr);

                    lspmaq_dec(ORDER, 1, 2, nsub8, nsize8, lsp, SScratch, 1, lsptab8);

                    /* Check for monotonic LSPs */
                    for( j=1 ; j < ORDER ; j++ )
                    {
                        if( lsp[j] <= lsp[j-1] )
                            fer_flag = 1;
                    }

                    /* Check for minimum LSP separation at splits */
                    if( lsp[5] <= add(lsp[4],MIN_LSP_SEP) )
                        fer_flag = 1;

                    if( fer_flag == 1 )
                    {
                        for( j=0 ; j < ORDER ; j++ )
                            lsp[j] = OldlspD[j];
                    }

                    BitUnpack(&idxcbg, (UINT16 *) PackedWords, 8, PackWdsPtr);
		}

		foutP = outFbuf;

		for (i = 0; i < NoOfSubFrames; i++)
		{
			if (i < 2)
				subframesize = SubFrameSize - 1;
			else
				subframesize = SubFrameSize;

			if (lastrateD != 1 && i == 0 && decode_fcnt == 0)
				j = 0;			/* Reset seed */
			else
				j = 1;

#if ANSI_EVRC_ALL_ONES
            if (ones_dec_cnt>2)
            {
                idxcbg=64;
            }
#endif

			GetExc800bps_dec(PitchMemoryD + ACBMemSize, subframesize, idxcbg, j, i, fer_flag);

			for (j = 0; j < ACBMemSize; j++)
				PitchMemoryD[j] = PitchMemoryD[j + subframesize];

			/* Linear interpolation of lsp */
			Interpol(lspi, OldlspD, lsp, i, ORDER);

			/* Convert lsp to PC */
			lsp2a(pci, lspi);

			/* Synthesis of decoder output signal and postfilter output signal */
			iir(DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory, ORDER, subframesize);
			if (post_filter)
				apf(DECspeech, pci, DECspeechPF, L_shr(L_add(delayi[0], delayi[1]), 1), ALPHA, ALPHA, 0, AGC, 0, ORDER, subframesize, bit_rate);
			else
			{
				for (j = 0; j < subframesize; j++)
					DECspeechPF[j] = DECspeech[j];
			}

			/* Write decoder output and variables to files */
			for (j = 0; j < subframesize; j++){
				*foutP++ = DECspeechPF[j];
				}
		}

}

