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



static char const rcsid[] =
    "$Id: preproc.cc,v 1.19.12.5 2007/06/24 06:43:16 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     bqiir.c                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     9/27/95  Born                                                    */
/*----------------------------------------------------------------------*/
/*         ..Complexity (not including main() buffers).                 */
/*           total data ROM        84 bytes                             */
/*           total static RAM     144 bytes                             */
/*           total dynamic RAM    736 bytes                             */
/*----------------------------------------------------------------------*/
/*======================================================================*/
/*         ..Include files.                                             */
/*----------------------------------------------------------------------*/
#include  <stdio.h>
#include  <stdlib.h>
#include  <math.h>
#include "struct.h"

/*======================================================================*/
/*        ..Typedefs.                                                   */
/*----------------------------------------------------------------------*/
typedef long int32;
typedef short int16;

/*======================================================================*/
/*        ..Defines.                                                    */
/*             data ROM       12 bytes                                  */
/*----------------------------------------------------------------------*/

#define  BQ_N_DATA     160
#define  BQ_N_FILTERS    3
#define  BQ_N_ORDER      2
#define  BQ_N_SAVE       (BQ_N_FILTERS * BQ_N_ORDER)
#define  BQ_N_BUFFER     (BQ_N_SAVE + BQ_N_DATA)
#define  BQ_N_W          (2 * BQ_N_FILTERS * (BQ_N_ORDER + 1))

/*======================================================================*/
/*        ..Globals.                                                    */
/*             data ROM       72 bytes                                  */
/*----------------------------------------------------------------------*/
float bq_w[BQ_N_W] = {
    /*   n2           n1            n0          d2             d1           d0 */
    1.000125737, -2.000125721, 1.000000000, 0.952444269, -1.943779252,
	1.000000000,
    0.999873585, -1.999873569, 1.000000000, 0.875214548, -1.866892280,
	1.000000000,
    0.833470028, -1.666939491, 0.833469450, 0.833345838, -1.825209384,
	1.000000000
};

/*======================================================================*/
/*        ..Floating-point vector copy.                                 */
/*----------------------------------------------------------------------*/
void
V_copy (float *a, float *b, int32 n)
{
    /*....execute.... */
    while (n--)
	*b++ = *a++;
}

/*======================================================================*/
/*        ..Three-stage bi-quad hi-pass IIR filter.                     */
/*          This function processes 160 point (20 ms) blocks).          */
/*             static RAM     144 bytes                                 */
/*             dynamic RAM    736 bytes                                 */
/*----------------------------------------------------------------------*/
void
bqiir (float *buf)
{
    /*....(static) variables.... */
    static float bq_xsave[BQ_N_FILTERS * BQ_N_SAVE];
    static float bq_ysave[BQ_N_FILTERS * BQ_N_SAVE];

    /*....(local) variables.... */
    register int16 i;
    register int16 j;
    register int16 k;
    register int16 fi;
    register float *tmpP;
    register float *wP;
    register float *wpP;
    register float *xP;
    register float *xpP;
    register float *xsP;
    register float *yP;
    register float *ypP;
    register float *ysP;
    register float sum;
    float bq_x[BQ_N_BUFFER];
    float bq_y[BQ_N_BUFFER];

    /*....execute.... */
    V_copy (buf, bq_x + BQ_N_SAVE, BQ_N_DATA);

    for (xsP = bq_xsave, ysP = bq_ysave, xP = bq_x, yP =
	 bq_y + BQ_N_ORDER, wP = bq_w, fi = 0, i = BQ_N_FILTERS - 1; i >= 0;
	 i--, xP += BQ_N_ORDER, yP += BQ_N_ORDER, wP +=
	 (2 * (BQ_N_ORDER + 1)), fi++, xsP += BQ_N_SAVE, ysP += BQ_N_SAVE) {

	V_copy (xsP, bq_x, BQ_N_SAVE);
	V_copy (ysP, bq_y, BQ_N_SAVE);

	for (xpP = xP, ypP = yP, j = 0; j < BQ_N_DATA + i * BQ_N_ORDER;
	     j++, xpP++, ypP++) {
	    sum = 0.0;
	    for (tmpP = xpP, wpP = wP, k = 0; k < BQ_N_ORDER + 1;
		 k++, wpP++, tmpP++) {
		sum += *wpP * *tmpP;
	    }
	    for (tmpP = ypP - BQ_N_ORDER, k = 0; k < BQ_N_ORDER;
		 k++, wpP++, tmpP++) {
		sum -= *wpP * *tmpP;
	    }
	    *ypP = sum;
	}
	V_copy (bq_x + BQ_N_BUFFER - BQ_N_SAVE, xsP, BQ_N_SAVE);
	V_copy (bq_y + BQ_N_BUFFER - BQ_N_SAVE, ysP, BQ_N_SAVE);

	V_copy (bq_y, bq_x, BQ_N_BUFFER);
    }
    V_copy (bq_y + BQ_N_SAVE, buf, BQ_N_DATA);
}

