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
/*  Memory Usage:                           */
/*      ROM:                            */
/*      Static/Global RAM:                      */
/*      Stack/Local RAM:                    */
/*----------------------------------------------------------------------*/

#include "typedefs.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathadv.h"

#define L_SUBFR   54
#define NB_PULSE  3
#define NB_TRACK  3
#define STEP      7
#define NB_POS    8
#define MSIZE     64

#define Q15_1_7  4682

Longword Inv_sqrt(				/* (o) Q30 : output value   (range: 0<=val<1)           */
					 Longword L_x	/* (i) Q0  : input value    (range: 0<=val<=7fffffff)   */
);

/*-------------------------------------------------------------------*
 * Function  code_3i54_10bits()                                      *
 * ~~~~~~~~~~~~~~~~~~~~~~~~~~~~                                      *
 * Algebraic codebook; 10 bits: 3 pulses in a frame of 54 samples.   *
 *-------------------------------------------------------------------*
 * The code length is 54, containing 3 nonzero pulses: i0, i1 and i2.*
 * All pulses have a fixed amplitude: +1 for i0 and i2, -1 for i1.   *
 * 1 bit is used to change sign of all pulses.                       *
 * Each pulse can have 8 possible positions:                         *
 *                                                                   *
 * track0 (i0):  0,  7, 14, ... 49                                   *
 * track1 (i1):  2,  9, 16, ... 51                                   *
 * track2 (i2):  4, 11, 18, ... 53                                   *
 *-------------------------------------------------------------------*/

