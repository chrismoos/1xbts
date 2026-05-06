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
/* EVRC Encoder */
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

#include  "e_globs.h"
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "acelp_pf.h"          /* for ACELP fixed codebook */
//#include  "tty.h"

/*======================================================================*/
/*         ..Reset RCELP encode rate.                                   */
/*----------------------------------------------------------------------*/
void
    SetEncodeRate(INT16 rate)
{

	/*....execute.... */
	bit_rate = rate;

	switch (bit_rate)
	{
	case 1:
		FCBGainSize = 16;		/*...use half-rate... */
		gnvq = gnvq_4;
		break;
	case 3:
		nsub = nsub22;
		nsize = nsize22;
		lognsize = lognsize22;
		lsptab = lsptab22;
		knum = 3;
		FCBGainSize = 16;
		gnvq = gnvq_4;
		break;
	case 4:
		nsub = nsub28;
		nsize = nsize28;
		lognsize = lognsize28;
		lsptab = lsptab28;
		knum = 4;
		FCBGainSize = 32;
		gnvq = gnvq_8;
		break;
	}
}

/*======================================================================*/
/*         ..Reset Encoder parameters.                                  */
/*----------------------------------------------------------------------*/
void InitEncoder()
{
	/*....(local) variables.... */
	int j;
	/****************************************************/
	/*         Algorithm (one time) initializations     */
	/****************************************************/

	SetEncodeRate(4);
	encode_fcnt = 0;

	for (j = 0; j < ORDER; j++)
		SynMemoryM[j] = 0;

	OldlspE[0] = 1573;
	OldlspE[1] = 3146;
	OldlspE[2] = 4719;
	OldlspE[3] = 6291;
	OldlspE[4] = 7864;
	OldlspE[5] = 9437;
	OldlspE[6] = 11010;
	OldlspE[7] = 12583;
	OldlspE[8] = 14156;
	OldlspE[9] = 15729;

	Oldlsp_nq[0] = 1573;
	Oldlsp_nq[1] = 3146;
	Oldlsp_nq[2] = 4719;
	Oldlsp_nq[3] = 6291;
	Oldlsp_nq[4] = 7864;
	Oldlsp_nq[5] = 9437;
	Oldlsp_nq[6] = 11010;
	Oldlsp_nq[7] = 12583;
	Oldlsp_nq[8] = 14156;
	Oldlsp_nq[9] = 15729;

	for (j = 0; j < 10; j++)
		residual[j] = 0;

	for (j = 0; j < GUARD*2; j++)
		ConstHPspeech[j] = 0;

	for (j = 0; j < ORDER; j++)
		WFmemIIR[j] = WFmemFIR[j] = 0;

	for (j = 0; j < ACBMemSize; j++)
		Excitation[j] = 0;

	accshift = 0;
	shiftSTATE = 0;
	dpm = 0;
	pdelay = 40;

    LPCgain = 0x0008;

}

