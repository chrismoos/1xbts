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

#include "typedefs.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathadv.h"

#define L_SUBFR   55
#define NB_PULSE  8
#define NB_TRACK  5
#define STEP      5
#define NB_POS    11
#define MSIZE     121

#define NB_ITER   4
#define Q15_1_5   6554

const Shortword ipos[8 + 3] =
{0, 1, 2, 3, 4, 0, 1, 2, 3, 4, 0};

Longword Inv_sqrt(				/* (o) Q30 : output value   (range: 0<=val<1)           */
                 Longword L_x   /* (i) Q0  : input value    (range: 0<=val<=7fffffff)   */
);

/* locals functions */

static void cor_h_vec(
						 Shortword h[],		/* (i) scaled impulse response */
						 Shortword vec[],	/* (i) vector to correlate with h[] */
						 Shortword track,	/* (i) track to use */
						 Shortword sign[],	/* (i) sign vector */
						 Shortword rrixix[][NB_POS],	/* (i) correlation of h[x] with h[x] */
						 Shortword cor[]	/* (o) result of correlation (NB_POS elements) */
);
static void search_ixiy(
						   Shortword track_x,	/* (i) track of pulse 1 */
						   Shortword track_y,	/* (i) track of pulse 2 */
						   Shortword * ps,	/* (i/o) correlation of all fixed pulses */
						   Shortword * alp,		/* (i/o) energy of all fixed pulses */
						   Shortword * ix,	/* (o) position of pulse 1 */
						   Shortword * iy,	/* (o) position of pulse 2 */
						   Shortword dn[],	/* (i) corr. between target and h[] */
						   Shortword cn[],	/* (i) corr. vector (search if cn[]>=0)   */
						   Shortword cor_x[],	/* (i) corr. of pulse 1 with fixed pulses */
						   Shortword cor_y[],	/* (i) corr. of pulse 2 with fixed pulses */
						   Shortword rrixiy[][MSIZE]	/* (i) corr. of pulse 1 with pulse 2 */
);

/*-------------------------------------------------------------------*
 * Function  code_8i55_35bits()                                      *
 * ~~~~~~~~~~~~~~~~~~~~~~~~~~~~                                      *
 * Algebraic codebook; 35 bits: 8 pulses in a frame of 55 samples.   *
 *-------------------------------------------------------------------*
 * The code length is 55, containing 8 nonzero pulses: i0...i7.      *
 * All pulses can have two (2) possible amplitudes: +1 or -1.        *
 * Each pulse can have 11 possible positions.                        *
 * The 8 pulses can be on the following 5 tracks:                    *
 *     track0 :  0, 5, .. 50                                         *
 *     track1 :  1, 6, .. 51                                         *
 *     track2 :  2, 7, .. 52                                         *
 *     track3 :  3, 8, .. 53                                         *
 *     track4 :  4, 9, .. 54                                         *
 * 3 tracks has 2 pulses (i0..i5), 2 tracks has 1 pulses (i6,i7)     *
 *                                                                   *
 * NOTE: A pulse is unused (amplitude = 0) when its position is      *
 *       quantized outside l_subfr (l_subfr = 53 or 54).             *
 *-------------------------------------------------------------------*/

