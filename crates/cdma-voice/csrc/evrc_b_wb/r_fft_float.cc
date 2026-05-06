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
/*     Enhanced Variable Rate Codec - Master C code Specification       */
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
*	1) A float pointer to the input data array 
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
*
* Written by:			Tenkasi V. Ramabadran
* Date:				December 20, 1994
*
*
* Version			Description
*
*   1.0				 Original
*
*****************************************************************/

/* Includes */

#include		<math.h>


/* Defines */

#define			SIZE			128
#define			SIZE_BY_TWO		64
#define			NUM_STAGE		6

#define			PI			3.141592653589793

#define			TRUE			1
#define			FALSE			0


/* External variables */

static double phs_tbl[SIZE];	/* holds the complex sinusoids */
static int first = TRUE;

void c_fft_flt (float *farray_ptr, int isign);
void fill_tbl_flt ();

/* FFT/IFFT function for real sequences */

void
r_fft_flt (float *farray_ptr, int isign)
{

    float ftmp1_real, ftmp1_imag, ftmp2_real, ftmp2_imag;
    int i, j;

//void          c_fft_flt (), fill_tbl_flt ();


/* If this is the first call to the function, fill up the
   phase table  */

    if (first == TRUE)
	fill_tbl_flt ();

/* The FFT part */

    if (isign == 1) {

/* Perform the complex FFT */

	c_fft_flt (farray_ptr, isign);

/* First, handle the DC and foldover frequencies */

	ftmp1_real = *farray_ptr;
	ftmp2_real = *(farray_ptr + 1);
	*farray_ptr = ftmp1_real + ftmp2_real;
	*(farray_ptr + 1) = ftmp1_real - ftmp2_real;

/* Now, handle the remaining positive frequencies */

	for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i) {

	    ftmp1_real = *(farray_ptr + i) + *(farray_ptr + j);
	    ftmp1_imag = *(farray_ptr + i + 1) - *(farray_ptr + j + 1);
	    ftmp2_real = *(farray_ptr + i + 1) + *(farray_ptr + j + 1);
	    ftmp2_imag = *(farray_ptr + j) - *(farray_ptr + i);

	    *(farray_ptr + i) = (ftmp1_real + phs_tbl[i] * ftmp2_real -
				 phs_tbl[i + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + i + 1) = (ftmp1_imag + phs_tbl[i] * ftmp2_imag +
				     phs_tbl[i + 1] * ftmp2_real) / 2.0;
	    *(farray_ptr + j) = (ftmp1_real + phs_tbl[j] * ftmp2_real +
				 phs_tbl[j + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + j + 1) = (-ftmp1_imag - phs_tbl[j] * ftmp2_imag +
				     phs_tbl[j + 1] * ftmp2_real) / 2.0;

	}

    }

/* The IFFT part */

    else {

/* First, handle the DC and foldover frequencies */

	ftmp1_real = *farray_ptr;
	ftmp2_real = *(farray_ptr + 1);
	*farray_ptr = (ftmp1_real + ftmp2_real) / 2.0;
	*(farray_ptr + 1) = (ftmp1_real - ftmp2_real) / 2.0;

/* Now, handle the remaining positive frequencies */

	for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i) {

	    ftmp1_real = *(farray_ptr + i) + *(farray_ptr + j);
	    ftmp1_imag = *(farray_ptr + i + 1) - *(farray_ptr + j + 1);
	    ftmp2_real = -(*(farray_ptr + i + 1) + *(farray_ptr + j + 1));
	    ftmp2_imag = -(*(farray_ptr + j) - *(farray_ptr + i));

	    *(farray_ptr + i) = (ftmp1_real + phs_tbl[i] * ftmp2_real +
				 phs_tbl[i + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + i + 1) = (ftmp1_imag + phs_tbl[i] * ftmp2_imag -
				     phs_tbl[i + 1] * ftmp2_real) / 2.0;
	    *(farray_ptr + j) = (ftmp1_real + phs_tbl[j] * ftmp2_real -
				 phs_tbl[j + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + j + 1) = (-ftmp1_imag - phs_tbl[j] * ftmp2_imag -
				     phs_tbl[j + 1] * ftmp2_real) / 2.0;

	}

/* Perform the complex IFFT */

	c_fft_flt (farray_ptr, isign);

    }

    return;

}				/* end r_fft () */



/* FFT/IFFT function for complex sequences */

/* The decimation-in-time complex FFT/IFFT is implemented below.
The input complex numbers are presented as real part followed by
imaginary part for each sample.  The counters are therefore
incremented by two to access the complex valued samples. */

void
c_fft_flt (float *farray_ptr, int isign)
{

    int i, j, k, ii, jj, kk, ji, kj;
    float ftmp, ftmp_real, ftmp_imag;


/* Rearrange the input array in bit reversed order */

    for (i = 0, j = 0; i < SIZE - 2; i = i + 2) {

	if (j > i) {

	    ftmp = *(farray_ptr + i);
	    *(farray_ptr + i) = *(farray_ptr + j);
	    *(farray_ptr + j) = ftmp;

	    ftmp = *(farray_ptr + i + 1);
	    *(farray_ptr + i + 1) = *(farray_ptr + j + 1);
	    *(farray_ptr + j + 1) = ftmp;

	}

	k = SIZE_BY_TWO;

	while (j >= k) {
	    j -= k;
	    k >>= 1;
	}

	j += k;

    }


/* The FFT part */

    if (isign == 1) {

	for (i = 0; i < NUM_STAGE; i++) {	/* i is stage counter */

	    jj = (2 << i);	/* FFT size */
	    kk = (jj << 1);	/* 2 * FFT size */
	    ii = SIZE / jj;	/* 2 * number of FFT's */

	    for (j = 0; j < jj; j = j + 2) {	/* j is sample counter */

		ji = j * ii;	/* ji is phase table index */

		for (k = j; k < SIZE; k = k + kk) {	/* k is butterfly top */

		    kj = k + jj;	/* kj is butterfly bottom */

/* Butterfly computations */

		    ftmp_real = *(farray_ptr + kj) * phs_tbl[ji] -
			*(farray_ptr + kj + 1) * phs_tbl[ji + 1];

		    ftmp_imag = *(farray_ptr + kj + 1) * phs_tbl[ji] +
			*(farray_ptr + kj) * phs_tbl[ji + 1];

		    *(farray_ptr + kj) =
			(*(farray_ptr + k) - ftmp_real) / 2.0;
		    *(farray_ptr + kj + 1) =
			(*(farray_ptr + k + 1) - ftmp_imag) / 2.0;

		    *(farray_ptr + k) = (*(farray_ptr + k) + ftmp_real) / 2.0;
		    *(farray_ptr + k + 1) =
			(*(farray_ptr + k + 1) + ftmp_imag) / 2.0;

		}

	    }

	}

    }

/* The IFFT part */

    else {

	for (i = 0; i < NUM_STAGE; i++) {	/* i is stage counter */

	    jj = (2 << i);	/* FFT size */
	    kk = (jj << 1);	/* 2 * FFT size */
	    ii = SIZE / jj;	/* 2 * number of FFT's */

	    for (j = 0; j < jj; j = j + 2) {	/* j is sample counter */

		ji = j * ii;	/* ji is phase table index */

		for (k = j; k < SIZE; k = k + kk) {	/* k is butterfly top */

		    kj = k + jj;	/* kj is butterfly bottom */

/* Butterfly computations */

		    ftmp_real = *(farray_ptr + kj) * phs_tbl[ji] +
			*(farray_ptr + kj + 1) * phs_tbl[ji + 1];

		    ftmp_imag = *(farray_ptr + kj + 1) * phs_tbl[ji] -
			*(farray_ptr + kj) * phs_tbl[ji + 1];

		    *(farray_ptr + kj) = *(farray_ptr + k) - ftmp_real;
		    *(farray_ptr + kj + 1) =
			*(farray_ptr + k + 1) - ftmp_imag;

		    *(farray_ptr + k) = *(farray_ptr + k) + ftmp_real;
		    *(farray_ptr + k + 1) = *(farray_ptr + k + 1) + ftmp_imag;

		}

	    }

	}

    }

    return;

}				/* end of c_fft () */



/* Function to fill the phase table values */

void
fill_tbl_flt (void)
{

    int i;
    double delta_f, theta;

    delta_f = -PI / (double) SIZE_BY_TWO;

    for (i = 0; i < SIZE_BY_TWO; i++) {

	theta = delta_f * (double) i;
	phs_tbl[2 * i] = cos (theta);
	phs_tbl[2 * i + 1] = sin (theta);

    }

    first = FALSE;
    return;

}				/* end fill_tbl () */