/*======================================================================*/
/*         ..Encode speech data.                                        */
/*----------------------------------------------------------------------*/
void encode(
			   INT16 rate,
			   INT16 *codeBuf
)
{

	/*....(local) variables.... */
	register INT16 i, j, n;
	INT32 delayi[3];
	INT16 subframesize;
	INT16 Aveidxppg;
	INT16 sum1;
	INT32 lsum1;
	INT16 shft_fctr;
	INT32 l_fcbGain;

	/* Re-initialize PackWdsPtr */
	PackWdsPtr[0] = 16;
	PackWdsPtr[1] = 0;
	for (i = 0; i < PACKWDSNUM; i++)
		PackedWords[i] = 0;

	/*....execute.... */
	SetEncodeRate(rate);

	/* Handle 800bps seperatly */
	if (bit_rate == 1)
	{
		/* Reset accumulated shift */
		accshift = 0;
		dpm = 0;

		lspmaq(lsp_nq, ORDER, 1, 2, nsub8, nsize8, 16384, lsp, SScratch, bit_rate, lsptab8);

		/* Bit-pack the quantization indices */
		for (i = 0; i < 2; i++)
			Bitpack(SScratch[i], (UINT16 *) PackedWords, lognsize8[i], PackWdsPtr);

		for (i = 0; i < NoOfSubFrames; i++)
		{
			if (i < 2)
				subframesize = SubFrameSize - 1;
			else
				subframesize = SubFrameSize;

			/* interpolate lsp */
			Interpol(lspi, OldlspE, lsp, i, ORDER);

			/* Convert lsp to PC */
			lsp2a(pci, lspi);

			/* Get lpc gain */
			/* Calculate impulse response of 1/A(z) */
			ImpulseRzp(H, pci, pci, 0x7fff, 0x7fff, ORDER, Hlength);
			/* Get energy of H */
			lsum1 = 0;

			for (j = 0; j < subframesize; j++)
				lsum1 = L_mac(lsum1, H[j], H[j]);

			shft_fctr = norm_l(lsum1);
			lsum1 = L_shl(lsum1, shft_fctr);
			sum1 = sqroot(lsum1);

			if (shft_fctr & 0x0001)
				sum1 = mult(sum1, 23170);

			shft_fctr = shr(shft_fctr, 1);
			sum1 = shr(sum1, shft_fctr - 1);

			if (lastrateE != 1 && i == 0 && encode_fcnt == 0)
				j = 0;			/* Reset seed */
			else
				j = 1;
			GetExc800bps(Excitation, &idxcbg, sum1, residual + GUARD + i * (SubFrameSize - 1), subframesize, j, i);

			/*...another puff fix... */
			ZeroInput(zir, pci, pci, Excitation + ACBMemSize - subframesize, GAMMA1, GAMMA2, ORDER, subframesize, 1);
		}

        /* Trap for all ones output packet. */
        /* If rate 1/8 packet is all ones, flip msb of gain code */
        /* (encoder shall never generate an all ones rate 1/8 packet..*/
        if ((SScratch[0]&SScratch[1]&0xf)==0xf && (idxcbg==0xff))
		{
			/* Clear Frame Energy Gain MSB if output packet == all ones */
			idxcbg=0x7f;  
        }
        Bitpack(idxcbg, (UINT16 *) PackedWords, 8, PackWdsPtr);

	}
	else
	{							/* bit rates 4 or 8kbps */
		/* Send indication on the spectrum transition */
		if (bit_rate == 4)
			Bitpack(LPCflag, (UINT16 *) PackedWords, 1, PackWdsPtr);

		/* Quantize the LSPs */
		lspmaq(lsp_nq, ORDER, 1, knum, nsub, nsize, 16384, lsp, SScratch, bit_rate, lsptab);

		/* Bit-pack the quantization indices */

		for (i = 0; i < knum; i++)
			Bitpack(SScratch[i], (UINT16 *) PackedWords,
					lognsize[i], PackWdsPtr);

		/* Update shiftSTATE with hysteresis */
		if (beta < 3277)
		{
			accshift = 0;
			dpm = 0;
			shiftSTATE = 0;
		}

		if (accshift > 5120)
			shiftSTATE = -1;
		if (accshift < -5120)
			shiftSTATE = 1;
		if (accshift <= 2560 && shiftSTATE == -1)
			shiftSTATE = 0;
		if (accshift >= -2560 && shiftSTATE == 1)
			shiftSTATE = 0;

		/* Control accshift */
		if (shiftSTATE == 1 && beta < 13107)
			delay = add(delay, 1);
		else if (shiftSTATE == -1 && beta < 13107)
			delay = sub(delay, 1);
		if (delay > DMAX)
			delay = DMAX;
		if (delay < DMIN)
			delay = DMIN;

#if 0
        if( tty_option == TTY_NO_GAIN && tty_enc_flag != 0 )
        {
            /* Pack TTY information */
            Bitpack((INT16) tty_enc_header, (UINT16 *) PackedWords, 2, PackWdsPtr);
            Bitpack((INT16) tty_enc_char, (UINT16 *) PackedWords, 5, PackWdsPtr);
        }
        else
#endif
        {
            Bitpack(sub(delay, DMIN), (UINT16 *) PackedWords, 7, PackWdsPtr);
        }

		if (bit_rate == 4)
		{
			j = sub(delay, pdelay);
			//if (abs(j) > 15)
			if (abs(j) > 15)	//maolin 2014-07-22
				j = 0;
			else
				j = add(j, 16);

			Bitpack(j, (UINT16 *) PackedWords, 5, PackWdsPtr);
		}

		/* Smooth interpolation if the difference between delays is too big */
		if (abs_s(sub(delay, pdelay)) > 15)
			pdelay = delay;

		Aveidxppg = 0;
		/*********************************
        * CELP codebook search procedure *
		*********************************/
		for (i = 0; i < NoOfSubFrames; i++)
		{
			if (i < 2)
				subframesize = sub(SubFrameSize, 1);
			else
				subframesize = SubFrameSize;

			/* interpolate lsp */
			Interpol(lspi, OldlspE, lsp, i, ORDER);
			Interpol(lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);

			/* Convert lsp to PC */
			lsp2a(pci, lspi);
			lsp2a(pci_nq, lspi_nq);

			/* Get zir */

			ZeroInput(zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1, GAMMA2, ORDER, subframesize, 0);

			/* Calculate impulse response of 1/A(z) * A(z/g1) / A(z/g2) */
			ImpulseRzp(H, pci_nq, pci, GAMMA1, GAMMA2, ORDER, Hlength);

			/* Interpolate delay */
			Interpol_delay(delayi, &pdelay, &delay, i);

			ComputeACB(residualm, Excitation + ACBMemSize, delayi,
					   residual + GUARD + i * (SubFrameSize - 1),
					   FrameSize + GUARD - i * (SubFrameSize - 1), &dpm,
					   &accshift, beta, subframesize, RSHIFT);

			/* Get weighted speech */
			/* ORIGM */

			SynthesisFilter(origm, residualm, pci_nq, SynMemoryM, ORDER, subframesize);

			/* Weighting filter */
			weight(wpci, pci_nq, GAMMA1, ORDER);
			fir(Scratch, origm, wpci, WFmemFIR, ORDER, subframesize);
			weight(wpci, pci_nq, GAMMA2, ORDER);
			iir(worigm, Scratch, wpci, WFmemIIR, ORDER, subframesize);

			/* Remove Zero input response from weighted speech */
			for (j = 0; j < subframesize; j++)
				worigm[j] = sub(worigm[j], zir[j]);

			/* Calculate closed loop gain */
			getgain(Excitation + ACBMemSize, &sum1, H, &idxppg, ppvq, ppvq_mid, ACBGainSize, 1, worigm, subframesize, Hlength);

			Aveidxppg = add(Aveidxppg, idxppg);

			/* Get TARGET for fixed c.b. */
			/* Convolve excitation with H */
			/* ExconvH stored in Scratch memory */

			ConvolveImpulseR(ExconvH, Excitation + ACBMemSize, H, Hlength, subframesize);

			for (j = 0; j < subframesize; j++)
				TARGETw[j] = sub(worigm[j], ExconvH[j]);

			/* Convert TARGET from weighted domain to residual domain */
			Weight2Res(TARGET, TARGETw, pci_nq, pci, GAMMA1, GAMMA2, ORDER, SubFrameSize);

			if (subframesize < SubFrameSize)
				TARGETw[subframesize] = TARGET[subframesize] = Scratch[subframesize] = 0;

			/* get delay for current subframe */
			n = extract_h(L_add(L_shr(L_add(delayi[1], delayi[0]), 1), 32768));
			/* Compute fixed codebook contribution */
			if (n > subframesize)
				n = 200;

			/* ACELP fixed codebook search */
			if (bit_rate == 4)
			{

				ACELP_Code(TARGETw, TARGET, H, n, sum1, subframesize, Scratch,
						   &fcbGain, y2, fcbIndexVector, 1);

				/* constrain fcb gain */
				/* fcbGain *= (1.0 - ppvq[idxppg] * 0.15); */

                l_fcbGain = L_mpy_ls(L_sub(2147483647, L_mult(ppvq[idxppg], 9830)), fcbGain);
            }
			else
			{
                /** Processing half rate case  **/

				ACELP_Code(TARGETw, TARGET, H, n, sum1, subframesize, Scratch,
						   &fcbGain, y2, fcbIndexVector, 0);

				/* Constrain fcb gain */
				/* fcbGain *= (0.9 - ppvq[idxppg] * 0.1); */
				l_fcbGain = L_mpy_ls(L_sub(1932735283, L_mult(ppvq[idxppg], 6554)), fcbGain);

			}
			/* scale fcbGain +6db to account for 6db loss at input */
            l_fcbGain = L_shl(l_fcbGain, 1);

			/* Quantize FCB Gain : */
			fcb_gainq(&idxcbg, &fcbGain, l_fcbGain, gnvq, FCBGainSize);

			/* adjust fcbGain -6db */
            fcbGain = shr(fcbGain, 1);

			/* Add to total excitation */
			for (j = 0; j < subframesize; j++)
				Excitation[j + ACBMemSize] = add(Excitation[j + ACBMemSize], round32(L_mult(Scratch[j], fcbGain)));

			/* Update filters memory */
			ZeroInput(zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1, GAMMA2, ORDER, subframesize, 1);

            /* checking outputs */
			/* Update residualm */
			for (j = 0; j < dpm; j++)
				residualm[j] = residualm[j + subframesize];

			/* Update excitation */
			for (j = 0; j < ACBMemSize; j++)
				Excitation[j] = Excitation[j + subframesize];

			/* Pack bits */
			/* ACB gain index */

			Bitpack(idxppg, (UINT16 *) PackedWords, 3, PackWdsPtr);

			if (bit_rate == 4)
			{
				idxcb = fcbIndexVector[0];
				Bitpack(idxcb, (UINT16 *) PackedWords, 8, PackWdsPtr);
				idxcb = fcbIndexVector[1];
				Bitpack(idxcb, (UINT16 *) PackedWords, 8, PackWdsPtr);
				idxcb = fcbIndexVector[2];
				Bitpack(idxcb, (UINT16 *) PackedWords, 8, PackWdsPtr);
				idxcb = fcbIndexVector[3];
				Bitpack(idxcb, (UINT16 *) PackedWords, 11, PackWdsPtr);
				/* FCB gain index */
				Bitpack(idxcbg, (UINT16 *) PackedWords, 5, PackWdsPtr);
			}
			else
			{
                /** Packing half-rate bits **/
				idxcb = fcbIndexVector[0];
				/* FCB shape index  10 bits */
				Bitpack(idxcb, (UINT16 *) PackedWords, 10, PackWdsPtr);
				/* FCB gain index */
				Bitpack(idxcbg, (UINT16 *) PackedWords, 4, PackWdsPtr);
			}

		}
		/* prevent overflow */
		if (Aveidxppg <= 1 && idxppg != 1)
		{
			accshift = 0;
			dpm = 0;
		}

		/* Update encoder variables */
		pdelay = delay;

#if 0
        if( tty_option == TTY_NO_GAIN && tty_enc_flag != 0 )
        {
            if( bit_rate == 4 )
            {
                /* Zero the delta delay when TTY is in the lag bits */
                PackedWords[2] = PackedWords[2] & ~0x0F80;

                /* Pack TTY Baud Rate information in Bit 171*/
                Bitpack((INT16) tty_enc_baud_rate, (UINT16 *) PackedWords, 1, PackWdsPtr);
            }
        }
#endif
	}							/* Ends if for rate of 4 or 8kbps */

	lastrateE = bit_rate;

	for (i = 0; i < PACKWDSNUM; i++)
		codeBuf[i] = PackedWords[i];

	encode_fcnt++;
}

/*======================================================================*/
/*         ..Save LSPs.                                                 */
/*----------------------------------------------------------------------*/
void post_encode()
{
	/*....(local) variables.... */
	register int j;

	/*....execute.... */
	for (j = 0; j < ORDER; j++)
	{
		OldlspE[j] = lsp[j];
		Oldlsp_nq[j] = lsp_nq[j];
	}
}