/* r_fft.c */

/****************************************************************/
/*								*/
/*								*/
/*		COPYRIGHT 1995, MOTOROLA INC.			*/
/*								*/
/*								*/
/****************************************************************/

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


#define			SIZE			512	//256
#define			SIZE_BY_TWO		256	//128
#define			NUM_STAGE		8	//7


//#define                       PI                      3.141592653589793

#define			TRUE			1
#define			FALSE			0


/* External variables */

static double phs_tbl[SIZE];	/* holds the complex sinusoids */
static int first = TRUE;


/* FFT/IFFT function for real sequences */
void c_fft (float *farray_ptr, int isign);
void fill_tbl ();

void
r_fft (float *farray_ptr, int isign)
{

    float ftmp1_real, ftmp1_imag, ftmp2_real, ftmp2_imag;
    int i, j;



    /* If this is the first call to the function, fill up the phase table  */

    if (first == TRUE)
	fill_tbl ();

    /* The FFT part */

    if (isign == 1) {

	/* Perform the complex FFT */

	c_fft (farray_ptr, isign);

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

	c_fft (farray_ptr, isign);

    }

    return;

}				/* end r_fft () */



/* FFT/IFFT function for complex sequences */

/* The decimation-in-time complex FFT/IFFT is implemented below.
   The input complex numbers are presented as real part followed by
   imaginary part for each sample.  The counters are therefore
   incremented by two to access the complex valued samples. */

void
c_fft (float *farray_ptr, int isign)
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
fill_tbl (void)
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


/* ns127.c */


/****************************************************************/
/*								*/
/*								*/
/*		COPYRIGHT 1995, 1996, MOTOROLA INC.    		*/
/*								*/
/*								*/
/****************************************************************/

/*****************************************************************
*
* EVRC Noise Suppression
*
* Input:  The input to the function is a float pointer to the
*         array of data to be noise suppressed.
*
* Output: There is no return value.  The input array is replaced
*         with the noise suppressed values.
*
*
* Written by:			Tenkasi V. Ramabadran
* Date:				December 28, 1994
*
* Last Modified:		James P. Ashley
* Date:				February 19, 1996
*
*
* Version    Date      Description
*
*   1.0    12/01/95    Released to TIA TR45.5.1.1
*   1.1	   02/14/96    Init Noise to first 4 frames	 
*   1.2	   02/19/96    Bug fix in frame_cnt declaration	 
*
*****************************************************************/

/* Includes */

#include		<math.h>


/* Defines */


#define			LO_CHAN			0
#define			HI_CHAN			(NUM_CHAN-1)

#define			TRUE			1
#define			FALSE			0

#define			UPDATE_THLD		((35*NUM_CHAN/16.0f)+10)
#define			METRIC_THLD		((45*NUM_CHAN/16.0f)+0)
#define			MID_CHAN		12
#define			INDEX_THLD		12
#define			SETBACK_THLD		12
#define			INDEX_CNT_THLD		20

#define			SNR_THLD		6
#define			UPDATE_CNT_THLD		50

/* use (32768.0 * 32768.0) for fractional */
#define			NORM_ENRG		(1.0)
#define			NOISE_FLOOR		(1.0 / NORM_ENRG)
#define			MIN_CHAN_ENRG		(0.0625 / NORM_ENRG)
#define			INE			MIN_CHAN_ENRG	//(16.0 / NORM_ENRG)

