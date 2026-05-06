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
/* r_fft.c */
/*****************************************************************
*
* This is an implementation of decimation-in-time FFT algorithm for
* real sequences.  The techniques used here can be found in several
* books, e.g., i) Proakis and Manolakis, "Digital Signal Processing",
* 2nd Edition, Chapter 9, and ii) W.H. Press et. al., "Numerical
* Recipes in C", 2nd Ediiton, Chapter 12.
*
* Input -  There are two inputs to this function:
*
*	1) An integer pointer to the input data array 
*	2) An integer value which should be set as +1 for FFT
*	   and some other value, e.g., -1 for IFFT
*
* Output - There is no return value.
*	The input data are replaced with transformed data.  If the
*	input is a real time domain sequence, it is replaced with
*	the complex FFT for positive frequencies.  The FFT value 
*	for DC and the foldover frequency are combined to form the
*	first complex number in the array.  The remaining complex
*	numbers correspond to increasing frequencies.  If the input
*	is a complex frequency domain sequence arranged	as above,
*	it is replaced with the corresponding time domain sequence. 
*
* Notes:
*
*	1) This function is designed to be a part of a noise supp-
*	   ression algorithm that requires 128-point FFT of real
*	   sequences.  This is achieved here through a 64-point
*	   complex FFT.  Consequently, the FFT size information is
*	   not transmitted explicitly.  However, some flexibility
*	   is provided in the function to change the size of the 
*	   FFT by specifying the size information through "define"
*	   statements.
*
*	2) The values of the complex sinusoids used in the FFT 
*	   algorithm are computed once (i.e., the first time the
*	   r_fft function is called) and stored in a table. To
*	   further speed up the algorithm, these values can be
*	   precomputed and stored in a ROM table in actual DSP
*	   based implementations.
*
*	3) In the c_fft function, the FFT values are divided by
*	   2 after each stage of computation thus dividing the
*	   final FFT values by 64.  No multiplying factor is used
*	   for the IFFT.  This is somewhat different from the usual
*	   definition of FFT where the factor 1/N, i.e., 1/64, is
*	   used for the IFFT and not the FFT.  No factor is used in
*	   the r_fft function.
*
*	4) Much of the code for the FFT and IFFT parts in r_fft
*	   and c_fft functions are similar and can be combined.
*	   They are, however, kept separate here to speed up the
*	   execution.
*
*****************************************************************/
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"

#define			SIZE			128
#define			SIZE_BY_TWO		64
#define			NUM_STAGE		6
#define			TRUE			1
#define			FALSE			0

static Shortword phs_tbl[] =
{

	32767, 0, 32729, -1608, 32610, -3212, 32413, -4808,
	32138, -6393, 31786, -7962, 31357, -9512, 30853, -11039,
	30274, -12540, 29622, -14010, 28899, -15447, 28106, -16846,
	27246, -18205, 26320, -19520, 25330, -20788, 24279, -22006,
	23170, -23170, 22006, -24279, 20788, -25330, 19520, -26320,
	18205, -27246, 16846, -28106, 15447, -28899, 14010, -29622,
	12540, -30274, 11039, -30853, 9512, -31357, 7962, -31786,
	6393, -32138, 4808, -32413, 3212, -32610, 1608, -32729,
	0, -32768, -1608, -32729, -3212, -32610, -4808, -32413,
	-6393, -32138, -7962, -31786, -9512, -31357, -11039, -30853,
	-12540, -30274, -14010, -29622, -15447, -28899, -16846, -28106,
	-18205, -27246, -19520, -26320, -20788, -25330, -22006, -24279,
	-23170, -23170, -24279, -22006, -25330, -20788, -26320, -19520,
	-27246, -18205, -28106, -16846, -28899, -15447, -29622, -14010,
	-30274, -12540, -30853, -11039, -31357, -9512, -31786, -7962,
	-32138, -6393, -32413, -4808, -32610, -3212, -32729, -1608

};

static Shortword ii_table[] =
{SIZE / 2, SIZE / 4, SIZE / 8, SIZE / 16, SIZE / 32, SIZE / 64};

/* FFT/IFFT function for complex sequences */

/*
 * The decimation-in-time complex FFT/IFFT is implemented below.
 * The input complex numbers are presented as real part followed by
 * imaginary part for each sample.  The counters are therefore
 * incremented by two to access the complex valued samples.
 */