void code_8i55_35bits(
						 Shortword dn[],	/* (i) Q0 : dn[55] correlation between target and h[]     */
						 Shortword cn[],	/* (i) Q0 : cn[55] residual after INT32 term prediction    */
						 Shortword l_subfr,		/* (i)    : lenght of subframe (53 or 54)                 */
						 Shortword H[],		/* (i) Q12: impulse response of weighted synthesis filter */
						 Shortword code[],	/* (o) Q12: algebraic (fixed) codebook excitation         */
						 Shortword y[],		/* (o) Q10: filtered fixed codebook excitation            */
						 Shortword indx[]	/* (o)    : index 4 words: 8 + 8 + 8 + 11 = 35 bits       */
)
{
	Shortword i, j, k, ix, iy, pos, index, track;
	Shortword psk, ps, alpk, alp;
	Shortword val, snorm, snorm1, snorm2, s_short, ps1_short, ps2_short;
	Longword s, cor, ps1, ps2;
	Shortword *p0, *p1, *p2, *p3, *p4, *p5, *p6, *p7, *psign;
	Shortword *h, *h_inv, *ptr_h1, *ptr_h2, *ptr_hf;

	Shortword sign[L_SUBFR], vec[L_SUBFR];
	Shortword ip[NB_PULSE], codvec[NB_PULSE + 1], pos_max[NB_TRACK];
	Shortword cor_x[NB_POS], cor_y[NB_POS];
	Shortword h_buf[4 * L_SUBFR];
	Shortword rrixix[NB_PULSE][NB_POS], rrixiy[NB_PULSE][MSIZE];

	pos = -1; 	/* initialize to nonsense position to eliminate warning */
				/* Does not effect code since pos will not be used */

	for (i=l_subfr; i<L_SUBFR; i++)
	{
		cn[i] = dn[i] = 0;
	}

	/* calculate energy for normalization of cn[] and dn[] */
	ps1 = 0;
	for (i = 0; i < L_SUBFR; i++)
		ps1 = L_mac(ps1, cn[i], cn[i]); /* LT -- TJ Fixed 7/1/96 */

	ps2 = 0;
	for (i = 0; i < L_SUBFR; i++)
		ps2 = L_mac(ps2, dn[i], dn[i]); /* LT -- TJ Fixed 7/1/96 */

	/* NOTE: The equation for determination the sign of the excitation    */
	/*       is changed to an equivalent equation to avoid the dividition */
	/*       operation */

	/* Normalize the ps1 and ps2 */
	snorm1 = norm_l(ps1);
	snorm2 = norm_l(ps2);

	ps1_short = extract_h(L_shl(ps1, snorm1));
	ps2_short = extract_h(L_shl(ps2, snorm2));

	/* Compute s = sqrt(ps1*ps2) */

	snorm1 = add(snorm1, snorm2);

	s = L_mult(ps1_short, ps2_short);
	snorm = norm_l(s);
	s_short = sqroot(L_shl(s, snorm));
	snorm = add(snorm, snorm1);

	if ((snorm) & 0x0001)		/*if snorm is odd, multiply by sqrt(2)/2 */
		s_short = mult(s_short, 23170);

	snorm = shr(snorm, 1);

	snorm = add(snorm, 1);
	snorm2 = add(snorm2, 2);
	/* Find the common dynamic shift snorm1 */
	if (snorm2 > snorm)
		snorm1 = snorm;
	else
		snorm1 = snorm2;

	snorm2 = sub(snorm2, snorm1);
	snorm = sub(snorm, snorm1);

	/* set sign according to en[] = ps1*cn[] + sqrt(ps1*ps2)*dn[]    */
	/* find position of maximum of correlation in each track */

	for (k = 0; k < NB_TRACK; k++)
	{
		cor = -1;
		for (i = k; i < L_SUBFR; i += STEP)
		{
			val = dn[i];
			s = L_add(L_shr(L_mult(ps2_short, cn[i]), snorm2), L_shr(L_mult(s_short, dn[i]), snorm));

			if (s >= 0)
			{
				sign[i] = 32767;	/* sign = +1 (Q15) */
				vec[i] = -32768;
			}
			else
			{
				sign[i] = -32768;	/* sign = -1 (Q15) */
				vec[i] = 32767;
				val = negate(val);
			}
			dn[i] = val;		/* modify dn[] according to the fixed sign */
			s = L_abs(s);
			cn[i] = extract_h(L_shl(s, 8));
			if (s > cor)
			{
				cor = s;
				pos = i;
			}
		}
		pos_max[k] = pos;
	}

	/* select 6 positions per track (criterion: max of corr.) */

	for (i = 0; i < NB_TRACK; i++)
	{
		for (k = 0; k < 5; k++)
		{
			ps = 32767;
			pos = i;
			for (j = i; j < L_SUBFR; j += STEP)
			{
				if ((cn[j] <= ps) && (cn[j] >= 0))
				{
					ps = cn[j];
					pos = j;
				}
			}
			cn[pos] = -1;		/* position not selected */
		}
	}

 /*------------------------------------------------------------*
  * normalize h[] for maximum precision on correlation.        *
  *------------------------------------------------------------*/

	h = h_buf;
	h_inv = h_buf + (2 * L_SUBFR);
	/* for (i = 0; i < ((2 * L_SUBFR) - l_subfr); i++) */
	 for (i = 0; i < L_SUBFR ; i++) 
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

	/* Add to set other be zero */
       for (i=l_subfr; i < L_SUBFR; i++)
	 {
          h[i]=0.0;
          h_inv[i]=0.0;
         } 

       /* h -= (L_SUBFR - l_subfr);
	h_inv -= (L_SUBFR - l_subfr); */
 
    
 /*------------------------------------------------------------*
  * Compute rrixix[][] needed for the codebook search.         *
  * This algorithm compute impulse response energy of all      *
  * positions 11 in each track (5).         Total = 5x11 = 55. *
  *------------------------------------------------------------*/

	/* storage order --> i4i4, i3i3, i2i2, i1i1, i0i0 */

	/* Init pointers to last position of rrixix[] */
	p0 = &rrixix[0][NB_POS - 1];
	p1 = &rrixix[1][NB_POS - 1];
	p2 = &rrixix[2][NB_POS - 1];
	p3 = &rrixix[3][NB_POS - 1];
	p4 = &rrixix[4][NB_POS - 1];

	ptr_h1 = h;
	cor = 0x00010000L;			/* 1.0 (for rounding) */
	for (i = 0; i < NB_POS; i++)
	{
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p4-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p3-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p2-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p1-- = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h1);
		ptr_h1++;
		*p0-- = extract_h(cor);
	}

	/* Divide all elements of rrixix[][] by 2. */

	p0 = &rrixix[0][0];
	for (i = 0; i < L_SUBFR; i++)
		*p0++ = shr(*p0, 1);

 /*------------------------------------------------------------*
  * Compute rrixiy[][] needed for the codebook search.         *
  * This algorithm compute correlation between 2 pulses        *
  * (2 impulses responses) in 5 possible adjacents tracks.     *
  * (track 0-1, 1-2, 2-3, 3-4 and 4-0).   Total = 5x11x11 =605.*
  *------------------------------------------------------------*/

	/* storage order --> i3i4, i2i3, i1i2, i0i1, i4i0 */

	pos = MSIZE - 1;
	ptr_hf = h + 1;

	for (k = 0; k < NB_POS; k++)
	{
		p4 = &rrixiy[3][pos];
		p3 = &rrixiy[2][pos];
		p2 = &rrixiy[1][pos];
		p1 = &rrixiy[0][pos];
		p0 = &rrixiy[4][pos - NB_POS];

		cor = 0x00008000L;		/* 0.5 (for rounding) */
		ptr_h1 = h;
		ptr_h2 = ptr_hf;

		for (i = k + (Shortword) 1; i < NB_POS; i++)
		{
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p4 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p3 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p2 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p1 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);

			p4 -= (NB_POS + 1);
			p3 -= (NB_POS + 1);
			p2 -= (NB_POS + 1);
			p1 -= (NB_POS + 1);
			p0 -= (NB_POS + 1);
		}
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p4 = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p3 = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p2 = extract_h(cor);
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p1 = extract_h(cor);

		pos -= NB_POS;
		ptr_hf += STEP;
	}

	/* storage order --> i4i0, i3i4, i2i3, i1i2, i0i1 */

	pos = MSIZE - 1;
	ptr_hf = h + 4;

	for (k = 0; k < NB_POS; k++)
	{
		p4 = &rrixiy[4][pos];
		p3 = &rrixiy[3][pos - 1];
		p2 = &rrixiy[2][pos - 1];
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
			*p4 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p3 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p2 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p1 = extract_h(cor);
			cor = L_mac(cor, *ptr_h1, *ptr_h2);
			ptr_h1++;
			ptr_h2++;
			*p0 = extract_h(cor);

			p4 -= (NB_POS + 1);
			p3 -= (NB_POS + 1);
			p2 -= (NB_POS + 1);
			p1 -= (NB_POS + 1);
			p0 -= (NB_POS + 1);
		}
		cor = L_mac(cor, *ptr_h1, *ptr_h2);
		ptr_h1++;
		ptr_h2++;
		*p4 = extract_h(cor);

		pos--;
		ptr_hf += STEP;
	}

 /*------------------------------------------------------------*
  * Modification of rrixiy[][] to take signs into account.     *
  *------------------------------------------------------------*/

	p0 = &rrixiy[0][0];

	for (k = 0; k < NB_TRACK; k++)
	{
		for (i = k; i < L_SUBFR; i += STEP)
		{
			psign=sign;

			if (psign[i] < 0)
				psign = vec;

			for (j = ((k + 1) % NB_TRACK); j < L_SUBFR; j += STEP)
			{
				*p0++ = mult(*p0, psign[j]);
			}
		}
	}

 /*-------------------------------------------------------------------*
  * Deep first search: 4 iterations of 264 tests = 1056 tests.        *
  *                                                                   *
  * Stages of deep first search:                                      *
  *     stage 1 : fix i0 and i1 --> try 6x11 = 66 positions           *
  *     stage 2 : fix i2 and i3 --> try 6x11 = 66 positions.          *
  *     stage 3 : fix i4 and i5 --> try 6x11 = 66 positions.          *
  *     stage 4 : fix i6 and i7 --> try 6x11 = 66 positions.          *
  *-------------------------------------------------------------------*/

	/* default value */
	psk = -1;
	alpk = 1;
	for (i = 0; i < NB_PULSE; i++)
		codvec[i] = i;

	for (k = 0; k < NB_ITER; k++)
	{
             /* Change  back to the search strategy used in the float point simulation */

		
		alp = 0;
                ps = 0;

               
		
                search_ixiy(ipos[k], ipos[k + 1], &ps, &alp, &ix, &iy,
		dn, cn, rrixix[ipos[k]], rrixix[ipos[k+1]], rrixiy);

			ip[0] = ix;
			ip[1] = iy;

 	       for (i = 0; i < L_SUBFR; i++)
		{
			vec[i] = 0;
		}
                

		/* stage 2..5: fix pulse i2,i3,i4,i5,i6 and i7 */

		for (j = 2; j < NB_PULSE; j += 2)
		{
     /*--------------------------------------------------*
      * Store all impulse response of all fixed pulses   *
      * in vector vec[] for the "cor_h_vec()" function.  *
      *--------------------------------------------------*/

			if (sign[ix] < 0)
				p0 = h_inv - ix;
			else
				p0 = h - ix;
			if (sign[iy] < 0)
				p1 = h_inv - iy;
			else
				p1 = h - iy;

			for (i = 0; i < L_SUBFR; i++)
			{
				vec[i] = add(vec[i], add(*p0++, *p1++));
			}

     /*--------------------------------------------------*
      * Calculate correlation of all possible positions  *
      * of the next 2 pulses with previous fixed pulses. *
      * Each pulse can have 8 possible positions         *
      *--------------------------------------------------*/

			cor_h_vec(h, vec, ipos[k + j], sign, rrixix, cor_x);
			cor_h_vec(h, vec, ipos[k + j + 1], sign, rrixix, cor_y);

      /*--------------------------------------------------*
      * Fix 2 pulses, try 6x11 = 66 positions.            *
      *--------------------------------------------------*/

		search_ixiy(ipos[k + j], ipos[k + j + 1], &ps, &alp, &ix, &iy,
		dn, cn, cor_x, cor_y, rrixiy);

			ip[j] = ix;
			ip[j + 1] = iy;
		}

		/* memorise new codevector if it's better than the last one. */

		ps = mult(ps, ps);
		s = L_msu(L_mult(alpk, ps), psk, alp);

		if (s > 0)
		{
			psk = ps;
			alpk = alp;
			for (i = 0; i < NB_PULSE; i++)
				codvec[i] = ip[i];
			codvec[8] = k;
		}

	}							/* end of for (k=0; k<NB_ITER; k++) */

 /*-------------------------------------------------------------------*
  * Build the codeword, the filtered codeword and index of codevector.*
  *-------------------------------------------------------------------*/

	for (i = 0; i < l_subfr; i++)
	{
		code[i] = 0;
	}

	for (i = 0; i < NB_TRACK; i++)
	{
		indx[i] = -1;
	}

	for (k = 0; k < NB_PULSE; k++)
	{
		i = codvec[k];			/* read pulse position */
		j = sign[i];			/* read sign           */

		index = mult(i, Q15_1_5);	/* index = pos/5       */
		/* track = pos%5 */
		track = sub(i, extract_l(L_shr(L_mult(index, 5), 1)));

		if (j > 0)
		{
			if (i < l_subfr)
				code[i] = add(code[i], 4096);	/* codeword in Q12 format */
			codvec[k] += (2 * L_SUBFR);
		}
		else
		{
			if (i < l_subfr)
				code[i] = sub(code[i], 4096);	/* codeword in Q12 format */
			index = add(index, 16);
		}

	/*-------------------------------------------------------*
	 * Quantization of 2 pulses with 8 or 9 bits:            *
	 *     1 bit  for 2 pulses sign.     (b8)                *
	 *     7 bits for 2 pulses in same track  (b7..b4)       *
	 *     8 bits for 2 pulses in different track  (b7..b4)  *
	 *-------------------------------------------------------*/
		if (indx[track] < 0)
		{
			indx[track] = index;
		}
		else
		{
			if (((index ^ indx[track]) & 16) == 0)
			{
				/* sign of 1st pulse == sign of 2th pulse */

				if (sub(indx[track], index) <= 0)
				{
					indx[track] = shl((indx[track] & 16), 3)
						+ shr(extract_l(L_mult((indx[track] & 15), NB_POS)), 1)
						+ (index & 15);
				}
				else
				{
					indx[track] = shl((index & 16), 3)
						+ shr(extract_l(L_mult((index & 15), NB_POS)), 1)
						+ (indx[track] & 15);
				}
			}
			else
			{
				/* sign of 1st pulse != sign of 2th pulse */

				if (sub((indx[track] & 15), (index & 15)) <= 0)
				{
					indx[track] = shl((index & 16), 3)
						+ shr(extract_l(L_mult((index & 15), NB_POS)), 1)
						+ (indx[track] & 15);
				}
				else
				{
					indx[track] = shl((indx[track] & 16), 3)
						+ shr(extract_l(L_mult((indx[track] & 15), NB_POS)), 1)
						+ (index & 15);
				}
			}
		}
	}

	if (codvec[8] == 0)
	{
		i = indx[3];
		j = indx[4];
	}
	else if (codvec[8] == 1)
	{
		i = indx[4];
		j = indx[0];
		indx[0] = indx[1];
		indx[1] = indx[2];
		indx[2] = indx[3];
	}
	else if (codvec[8] == 2)
	{
		i = indx[0];
		j = indx[1];
		indx[0] = indx[2];
		indx[1] = indx[3];
		indx[2] = indx[4];
	}
	else if (codvec[8] == 3)
	{
		i = indx[1];
		j = indx[2];
		indx[1] = indx[4];
		indx[2] = indx[0];
		indx[0] = indx[3];
	}

	indx[3] = shl(codvec[8], 9)
		+ shl((i & 16), 4)
		+ shl((j & 16), 3)
		+ shr(extract_l(L_mult((i & 15), NB_POS)), 1)
		+ (j & 15);

	for (i = 0; i < l_subfr; i++)
	{
		h[i] = H[i];
		h_inv[i] = negate(h[i]);
	}

	p0 = h_inv - codvec[0];
	p1 = h_inv - codvec[1];
	p2 = h_inv - codvec[2];
	p3 = h_inv - codvec[3];
	p4 = h_inv - codvec[4];
	p5 = h_inv - codvec[5];
	p6 = h_inv - codvec[6];
	p7 = h_inv - codvec[7];

	for (i = 0; i < l_subfr; i++)
	{
		s = L_mult(*p0++, 8192);	/* Q12 --> Q10 */
		s = L_mac(s, *p1++, 8192);
		s = L_mac(s, *p2++, 8192);
		s = L_mac(s, *p3++, 8192);
		s = L_mac(s, *p4++, 8192);
		s = L_mac(s, *p5++, 8192);
		s = L_mac(s, *p6++, 8192);
		s = L_mac(s, *p7++, 8192);
		y[i] = round32(s);
	}

	return;
}