#define			MIN_GAIN		(-13.0)
#define			GAIN_SLOPE		0.50

#define			CNE_SM_FAC		0.1
#define			CEE_SM_FAC		0.60
#define			PRE_EMP_FAC		(-0.8)
#define			DE_EMP_FAC		0.8

/* forced update constants... */
#define			HYSTER_CNT_THLD		6
/* 50 - 10log10(NORM_ENRG) */
#define			HIGH_TCE_DB		(50.)
/* 30 - 10log10(NORM_ENRG) */
#define			LOW_TCE_DB		(30.)
#define			TCE_RANGE		(HIGH_TCE_DB - LOW_TCE_DB)
#define			HIGH_ALPHA		0.99
#define			LOW_ALPHA		0.50
#define			ALPHA_RANGE		(HIGH_ALPHA - LOW_ALPHA)
#define			DEV_THLD		(28*NUM_CHAN/16.0f)


/* Macros */

#define			min(a,b)		((a)<(b)?(a):(b))
#define			max(a,b)		((a)>(b)?(a):(b))
#define			square(a)		((a)*(a))


/* The noise supression function */

  /* Functions */

void r_fft (float *, int);
void init_window (float *, int, float);

void
FGV_MEM::noise_suprs (float *farray_ptr, float *ns_snr)
{

    /* Static variables */

    /* The channel table is defined below.  In this table, the
       lower and higher frequency coefficients for each of the 16
       channels are specified.  The table excludes the coefficients
       with numbers 0 (DC), 1, and 64 (Foldover frequency).  For
       these coefficients, the gain is always set at 1.0 (0 dB). */


    static const int ch_tbl[NUM_CHAN][2] = {

	{1, 1},			// index  0:   31 -   31 Hz
	{2, 2},			// index  1:   62 -   62 Hz
	{3, 3},			// index  2:   94 -   94 Hz
	{4, 4},			// index  3:  125 -  125 Hz
	{5, 5},			// index  4:  156 -  156 Hz
	{6, 6},			// index  5:  188 -  188 Hz
	{7, 8},			// index  6:  219 -  250 Hz
	{9, 10},		// index  7:  281 -  312 Hz
	{11, 12},		// index  8:  344 -  375 Hz
	{13, 14},		// index  9:  406 -  438 Hz
	{15, 16},		// index 10:  469 -  500 Hz
	{17, 18},		// index 11:  531 -  562 Hz
	{19, 20},		// index 12:  594 -  625 Hz
	{21, 22},		// index 13:  656 -  688 Hz
	{23, 24},		// index 14:  719 -  750 Hz
	{25, 26},		// index 15:  781 -  812 Hz
	{27, 28},		// index 16:  844 -  875 Hz
	{29, 30},		// index 17:  906 -  938 Hz
	{31, 32},		// index 18:  969 - 1000 Hz
	{33, 34},		// index 19: 1031 - 1062 Hz
	{35, 36},		// index 20: 1094 - 1125 Hz
	{37, 38},		// index 21: 1156 - 1188 Hz
	{39, 40},		// index 22: 1219 - 1250 Hz
	{41, 42},		// index 23: 1281 - 1312 Hz
	{43, 44},		// index 24: 1344 - 1375 Hz
	{45, 46},		// index 25: 1406 - 1438 Hz
	{47, 48},		// index 26: 1469 - 1500 Hz
	{49, 50},		// index 27: 1531 - 1562 Hz
	{51, 52},		// index 28: 1594 - 1625 Hz
	{53, 54},		// index 29: 1656 - 1688 Hz
	{55, 56},		// index 30: 1719 - 1750 Hz
	{57, 58},		// index 31: 1781 - 1812 Hz
	{59, 61},		// index 32: 1844 - 1906 Hz
	{62, 64},		// index 33: 1938 - 2000 Hz
	{65, 67},		// index 34: 2031 - 2094 Hz
	{68, 70},		// index 35: 2125 - 2188 Hz
	{71, 73},		// index 36: 2219 - 2281 Hz
	{74, 76},		// index 37: 2312 - 2375 Hz
	{77, 79},		// index 38: 2406 - 2469 Hz
	{80, 83},		// index 39: 2500 - 2594 Hz
	{84, 87},		// index 40: 2625 - 2719 Hz
	{88, 91},		// index 41: 2750 - 2844 Hz
	{92, 95},		// index 42: 2875 - 2969 Hz
	{96, 99},		// index 43: 3000 - 3094 Hz
	{100, 104},		// index 44: 3125 - 3250 Hz
	{105, 109},		// index 45: 3281 - 3406 Hz
	{110, 114},		// index 46: 3438 - 3562 Hz
	{115, 120},		// index 47: 3594 - 3750 Hz
	{121, 126},		// index 48: 3781 - 3938 Hz
	{127, 132},		// index 49: 3969 - 4125 Hz
	{133, 140},		// index 50: 4156 - 4375 Hz
	{141, 148},		// index 51: 4406 - 4625 Hz
	{149, 156},		// index 52: 4656 - 4875 Hz
	{157, 165},		// index 53: 4906 - 5156 Hz
	{166, 175},		// index 54: 5188 - 5469 Hz
	{176, 185},		// index 55: 5500 - 5781 Hz
	{186, 196},		// index 56: 5812 - 6125 Hz
	{197, 208},		// index 57: 6156 - 6500 Hz
	{209, 220},		// index 58: 6531 - 6875 Hz
	{221, 234},		// index 59: 6906 - 7312 Hz
	{235, 248},		// index 60: 7344 - 7750 Hz

    };




    /* The voice metric table is defined below.  It is a non-
       linear table with a deadband near zero.  It maps the SNR
       index (quantized SNR value) to a number that is a measure
       of voice quality. */

    static const int vm_tbl[90] = { 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
	3, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 7, 7, 7,
	8, 8, 9, 9, 10, 10, 11, 12, 12, 13, 13, 14, 15,
	15, 16, 17, 17, 18, 19, 20, 20, 21, 22, 23, 24,
	24, 25, 26, 27, 28, 28, 29, 30, 31, 32, 33, 34,
	35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 45,
	46, 47, 48, 49, 50, 50, 50, 50, 50, 50, 50, 50,
	50, 50
    };



    /* Automatic variables */

    float data_buffer[FFT_LEN], enrg, snr;
    float tne, tce, gain, ftmp1, ftmp2;
    int ch_snr[NUM_CHAN];
    int i, j, j1, j2;
    int vm_sum;
    int update_flag, modify_flag, index_cnt;

    float ch_enrg_dev;		/* for forced update... */
    float ch_enrg_db[NUM_CHAN];
    float alpha;

    float ch_snr_unq[NUM_CHAN];

    float gain_tmp[NUM_CHAN];



    /* Init the window function, channel gains one time */

    if (noise_suprs_first == TRUE) {

	init_window (window, DELAY + FRM_LEN,
		     (float) DELAY / (DELAY + FRM_LEN));

	ch_gain[0] = (ch_gain[1] = 1.0);

    }


    /* Increment frame counter */
    frame_cnt++;

    /* Preemphasize the input data and store in the data buffer with
       appropriate delay */

    for (i = 0; i < DELAY; i++)
	data_buffer[i] = window_overlap[i];

    data_buffer[DELAY] = *farray_ptr + PRE_EMP_FAC * pre_emp_mem;

    for (i = DELAY + 1, j = 1; i < DELAY + FRM_LEN; i++, j++)
	data_buffer[i] =
	    *(farray_ptr + j) + PRE_EMP_FAC * *(farray_ptr + j - 1);

    pre_emp_mem = *(farray_ptr + FRM_LEN - 1);

    for (i = DELAY + FRM_LEN; i < FFT_LEN; i++)
	data_buffer[i] = 0.0;


    /* update window_overlap buffer */

    for (i = 0, j = FRM_LEN; i < DELAY; i++, j++)
	window_overlap[i] = data_buffer[j];


    /* Apply window to frame prior to FFT */

    for (i = 0; i < FRM_LEN + DELAY; i++)
	data_buffer[i] *= window[i];


    /* Perform FFT on the data buffer */

    r_fft (data_buffer, +1);


    /* Estimate the energy in each channel */

    alpha = (noise_suprs_first == TRUE) ? 1.0 : CEE_SM_FAC;

    for (i = LO_CHAN; i <= HI_CHAN; i++) {

	enrg = 0.0;
	j1 = ch_tbl[i][0], j2 = ch_tbl[i][1];
	for (j = j1; j <= j2; j++)
	    enrg +=
		square (data_buffer[2 * j]) + square (data_buffer[2 * j + 1]);
	enrg /= (float) (j2 - j1 + 1);
	ch_enrg[i] = (1 - alpha) * ch_enrg[i] + alpha * enrg;
	if (ch_enrg[i] < MIN_CHAN_ENRG)
	    ch_enrg[i] = MIN_CHAN_ENRG;

    }


    /*Initialize channel noise estimate to channel energy of first four frames */

    if (frame_cnt <= 16)
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	    ch_noise[i] = 0.5 * ch_noise[i] + 0.5 * max (ch_enrg[i], INE);

    /* Compute the channel SNR indices */

    for (i = LO_CHAN; i <= HI_CHAN; i++) {

	snr = 10.0 * log10 ((double) (ch_enrg[i] / ch_noise[i]));

	ch_snr_unq[i] = snr;

	if (snr < 0.0)
	    snr = 0.0;
	ch_snr[i] = (int) ((snr + 0.1875) / 0.375);


    }


    /* Compute the sum of voice metrics */

    vm_sum = 0;
    for (i = LO_CHAN; i <= HI_CHAN; i++) {

	j = min (ch_snr[i], 89);
	vm_sum += vm_tbl[j];

    }


    /* Compute the total noise estimate (tne)
       and total channel energy estimate (tce) */

    tne = tce = 0.0;

    for (i = LO_CHAN; i <= HI_CHAN; i++) {

	tne += ch_noise[i];
	tce += ch_enrg[i];

    }

    /* calculate SNR for mode decision */
    float band_eng, band_noise;



    band_eng = 0;
    band_noise = 0;
    for (i = LO_CHAN; i < 33; i++) {
	band_eng += ch_enrg[i];
	band_noise += ch_noise[i];
    }
    ns_snr[0] = max (0, 10 * log10 (band_eng / band_noise));

    band_eng = 0;
    band_noise = 0;
    for (i = 33; i < 50; i++) {
	band_eng += ch_enrg[i];
	band_noise += ch_noise[i];
    }
    ns_snr[1] = max (0, 10 * log10 (band_eng / band_noise));


    /* Calculate log spectral deviation */

    for (i = LO_CHAN; i <= HI_CHAN; i++)
	ch_enrg_db[i] = 10. * log10 (ch_enrg[i]);

    if (noise_suprs_first == TRUE)
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	    ch_enrg_long_db[i] = ch_enrg_db[i];

    ch_enrg_dev = 0.;
    for (i = LO_CHAN; i <= HI_CHAN; i++)
	ch_enrg_dev += fabs (ch_enrg_long_db[i] - ch_enrg_db[i]);


    /* Calculate long term integration constant as a function of
       total channel energy (tce) */
    /* (i.e., high tce (-40 dB) -> slow integration (alpha = 0.99),
       low tce (-60 dB) -> fast integration (alpha = 0.50) */

    alpha =
	HIGH_ALPHA - (ALPHA_RANGE / TCE_RANGE) * (HIGH_TCE_DB -
						  10. * log10 (tce));
    if (alpha > HIGH_ALPHA)
	alpha = HIGH_ALPHA;
    else if (alpha < LOW_ALPHA)
	alpha = LOW_ALPHA;


    /* Calc long term log spectral energy */

    for (i = LO_CHAN; i <= HI_CHAN; i++)
	ch_enrg_long_db[i] =
	    alpha * ch_enrg_long_db[i] + (1. - alpha) * ch_enrg_db[i];


    /* Set or reset the update flag */

    update_flag = FALSE;

    if (vm_sum <= UPDATE_THLD) {

	update_flag = TRUE;
	update_cnt = 0;

    }

    else if (tce > NOISE_FLOOR && ch_enrg_dev < DEV_THLD) {

	if (beta < 0.4) {
	    update_cnt++;
	}

	if (update_cnt >= UPDATE_CNT_THLD) {
	    update_flag = TRUE;
	    //fprintf(stderr,"FORCED UPDATE, NS FRAME %d\n",frame_cnt);
	}
    }

    if (update_cnt == last_update_cnt)
	hyster_cnt++;
    else
	hyster_cnt = 0;
    last_update_cnt = update_cnt;

    if (hyster_cnt > HYSTER_CNT_THLD)
	update_cnt = 0;




    /* Set or reset modify flag */

    index_cnt = 0;

    for (i = MID_CHAN; i <= HI_CHAN; i++)
	if (ch_snr[i] >= INDEX_THLD)
	    index_cnt++;

    modify_flag = (index_cnt < INDEX_CNT_THLD) ? TRUE : FALSE;


    /* Modify the SNR indices */

    if (modify_flag == TRUE)
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	    if ((vm_sum <= METRIC_THLD) || (ch_snr[i] <= SETBACK_THLD)) {
		ch_snr[i] = 1;

		ch_snr_unq[i] = 0.0;

	    }






    float SNR, SNRmax;
    float s0, s1;
    int SNRnegcnt = 0;;
    s1 = 0.0;

    SNRmax = -100.0;

#define LOW_CHAN_CUT 5
    for (i = LOW_CHAN_CUT; i <= HI_CHAN; i++) {
	s0 = 10 * log10 (ch_enrg[i] / ch_noise[i]);
	s1 += s0;
	if (s0 > SNRmax)
	    SNRmax = s0;
	if (s0 < 0)
	    SNRnegcnt++;
    }
    SNR = s1 * (1.0 / (HI_CHAN - LOW_CHAN_CUT + 1));

    /* Prevent update of noise estimate (too close to speech) during low SNR
       by implementing hangover */

    if (frame_cnt <= 4) {
	ns_snr_previous = 10;
    }
    else {
	ns_snr_previous =
	    (SNR >
	     ns_snr_previous) ? ns_snr_previous + 0.2 : ((SNR > 0.25 * ns_snr_previous) ? ns_snr_previous - 0.01 : ns_snr_previous);
    }

# define NEG_SNR_VAR_THRESH 0.75
    if (frame_cnt <= 4) {
	ns_neg_snr_mean = -NEG_SNR_VAR_THRESH;
	ns_neg_snr_var = 0.5 * NEG_SNR_VAR_THRESH;
	ns_neg_snr_bias = 0.0;
    }
    else if (SNR < 0 && SNRnegcnt < (int) (0.75 * NUM_CHAN)) {
	ns_neg_snr_var = 0.99 * ns_neg_snr_var + 0.01 * SNR * SNR;
	ns_neg_snr_bias =
	    min (16.0, max (0.0, 40.0 * (ns_neg_snr_var - NEG_SNR_VAR_THRESH)));
    }


    ns_snr_threshold = 6.0 + ns_neg_snr_bias;


    /* Compute the channel gains */

    ftmp1 = 10.0 * log10 ((double) (NOISE_FLOOR / tne));
    ftmp1 = max (ftmp1, MIN_GAIN);

    for (i = LO_CHAN; i <= HI_CHAN; i++) {


	if (ch_snr_unq[i] <= ns_snr_threshold * (3.0 / 8.0))
	    ch_snr_unq[i] = ns_snr_threshold * (3.0 / 8.0);

	gain =
	    (ch_snr_unq[i] -
	     ns_snr_threshold * (3.0 / 8.0)) * GAIN_SLOPE * (8.0 / 3.0) + ftmp1;

	gain = min (0.0, gain);

	gain =
	    (1 - sin (PI / 2 * (ftmp1 / 2 - gain) / (ftmp1 / 2))) * ftmp1 / 2;

	ftmp2 = pow (10.0, (double) (gain / 20.0));

	gain_tmp[i] = ftmp2;

	j1 = ch_tbl[i][0], j2 = ch_tbl[i][1];

	for (j = j1; j <= j2; j++)
	    ch_gain[j] = ftmp2;

    }


    /* Update the channel noise estimates */

    if (update_flag == TRUE)
	for (i = LO_CHAN; i <= HI_CHAN; i++) {

	    ch_noise[i] =
		(1.0 - CNE_SM_FAC) * ch_noise[i] + CNE_SM_FAC * ch_enrg[i];

	    if (ch_noise[i] < MIN_CHAN_ENRG)
		ch_noise[i] = MIN_CHAN_ENRG;

	}


    /* Fill gaps in spectrum (near DC and fs/2) */
    for (j = 1; j < ch_tbl[LO_CHAN][0]; j++) {
	ch_gain[j] = ch_gain[ch_tbl[LO_CHAN][0]];
    }
    for (j = ch_tbl[HI_CHAN][1] + 1; j < FFT_LEN / 2; j++) {
	ch_gain[j] = ch_gain[ch_tbl[HI_CHAN][1]];
    }

    /* Filter the input data in the frequency domain and perform IFFT */
    data_buffer[0] *= ch_gain[1];	// DC
    data_buffer[1] *= ch_gain[FFT_LEN / 2 - 1];	// Fs/2
    for (i = 1; i < FFT_LEN / 2; i++) {
	data_buffer[2 * i] *= ch_gain[i];
	data_buffer[2 * i + 1] *= ch_gain[i];
    }

    r_fft (data_buffer, -1);


    /* Overlap add the filtered data from previous block.
       Save data from this block for the next. */

    for (i = 0; i < FRM_LEN; i++)
	data_buffer[i] *= window[i];

    for (i = 0; i < DELAY; i++)
	data_buffer[i] += overlap[i];

    for (i = FRM_LEN; i < FRM_LEN + DELAY; i++)
	overlap[i - FRM_LEN] = data_buffer[i] * window[i];


    /* Deemphasize the filtered speech and write it out to farray */

    *farray_ptr = data_buffer[0] + DE_EMP_FAC * de_emp_mem;

    for (i = 1; i < FRM_LEN; i++)
	*(farray_ptr + i) =
	    data_buffer[i] + DE_EMP_FAC * *(farray_ptr + i - 1);

    de_emp_mem = *(farray_ptr + FRM_LEN - 1);



    /* Combine input overlap with output overlap to create 
       a temporary signal for input to LPC analysis lookahead */

    /* Do zero delay processing only on 2nd 10 ms subframe */
    if ((++ns_subframe_count % 2) == 0) {

	float ext1[DELAY];
	float ext2[DELAY];

	/* Find the max channel gain */
	float gmax = gain_tmp[0];

	for (i = LO_CHAN + 1; i <= HI_CHAN; i++) {
	    if (gain_tmp[i] > gmax) {
		gmax = gain_tmp[i];
	    }
	}

	/* Attenuate the input overlap by the max gain, window,
	   & add to the output windowed data */
	for (i = 0; i < DELAY; i++) {

	    ext1[i] =
		gmax * window_overlap[i] * window[i] * window[i] + overlap[i];
	}

	/* Deemphasize and save in extension buffer for later LPC processing */
	ext2[0] = ext1[0] + DE_EMP_FAC * de_emp_mem;
	for (i = 1; i < DELAY; i++) {
	    ext2[i] = ext1[i] + DE_EMP_FAC * ext2[i - 1];
	}

	/* Transfer to the time shifted output buffer (could be done above) */
	for (i = 0; i < DELAY; i++) {
	    farray_ptr[FRM_LEN + i] = ext2[i];
	}



    }

    noise_suprs_first = FALSE;

}				/* end noise_suprs () */

/***************************************************************************/

void
init_window (float *x, int n, float ovlap)
{
    int i;
    float arg;
    int n1;

    /* use smoothed trapezoidal window */

    n1 = (int) (ovlap * n);
    arg = 2. * atan (1.) / n1;

    if (1) {
	for (i = 0; i < n1; i++)
	    x[i] = sin ((i + 0.5) * arg);
	for (i = n1; i < n - n1; i++)
	    x[i] = 1.;
	for (i = n - n1; i < n; i++)
	    x[i] = x[n - 1 - i];
    }
    else {
	for (i = 0; i < n1; i++)
	    x[i] = pow (sin ((i + 0.5) * arg), 2.);
	for (i = n1; i < n - n1; i++)
	    x[i] = 1.;
	for (i = n - n1; i < n; i++)
	    x[i] = pow (sin (((i - n) + 0.5) * arg), 2.);

    }

    return;

}				/* end of init_window() */

/***************************************************************************/