void code_3i54_10bits(
						 Shortword dn[],	/* (i) Q0 : dn[54] correlation between target and h[]     */
						 Shortword l_subfr,		/* (i)    : lenght of subframe (53 or 54)                 */
						 Shortword H[],		/* (i) Q12: impulse response of weighted synthesis filter */
						 Shortword code[],	/* (o) Q12: algebraic (fixed) codebook excitation         */
						 Shortword y[],		/* (o) Q10: filtered fixed codebook excitation            */
						 Shortword * indx	/* (o)    : index 1 words of 10 bits                      */
)
{
	Shortword i, k, i0, i1, i2, pos, index;
	Shortword psk, ps0, ps1, ps2, alpk, alp0, alp1, alp2;
	Longword s, cor;
	Shortword *p0, *p1, *p2, *p3, *p4, *p5;
	Shortword *h, *h_inv, *ptr_h1, *ptr_h2, *ptr_hf;

	/* these vectors are not static */

	Shortword codvec[NB_PULSE];
	Shortword cor_l2[NB_POS];
	Shortword h_buf[4 * L_SUBFR];
	Shortword rrixix[NB_PULSE][NB_POS], rrixiy[NB_PULSE][MSIZE];

	for (i=l_subfr; i<L_SUBFR; i++)
	{
		dn[i] = 0;
	}

 /*------------------------------------------------------------*
  * normalize h[] for maximum precision on correlation.        *
  *------------------------------------------------------------*/

	h = h_buf;
	h_inv = h_buf + (2 * L_SUBFR);
	for (i = 0; i < ((2 * L_SUBFR) - l_subfr); i++)
	{
		*h++ = 0;
		*h_inv++ = 0;
	}

	cor = 0;
	for (i = 0; i < l_subfr; i++)
		cor = L_mac(cor, H[i], H[i]);

	/* scale h[] with shift operation */

	k = norm_l(cor);
	k = shr(k, 1);

	for (i = 0; i < l_subfr; i++)
	{
		h[i] = shl(H[i], k);
	}
	cor = L_shl(cor, add(k, k));

 /*------------------------------------------------------------*
  * Scaling h[] with a factor (0.5 < fac < 0.25)               *
  * ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~               *
  * extract_h(cor) = 8192 .. 32768 --> we want 4096 (1/8 Q15)  *
  *                                                            *
  * 4096 (1/8) = fac^2 * extract_h(cor)                        *
  * fac = sqrt(4096/extract_h(cor))                            *
  *                                                            *
  * fac = 1/sqrt(cor/4096) * 256 = 0.125 to 0.5                *
  *------------------------------------------------------------*/

	cor = L_shr(cor, 12);
	k = extract_h(L_shl(Inv_sqrt(cor), 8));

	for (i = 0; i < l_subfr; i++)
	{
		h[i] = mult(h[i], k);
		h_inv[i] = negate(h[i]);
	}

	h -= (L_SUBFR - l_subfr);
	h_inv -= (L_SUBFR - l_subfr);

 /*------------------------------------------------------------*
  * Compute rrixix[][] needed for the codebook search.         *
  * This algorithm compute impulse response energy of all      *
  * positions (8) in each track (5).         Total = 5x8 = 40. *
  *------------------------------------------------------------*/

	/* storage order --> i2i2, i1i1, i0i0 */

	/* Init pointers to last position of rrixix[] */
	p0 = &rrixix[0][NB_POS - 1];
	p1 = &rrixix[1][NB_POS - 1];
	p2 = &rrixix[2][NB_POS - 1];

	ptr_h1 = h;
	cor = 0x00010000L;			/* 1.0 (for rounding) */
	for (i = 0; i < NB_POS; i++)
	{
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p2-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p1-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p0-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
	}

	/* Divide all elements of rrixix[][] by 2. */

	p0 = &rrixix[0][0];
        for (i = 0; i < NB_PULSE*NB_POS ; i++)
		*p0++ = shr(*p0, 1);

 /*------------------------------------------------------------*
  * Compute rrixiy[][] needed for the codebook search.         *
  * This algorithm compute correlation between 2 pulses.       *
  *------------------------------------------------------------*/

	/* storage order --> i1i2, i0i1 */

	pos = MSIZE - 1;
	ptr_hf = h + 2;

	for (k = 0; k < NB_POS; k++)
	{
		p1 = &rrixiy[1][pos];
		p0 = &rrixiy[0][pos];

		cor = 0x00008000L;		/* 0.5 (for rounding) */
		ptr_h1 = h;
		ptr_h2 = ptr_hf;

		for (i = k + (Shortword) 1; i < NB_POS; i++)
		{
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p1 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;

			p1 -= (NB_POS + 1);
			p0 -= (NB_POS + 1);
		}
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p1 = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p0 = extract_h(cor);

		pos -= NB_POS;
		ptr_hf += STEP;
	}

	/* storage order --> i1i2, i0i1 */

	pos = MSIZE - 1;
	ptr_hf = h + 5;

	for (k = 0; k < NB_POS; k++)
	{
		p1 = &rrixiy[1][pos - 1];
		p0 = &rrixiy[0][pos - 1];

		cor = 0x00008000L;		/* 0.5 (for rounding) */
		ptr_h1 = h;
		ptr_h2 = ptr_hf;

		for (i = k + (Shortword) 1; i < NB_POS; i++)
		{
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p1 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;

			p1 -= (NB_POS + 1);
			p0 -= (NB_POS + 1);
		}

		pos--;
		ptr_hf += STEP;
	}

	/* storage order --> i0i2 */

	pos = MSIZE - 1;
	ptr_hf = h + 4;

	for (k = 0; k < NB_POS; k++)
	{
		p0 = &rrixiy[2][pos];

		cor = 0x00008000L;		/* 0.5 (for rounding) */
		ptr_h1 = h;
		ptr_h2 = ptr_hf;

		for (i = k + (Shortword) 1; i < NB_POS; i++)
		{
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;

			p0 -= (NB_POS + 1);
		}
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p0 = extract_h(cor);

		pos -= NB_POS;
		ptr_hf += STEP;
	}

	/* storage order --> i0i2 */

	pos = MSIZE - 1;
	ptr_hf = h + 3;

	for (k = 0; k < NB_POS; k++)
	{
		p0 = &rrixiy[2][pos - 1];

		cor = 0x00008000L;		/* 0.5 (for rounding) */
		ptr_h1 = h;
		ptr_h2 = ptr_hf;

		for (i = k + (Shortword) 1; i < NB_POS; i++)
		{
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;

			p0 -= (NB_POS + 1);
		}

		pos--;
		ptr_hf += STEP;
	}

 /*-------------------------------------------------------------------*
  *                 SEARCH THE BEST CODEVECTOR.                       *
  *                                                                   *
  * complexity: 8x8x8 = 512 tests                                     *
  *-------------------------------------------------------------------*/

	/* default value */
	psk = -1;
	alpk = 1;
	codvec[0] = 0;
	codvec[1] = 2;
	codvec[2] = 4;

	p0 = rrixix[0];
	p2 = rrixiy[0];
	p4 = rrixiy[2];

	for (i0 = 0; i0 < L_SUBFR; i0 += STEP)
	{
		ps0 = dn[i0];
		alp0 = *p0++;

		p1 = rrixix[1];
		p5 = rrixiy[1];

		for (i = 0; i < NB_POS; i++)
			cor_l2[i] = add(rrixix[2][i], *p4++);

		for (i1 = 2; i1 < L_SUBFR; i1 += STEP)
		{
			ps1 = sub(ps0, dn[i1]);
			/* alp1 = alp0 + rr[i1][i1] - 2.0 * rr[i0][i1]; */
			alp1 = add(alp0, sub(*p1++, *p2++));

			p3 = cor_l2;

			for (i2 = 4; i2 < L_SUBFR; i2 += STEP)
			{
				ps2 = add(ps1, dn[i2]);
				/* alp2 = alp1 + rr[i2][i2] + 2.0 * (rr[i0][i2] - rr[i1][i2]); */
				alp2 = add(alp1, sub(*p3++, *p5++));

				ps2 = mult(ps2, ps2);
				s = L_msu(L_mult(alpk, ps2), psk, alp2);

				if (s > 0)
				{
					psk = ps2;
					alpk = alp2;
					codvec[0] = i0;
					codvec[1] = i1;
					codvec[2] = i2;
				}
			}
		}
	}

 /*-------------------------------------------------------------------*
  * Build the codeword, the filtered codeword and index of codevector.*
  *-------------------------------------------------------------------*/

	ps2 = sub(add(dn[codvec[0]], dn[codvec[2]]), dn[codvec[1]]);

	for (i = 0; i < l_subfr; i++)
	{
		code[i] = 0;
	}

	if (ps2 < 0)
		index = 1;				/* global sign */
	else
		index = 0;

	for (k = 0; k < NB_PULSE; k++)
	{
		i = codvec[k];			/* read pulse position */

		pos = mult(i, Q15_1_7);	/* pos = position/7 */
		index = add(shl(index, 3), pos);

		if (((k != 1) && (ps2 >= 0)) || ((k == 1) && (ps2 < 0)))
		{
			if (i < l_subfr)
				code[i] = 4096;	/* codeword in Q12  format */
			codvec[k] += (2 * L_SUBFR);
		}
		else
		{
			if (i < l_subfr)
				code[i] = -4096;	/* codeword in Q12  format */
		}
	}

	for (i = 0; i < l_subfr; i++)
	{
		h[i] = H[i];
		h_inv[i] = negate(h[i]);
	}

	p0 = h_inv - codvec[0];
	p1 = h_inv - codvec[1];
	p2 = h_inv - codvec[2];

	for (i = 0; i < l_subfr; i++)
	{
		s = L_mult(*p0++, 8192);	/* Q12 --> Q10 */
		s = L_mac(s, *p1++, 8192);
		s = L_mac(s, *p2++, 8192);
		y[i] = round32(s);
	}

	*indx = index;

	return;
}