#ifndef ANDROID
void c_fft(Shortword * farray_ptr, Shortword isign)
{

	Shortword i, j, k, ii, jj, kk, ji, kj;
	Longword ftmp, ftmp_real, ftmp_imag;
	Shortword tmp, tmp1, tmp2;

/* Rearrange the input array in bit reversed order */
	for (i = 0, j = 0; i < SIZE - 2; i = i + 2)
	{
		if (j > i)
		{
			ftmp = *(farray_ptr + i);
			*(farray_ptr + i) = *(farray_ptr + j);
			*(farray_ptr + j) = ftmp;

			ftmp = *(farray_ptr + i + 1);
			*(farray_ptr + i + 1) = *(farray_ptr + j + 1);
			*(farray_ptr + j + 1) = ftmp;
		}

		k = SIZE_BY_TWO;
		while (j >= k)
		{
			j = sub(j, k);
			k = shr(k, 1);
		}
		j += k;
	}

/* The FFT part */
	if (isign == 1)
	{
		for (i = 0; i < NUM_STAGE; i++)
		{						/* i is stage counter */
			jj = shl(2, i);		/* FFT size */
			kk = shl(jj, 1);	/* 2 * FFT size */
			ii = ii_table[i];	/* 2 * number of FFT's */

			for (j = 0; j < jj; j = j + 2)
			{					/* j is sample counter */
				ji = j * ii;	/* ji is phase table index */

				for (k = j; k < SIZE; k = k + kk)
				{				/* k is butterfly top */
					kj = add(k, jj);	/* kj is butterfly bottom */

					/* Butterfly computations */
					ftmp_real = L_sub(L_mult(*(farray_ptr + kj), phs_tbl[ji]),
									  L_mult(*(farray_ptr + kj + 1), phs_tbl[ji + 1]));
					ftmp_imag = L_add(L_mult(*(farray_ptr + kj + 1), phs_tbl[ji]),
									  L_mult(*(farray_ptr + kj), phs_tbl[ji + 1]));

					tmp1 = round32(ftmp_real);
					tmp2 = round32(ftmp_imag);

					tmp = sub(*(farray_ptr + k), tmp1);
					*(farray_ptr + kj) = shr(tmp, 1);

					tmp = sub(*(farray_ptr + k + 1), tmp2);
					*(farray_ptr + kj + 1) = shr(tmp, 1);

					tmp = add(*(farray_ptr + k), tmp1);
					*(farray_ptr + k) = shr(tmp, 1);

					tmp = add(*(farray_ptr + k + 1), tmp2);
					*(farray_ptr + k + 1) = shr(tmp, 1);
				}
			}
		}

/* The IFFT part */
	}
	else
	{
		for (i = 0; i < NUM_STAGE; i++)
		{						/* i is stage counter */
			jj = shl(2, i);		/* FFT size */
			kk = shl(jj, 1);	/* 2 * FFT size */
			ii = ii_table[i];	/* 2 * number of FFT's */

			for (j = 0; j < jj; j = j + 2)
			{					/* j is sample counter */
				ji = j * ii;	/* ji is phase table index */

				for (k = j; k < SIZE; k = k + kk)
				{				/* k is butterfly top */
					kj = add(k, jj);	/* kj is butterfly bottom */

					/* Butterfly computations */
					ftmp_real = L_add(L_mult(*(farray_ptr + kj), phs_tbl[ji]),
									  L_mult(*(farray_ptr + kj + 1), phs_tbl[ji + 1]));
					ftmp_imag = L_sub(L_mult(*(farray_ptr + kj + 1), phs_tbl[ji]),
									  L_mult(*(farray_ptr + kj), phs_tbl[ji + 1]));

					tmp1 = round32(ftmp_real);
					tmp2 = round32(ftmp_imag);

					*(farray_ptr + kj) = sub(*(farray_ptr + k), tmp1);
					*(farray_ptr + kj + 1) = sub(*(farray_ptr + k + 1), tmp2);
					*(farray_ptr + k) = add(*(farray_ptr + k), tmp1);
					*(farray_ptr + k + 1) = add(*(farray_ptr + k + 1), tmp2);
				}
			}
		}
	}

}								/* end of c_fft () */
#endif