/*-------------------------------------------------------------------*
 * Function  cor_h_vec()                                             *
 * ~~~~~~~~~~~~~~~~~~~~~                                             *
 * Compute correlations of h[] with vec[] for the specified track.   *
 *-------------------------------------------------------------------*/

static void cor_h_vec(
						 Shortword h[],		/* (i) scaled impulse response */
						 Shortword vec[],	/* (i) vector to correlate with h[] */
						 Shortword track,	/* (i) track to use */
						 Shortword sign[],	/* (i) sign vector */
						 Shortword rrixix[][NB_POS],	/* (i) correlation of h[x] with h[x] */
						 Shortword cor[]	/* (o) result of correlation (NB_POS elements) */
)
{
	Shortword i, j, pos;
	Shortword *p0, *p1, *p2;
	Longword s;

	p0 = rrixix[track];

	pos = track;
	for (i = 0; i < NB_POS; i++, pos += STEP)
	{
		s = 0;
		p1 = h;
		p2 = &vec[pos];
		for (j = pos; j < L_SUBFR; j++)
			s = L_mac(s, *p1++, *p2++);

		cor[i] = add(mult(round32(s), sign[pos]), *p0++);
	}

	return;
}

/*-------------------------------------------------------------------*
 * Function  search_ixiy()                                           *
 * ~~~~~~~~~~~~~~~~~~~~~~~                                           *
 * Find the best positions of 2 pulses in a subframe.                *
 *-------------------------------------------------------------------*/