void r_fft(Shortword * farray_ptr, Shortword isign)
{

	Shortword ftmp1_real, ftmp1_imag, ftmp2_real, ftmp2_imag;
	Longword Lftmp1_real, Lftmp1_imag, Lftmp2_real, Lftmp2_imag;
	Shortword i, j;
	Longword Ltmp1, Ltmp2;

/* The FFT part */
	if (isign == 1)
	{
		/* Perform the complex FFT */
		c_fft(farray_ptr, isign);

		/* First, handle the DC and foldover frequencies */
		ftmp1_real = *farray_ptr;
		ftmp2_real = *(farray_ptr + 1);
		*farray_ptr = add(ftmp1_real, ftmp2_real);
		*(farray_ptr + 1) = sub(ftmp1_real, ftmp2_real);

		/* Now, handle the remaining positive frequencies */
		for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i)
		{
			ftmp1_real = add(*(farray_ptr + i), *(farray_ptr + j));
			ftmp1_imag = sub(*(farray_ptr + i + 1), *(farray_ptr + j + 1));
			ftmp2_real = add(*(farray_ptr + i + 1), *(farray_ptr + j + 1));
			ftmp2_imag = sub(*(farray_ptr + j), *(farray_ptr + i));

			Lftmp1_real = L_deposit_h(ftmp1_real);
			Lftmp1_imag = L_deposit_h(ftmp1_imag);
			Lftmp2_real = L_deposit_h(ftmp2_real);
			Lftmp2_imag = L_deposit_h(ftmp2_imag);

			Ltmp1 = L_sub(L_mult(ftmp2_real, phs_tbl[i]), L_mult(ftmp2_imag, phs_tbl[i + 1]));
			*(farray_ptr + i) = round32(L_shr(L_add(Lftmp1_real, Ltmp1), 1));

			Ltmp1 = L_add(L_mult(ftmp2_imag, phs_tbl[i]), L_mult(ftmp2_real, phs_tbl[i + 1]));
			*(farray_ptr + i + 1) = round32(L_shr(L_add(Lftmp1_imag, Ltmp1), 1));

			Ltmp1 = L_add(L_mult(ftmp2_real, phs_tbl[j]), L_mult(ftmp2_imag, phs_tbl[j + 1]));
			*(farray_ptr + j) = round32(L_shr(L_add(Lftmp1_real, Ltmp1), 1));

			Ltmp1 = L_add(L_negate(L_mult(ftmp2_imag, phs_tbl[j])), L_mult(ftmp2_real, phs_tbl[j + 1]));
			Ltmp2 = L_add(L_negate(Lftmp1_imag), Ltmp1);
			*(farray_ptr + j + 1) = round32(L_shr(Ltmp2, 1));
		}

	}
	else
	{

		/* First, handle the DC and foldover frequencies */
		ftmp1_real = *farray_ptr;
		ftmp2_real = *(farray_ptr + 1);
		*farray_ptr = shr(add(ftmp1_real, ftmp2_real), 1);
		*(farray_ptr + 1) = shr(sub(ftmp1_real, ftmp2_real), 1);

		/* Now, handle the remaining positive frequencies */
		for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i)
		{
			ftmp1_real = add(*(farray_ptr + i), *(farray_ptr + j));
			ftmp1_imag = sub(*(farray_ptr + i + 1), *(farray_ptr + j + 1));
			ftmp2_real = negate(add(*(farray_ptr + j + 1), *(farray_ptr + i + 1)));
			ftmp2_imag = negate(sub(*(farray_ptr + j), *(farray_ptr + i)));

			Lftmp1_real = L_deposit_h(ftmp1_real);
			Lftmp1_imag = L_deposit_h(ftmp1_imag);
			Lftmp2_real = L_deposit_h(ftmp2_real);
			Lftmp2_imag = L_deposit_h(ftmp2_imag);

			Ltmp1 = L_add(L_mult(ftmp2_real, phs_tbl[i]), L_mult(ftmp2_imag, phs_tbl[i + 1]));
			*(farray_ptr + i) = round32(L_shr(L_add(Lftmp1_real, Ltmp1), 1));

			Ltmp1 = L_sub(L_mult(ftmp2_imag, phs_tbl[i]), L_mult(ftmp2_real, phs_tbl[i + 1]));
			*(farray_ptr + i + 1) = round32(L_shr(L_add(Lftmp1_imag, Ltmp1), 1));

			Ltmp1 = L_sub(L_mult(ftmp2_real, phs_tbl[j]), L_mult(ftmp2_imag, phs_tbl[j + 1]));
			*(farray_ptr + j) = round32(L_shr(L_add(Lftmp1_real, Ltmp1), 1));

			Ltmp1 = L_negate(L_add(L_mult(ftmp2_imag, phs_tbl[j]), L_mult(ftmp2_real, phs_tbl[j + 1])));
			Ltmp2 = L_add(L_negate(Lftmp1_imag), Ltmp1);
			*(farray_ptr + j + 1) = round32(L_shr(Ltmp2, 1));
		}

		/* Perform the complex IFFT */
		c_fft(farray_ptr, isign);

	}

}								/* end r_fft () */