static void search_ixiy(
						   Shortword track_x,	/* (i) track of pulse 1 */
						   Shortword track_y,	/* (i) track of pulse 2 */
						   Shortword * ps,	/* (i/o) correlation of all fixed pulses */
						   Shortword * alp,		/* (i/o) energy of all fixed pulses */
						   Shortword * ix,	/* (o) position of pulse 1 */
						   Shortword * iy,	/* (o) position of pulse 2 */
						   Shortword dn[],	/* (i) corr. between target and h[] */
						   Shortword cn[],	/* (i) corr. vector (search if cn[]>=0)   */
						   Shortword cor_x[],	/* (i) corr. of pulse 1 with fixed pulses */
						   Shortword cor_y[],	/* (i) corr. of pulse 2 with fixed pulses */
						   Shortword rrixiy[][MSIZE]	/* (i) corr. of pulse 1 with pulse 2 */
)
{
	Shortword x, y, pos;
	Shortword ps1, ps2, sq, sqk;
	Shortword alp1, alp2, alpk;
	Shortword *p0, *p1, *p2;
	Longword s;

	p0 = cor_x;
	p1 = cor_y;
	p2 = rrixiy[track_x];

	/* default value */
	sqk = -1;
	alpk = 1;
	*ix = track_x;
	*iy = track_y;

	for (x = track_x; x < L_SUBFR; x += STEP)
	{
		ps1 = add(*ps, dn[x]);
		alp1 = add(*alp, *p0++);

		if (cn[x] >= 0)
		{
			pos = -1;
			for (y = track_y; y < L_SUBFR; y += STEP)
			{
				ps2 = add(ps1, dn[y]);
				alp2 = add(alp1, add(*p1++, *p2++));
				sq = mult(ps2, ps2);

				s = L_msu(L_mult(alpk, sq), sqk, alp2);

				if (s > 0)
				{
					sqk = sq;
					alpk = alp2;
					pos = y;
				}
			}
			p1 -= NB_POS;

			if (pos >= 0)
			{
				*ix = x;
				*iy = pos;
			}
		}
		else
		{
			p2 += NB_POS;
		}
	}

	*ps = add(*ps, add(dn[*ix], dn[*iy]));
	*alp = alpk;

	return;
}

