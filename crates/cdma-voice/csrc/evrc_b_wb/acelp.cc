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
    "$Id: acelp.cc,v 1.17.6.5 2007/06/24 06:43:03 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

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

#include "macro.h"
#include "acelp.h"
#define L_SUBFR  54
#define L_SUBFR2 55
#define L_CODE    55
#include "struct.h"

/*Note: Vector xn[], res2[], code[], y2[] and h1[] should be of lenght 55 !!! */

/*---------------------------------------------------------------------------*
 *  Function  ACELP_code()                                                   *
 *  ~~~~~~~~~~~~~~~~~~~~~~                                                   *
 *   Find Algebraic codebook.                                                *
 *--------------------------------------------------------------------------*/

void
FGV_MEM::ACELP_Code (float xn[],	/* (i)     :Target signal for codebook.       */
		     float res2[],	/* (i)     :Residual after pitch contribution */
		     float h1[],	/* (i)     :Impulse response of filters.      */
		     int T0,	/* (i)     :Pitch lag.                        */
		     float pitch_gain,	/* (i)     :Pitch gain.                       */
		     int l_subfr,	/* (i)     :Subframe lenght.                  */
		     float code[],	/* (o)     :Innovative vector.                */
		     float *gain_code,	/* (o)     :Innovative vector gain.           */
		     float y2[],	/* (o)     :Filtered innovative vector.       */
		     int *index,	/* (o)     :Index of codebook + signs         */
		     int choice	/* (i)     :Choice of innovative codebook     */
		     /*          0 -> 10 bits                      */
		     /*          1 -> 35 bits                      */
    )
{
    int i;
    float pit_sharp;
    static float dn[L_SUBFR2];

  /*-----------------------------------------------------------------*
   * - Find pitch sharpening.                                        *
   *-----------------------------------------------------------------*/

    pit_sharp = pitch_gain;

    if (pit_sharp > 0.9)
	pit_sharp = 0.9;
    if (pit_sharp < 0.2)
	pit_sharp = 0.2;


  /*-----------------------------------------------------------------*
   * - Include fixed-gain pitch contribution into impulse resp. h1[] *
   * - Correlation between target xn[] and impulse response h1[]     *
   * - Innovative codebook search                                    *
   *-----------------------------------------------------------------*/



    pit_shrp (h1, T0, pit_sharp, L_SUBFR);

    corr_xh (xn, dn, h1, L_SUBFR);

    if (choice == 0) {
	fprintf (stderr, "ERROR: Choice should not be 0 in ACELP_Code\n");
	exit (1);
    }
    else {

	cod7_35 (xn, res2, l_subfr, h1, code, y2, gain_code, index);

    }

    pit_shrp (code, T0, pit_sharp, l_subfr);
    return;
}

#include <float.h>
#include <math.h>

#define NB_PULSE310  3
#define STEP310      7
#define NB_TRACK310  3




#include <math.h>
#include <float.h>





#include <float.h>
#include <math.h>
#include <stdio.h>


int index0[22] = { 1, 3, 6, 8, 10, 13, 15, 18, 20, 22, 25,
    27, 30, 32, 34, 37, 39, 42, 44, 46, 49, 51
};

int index1[23] = { 0, 2, 4, 7, 9, 12, 14, 16, 19, 21, 24,
    26, 28, 31, 33, 36, 38, 40, 43, 45, 48, 50, 52
};


void
build_rr (float h[],		/* input */
	  float rr[][VCM_L0],	/* output */
	  int l_subfr		/* input */
    )
{
    int i, j, k, dec;
    float s;

    /* Put h[i] = 0; for i>l_subfr */
    for (i = l_subfr; i < VCM_L0; i++)
	h[i] = 0.0;

    for (k = 0, s = 0.0, i = l_subfr - 1; k < l_subfr; k++, i--) {
	s += h[k] * h[k];
	rr[i][i] = s;
    }

    for (dec = 1; dec < l_subfr; dec++) {
	j = l_subfr - 1;
	i = j - dec;
	for (k = 0, s = 0.0; k < (l_subfr - dec); k++, i--, j--) {
	    s += h[k] * h[k + dec];
	    rr[i][j] = rr[j][i] = s;
	}
    }

    /* Put rr[i][j] = 0; for i,j>l_subfr */
    for (i = l_subfr; i < VCM_L0; i++)
	for (j = 0; j < VCM_L0; j++)
	    rr[i][j] = rr[j][i] = 0.0;
}

/* Pitch sharpening:  */

void
pit_shrp (float *x,		/* in/out: impulse response (or algebraic code) */
	  int pit_lag,		/* input : pitch lag                            */
	  float sharp,		/* input : pitch sharpening factor              */
	  int L_subfr		/* input : subframe size                        */
    )
{
    int i;


    if (pit_lag < L_subfr)
	for (i = pit_lag; i < L_subfr; i++)
	    x[i] += sharp * x[i - pit_lag];
}


/*----------------------------------------------------------------------*
 * procedure corr_xh:                                                   *
 *           ~~~~~~~~                                                   *
 *  Compute the correlation between the target signal and the impulse   *
 *  response of the weighted synthesis filter.                          *
 *                                                                      *
 *           y[i]=sum(j=i,l-1) x[j]*h[j-i], i=0,l-1                     *
 *                                                                      *
 *----------------------------------------------------------------------*/

void
corr_xh (float *x,		/* input : target signal                                   */
	 float *y,		/* output: correlation between x[] and h[]                 */
	 float *h,		/* input : impulse response (of weighted synthesis filter) */
	 int l			/* input : vector size                                     */
    )
{
    int i, j, l_1, l2;
    float s, *px, *px2, *ph;

    l_1 = l - 1;
    l2 = l_1;
    px2 = x;
    for (i = 0; i <= l_1; i++) {
	px = px2;
	ph = h;
	s = 0.0;
	for (j = 0; j <= l2; j++)
	    s += (*px++) * (*ph++);
	*y++ = s;
	l2--;
	px2++;
    }
    return;
}

/********************************************************
 Version with indices

 for (i = 0; i < l; i++) {
  s = 0.0;
  for (j = i; j < l; j++) s += x[j]*h[j-i];
  y[i] = s;
 }
 return;
}
*********************************************************/
/*
 * Factorial Packing C Language function.
 * 
 * By:   Edgardo M. Cruz-Zeno
 * Date: Aug/17/1998
 *
 */

#include <stdio.h>
#include <stdlib.h>
#include <math.h>		/* For rint() */

int64
F (int n, int m)
{
    int64 retval;
    int num, den;
    double dval;
    int max;

    if ((n < m) || (m < 0)) {
	retval = 0;
    }
    else {
	if ((n - m) > m) {
	    max = n - m;
	    den = m;
	}
	else {
	    max = m;
	    den = n - m;
	}
	dval = 1;
	for (num = n; num > max; num--) {
	    dval = dval * (double) (num) / (double) (den);
	    den = den - 1;
	}
	retval = (int64) rint (dval);
    }
    return (retval);
}

int64
D (int n, int m)
{
    return (F (n - 1, m - 1));
}

int64
Ipos (int *posvect, int vectlen)
{
    int indx;
    int64 pos = 0;

    for (indx = 0; indx < vectlen; indx++) {
	pos += F (posvect[indx], indx + 1);
    }
    return (pos);
}

int64
Ioff (int Nd, int Np, int len)
{
    int64 off = 0;
    int indx;

    for (indx = Nd + 1; indx <= Np; indx++) {
	off += F (len, indx) * D (Np, indx);
    }
    return (off);
}

int64
Ioff_S (int Nd, int Np, int len)
{
    int64 off = 0;
    int indx;

    for (indx = Nd + 1; indx <= Np; indx++) {
	off += F (len, indx) * D (Np, indx) * (0x0001 << indx);
    }
    return (off);
}


int64
factorial_pack (int Tlen, float *vector)
{
    int indx;
    int indx2;
    int Tvect[L_MAXSIZE];
    int TLam[L_MAXSIZE];
    int Spos;
    int Soff;
    int TLamindx;
    int Td, Tn, Tm;
    int Niter;
    int64 Tcode, code;
    int64 Toff;


    Td = 0;
    Tm = 0;
    Tn = Tlen;
    TLamindx = 0;
    for (indx = 0; indx < Tlen; indx++) {
	Tvect[indx] = abs ((int) (vector[indx]));
	if (Tvect[indx] != 0) {
	    Td++;
	    Tm += Tvect[indx];
	    TLam[TLamindx] = indx;
	    TLamindx++;
	}
    }

    Spos = 0;
    Soff = 0x01 << Td;
    for (indx = 0; indx < Td; indx++) {
	if (vector[TLam[indx]] < 0) {
	    Spos += (0x01 << (Td - indx - 1));
	}
    }

    code = Ioff_S (Td, Tm, Tn) + Spos;
    Niter = Tm - Td + 1;
    Toff = 0;
    Tcode = 0;
    for (indx = 0; indx < Niter; indx++) {
	Tcode += Toff + Ipos (TLam, Td) * D (Tm, Td);
	Tn = TLamindx;
	Tm = Tm - Tn;
	Td = 0;
	TLamindx = 0;
	for (indx2 = 0; indx2 < Tn; indx2++) {
	    Tvect[indx2] = Tvect[TLam[indx2]] - 1;
	    if (Tvect[indx2] != 0) {
		Td++;
		TLam[TLamindx] = indx2;
		TLamindx++;
	    }
	}
	Toff = Ioff (Td, Tm, Tn);
    }
    code += Soff * Tcode;
    return (code);
}



void
factorial_unpack (int64 code, int len, int Np, int *retvector)
{
    int Nd;
    int64 Tcode;
    int64 Tpos;
    int64 pos;
    int Spos;
    int *posvect;
    int TNd;
    int Tlen;
    int indx;
    int signvector[L_MAXSIZE];
    int pv_offset;

    Tlen = len;

    for (indx = 0; indx < Tlen; indx++) {
	retvector[indx] = 0;
	signvector[indx] = 1;
    }

    Nd = 0;
    do {
	Tcode = Ioff_S (Nd, Np, Tlen);
	Nd++;
    } while (Tcode > code);
    Nd--;
    Spos = code & ((0x0001 << Nd) - 1);

    Tcode = (code - Ioff_S (Nd, Np, Tlen)) >> Nd;
    pos = Tcode / D (Np, Nd);
    posvect = (int *) malloc (Nd * sizeof (int));
    for (indx = 0; indx < Nd; indx++) {
	TNd = Nd - indx - 1;
	while (F (TNd, Nd - indx) <= pos) {
	    TNd++;
	}
	TNd--;
	posvect[Nd - indx - 1] = TNd;
	retvector[TNd]++;
	signvector[TNd] = (Spos & (0x0001 << (indx))) ? -1 : 1;
	pos = pos - F (TNd, Nd - indx);
    }

    Tcode = (code - Ioff_S (Nd, Np, Tlen)) >> Nd;
    pos = Tcode / D (Np, Nd);
    code = Tcode - pos * D (Np, Nd);
    Np = Np - Nd;
    Tlen = Nd;
    pv_offset = Tlen;
    while (Np > 0) {
	Nd = 0;
	while (Ioff (Nd, Np, Tlen) > code) {
	    Nd++;
	}

	Tcode = (code - Ioff (Nd, Np, Tlen));
	pos = Tcode / D (Np, Nd);
	for (indx = 0; indx < Nd; indx++) {
	    TNd = Nd - indx - 1;
	    while (F (TNd, Nd - indx) <= pos) {
		TNd++;
	    }
	    TNd--;
	    retvector[posvect[TNd + pv_offset - Tlen]]++;
	    posvect[pv_offset - indx - 1] = posvect[TNd + pv_offset - Tlen];
	    pos = pos - F (TNd, Nd - indx);
	}
	Tcode = (code - Ioff (Nd, Np, Tlen));
	pos = Tcode / D (Np, Nd);
	code = Tcode - pos * D (Np, Nd);
	Np = Np - Nd;
	Tlen = Nd;
    }
    for (indx = 0; indx < len; indx++) {
	retvector[indx] = retvector[indx] * signvector[indx];
    }

    free (posvect);
}


#define min(a,b) (((a)<(b))?(a):(b))
#define MAXSIGNIFICANTBITS    19

int
cf (int n)
{
    static int b[160];
    unsigned int i, j;
    static int first = 1;

    if (first) {
	float a;

	for (i = 1; i < 160; i++) {
	    a = log10 ((float) i) / log10 ((float) 2);
	    j = (int) (a * pow (2.0, 21) + 1.0);
	    b[i - 1] = j;;
	}
	first = 0;
    }
    return (b[n - 1]);

}






int
ff (int n)
{
    static int b[65];

    static int first = 1;
    int i, j;

    if (first) {
	double a = 0;

	for (i = 1; i <= 65; i++) {
	    a = log10 ((float) i) / log10 ((float) 2);
	    if (i == 1)
		a = 0.0;
	    else {
		j = (int) (a * pow (2.0, 14) - 1.0);
		a = (j * pow (2.0, -14));
	    }

	    if (i < 2)
		b[i - 1] = (int) (a * pow (2, 20));
	    else
		b[i - 1] = b[i - 2] + (int) (a * pow (2, 20));
	}
	first = 0;
    }
    if ((n <= 0) || (n > 35))
	printf (" Possible Error \n");
    return (b[n - 1]);
}

typedef int int32_loc;



#define TAYLOR_PREC_BITS 29

int32_loc
mpy_32_32 (int32_loc arg1, int32_loc arg2)
{
    int32_loc result;
    unsigned int r1, r2;

    result = (arg1 >> 16) * (arg2 >> 16);
    r1 = (arg1 & 0xffff) * (arg2 >> 16);
    r1 = r1 + (arg2 & 0xffff) * (arg1 >> 16);
    result += (r1 >> 16);
    result = result << 1;
    return (result);
}

int
taylor_pow2_low (short darg, int k)
{
    short acc16;
    unsigned int dthird_termQ19 = 29738;
    unsigned int dsecond_termQ17 = 64370 / 2;
    unsigned int dfirst_termQ15 = 185688 / 8;
    unsigned int dzero_termQ14 = 133944 / 4;
    int dacc32;

    dacc32 = (dthird_termQ19 << 10);	/* Q35 */
    acc16 = ((dacc32) >> 16);	/* Q19 */
    dacc32 = ((acc16 * darg) >> (0 + k)) + (dsecond_termQ17 << 16);	/* Q33  */
    acc16 = ((dacc32) >> 16);	/* Q17 */
    dacc32 = ((acc16 * darg) >> (6 + k)) + (dfirst_termQ15 << 16);	/* Q31  */
    acc16 = ((dacc32) >> 16);	/* Q15 */
    dacc32 = ((acc16 * darg) >> (5 + k)) + (dzero_termQ14 << 15);	/* Q30  */
//      printf(" %x %x %x\n", arg, acc32, dacc32);

    return (dacc32);
}



int
logF (int n, int k)
{
    int i;
    int sum = 0;

    if ((k == 0) || (n == k))
	return (0);

    for (i = 0; i < min (k, n - k); i++) {
	sum += cf (n - i);
    }
    sum = (sum) - (ff (min (k, (n - k))) << 1);
    return (sum);
}


intVar
F_n (int n, int k)
{
    int x, y;
    intVar z;
    int a;
    int ai;

    int acc, acc1, acc2;
    static int first = 1;
    static int pwf[16];

    if (first) {
	int32_loc tmp;

	pwf[0] = 0x20000000;
	pwf[1] = 0x216ab0da;
	for (int i = 2; i < 16; i++) {
	    tmp = mpy_32_32 (pwf[i - 1], pwf[1]);
	    pwf[i] = (tmp << 2);
	}
	first = 0;
    }



    if ((n < k) || (k < 0) || (n < 0)) {
	x = 0;
	y = 0;
	return (0);
    }
    if (n == k)
	return (1);

    a = logF (n, k);
    ai = a >> 21;
    a = a - (ai << 21);
    if (ai < MAXSIGNIFICANTBITS) {
	acc = (a << (TAYLOR_PREC_BITS - 21));
	acc = (acc >> 8);
	acc = ((acc + 1) >> 1) << 9;	/* Removing 8+1 LSB with rounding */
	acc1 = taylor_pow2_low (((acc >> 9) & 0x0ffff) - 0x8000, 0);
	acc2 = pwf[acc >> 25];
	acc1 = mpy_32_32 (acc2, acc1);
	x = (acc1 >> (TAYLOR_PREC_BITS - ai - 1));

	//  x = round(pow(2.0, a)<<ai);
	y = 0;
    }
    else {
	acc = (a << (TAYLOR_PREC_BITS - 21));
	acc = (acc >> 9) << 9;	/* Removing 9 LSB which are not required */
	acc1 = taylor_pow2_low (((acc >> 9) & 0x0ffff) - 0x8000, 0);
	acc2 = pwf[acc >> 25];
	acc1 = mpy_32_32 (acc2, acc1);
	x = (acc1 >> (TAYLOR_PREC_BITS - MAXSIGNIFICANTBITS - 1));
	//  x = round(pow(2.0, a)<<MAXSIGNIFICANTBITS);
	y = ai - MAXSIGNIFICANTBITS;
    }
    z = x;
    z = (z << y);
    return (z);
}



int
Fv (int n, int m)
{
    int retval;
    int num, den;
    double dval;
    int max;

    if ((n < m) || (m < 0)) {
	retval = 0;
    }
    else {
	if ((n - m) > m) {
	    max = n - m;
	    den = m;
	}
	else {
	    max = m;
	    den = n - m;
	}
	dval = 1;
	if (Min (m, (n - m)) == 1)
	    return (n);
	if (Min (m, (n - m)) == 2)
	    return ((n * (n - 1)) >> 1);
	//  x1 = 1;
	for (num = n; num > max; num--) {
	    dval = (dval * (double) (num)) / (double) den;
	    den = den - 1;
	}
	retval = (int) rint (dval);
    }
    return (retval);
}


int
Dv (int n, int m)
{
    return (Fv (n - 1, m - 1));
}


intVar
Ipos1v (int *posvect, int vectlen)
{
    int indx;
    intVar pos = 0;

    for (indx = 0; indx < vectlen; indx++) {
	pos += F_n (posvect[indx], indx + 1);
    }
    return (pos);
}


int
Iposv (int *posvect, int vectlen)
{
    int indx;
    int pos = 0;

    for (indx = 0; indx < vectlen; indx++) {
	pos += Fv (posvect[indx], indx + 1);
    }
    return (pos);
}


int
Ioffv (int Nd, int Np, int len)
{
    int off = 0;
    int indx;

    for (indx = Nd + 1; indx <= min (Np, len); indx++) {
	off += Fv (len, indx) * Dv (Np, indx);
    }
    return (off);
}


intVar
Ioff_Sv (int Nd, int Np, int len)
{
    intVar off = 0;
    int indx;

    {
	for (indx = Nd + 1; indx <= Np; indx++) {
	    off += ((F_n (len, indx) * Dv (Np, indx)) << indx);
	}
    }
    return (off);
}


intVar
factorial_pack_new (int Tlen, float *vector)
{
    int indx;
    int indx2;
    int Tvect[160];
    int TLam[160];
    int Spos;
    int Sp1;
    int Soff;
    int TLamindx;
    int Td, Tn, Tm, Tdn;
    int Niter;
    int Tcode, Toff;
    intVar code;
    intVar Tcode1;

    Td = 0;
    Tm = 0;
    Tn = Tlen;
    TLamindx = 0;


    for (indx = 0; indx < Tlen; indx++) {
	Tvect[indx] = abs ((int) (vector[indx]));
	if (Tvect[indx] != 0) {
	    Td++;
	    Tm += Tvect[indx];
	    TLam[TLamindx] = indx;
	    TLamindx++;
	}
    }

    Spos = 0;
    Tdn = Td;
    Soff = (int) 0x01 << Td;
    for (indx = 0; indx < Td; indx++) {
	if (vector[TLam[indx]] < 0) {
	    Spos += ((int) 0x01 << (Td - indx - 1));
	}
    }
    Tcode1 = Spos;
    code = Ioff_Sv (Td, Tm, Tn);
    Tcode1 = Ipos1v (TLam, Td) * Dv (Tm, Td);
    Tcode1 = Tcode1 << Tdn;
    code = code + Tcode1;
    Tcode1 = Spos;
    code = code + Tcode1;

    Niter = Tm - Td + 1;
    Toff = 0;
    Tcode = 0;
    for (indx = 0; indx < Niter; indx++) {
	if (indx > 0) {
	    Tcode += Toff + Iposv (TLam, Td) * Dv (Tm, Td);
	}
	Tn = TLamindx;
	Tm = Tm - Tn;
	Td = 0;
	TLamindx = 0;
	for (indx2 = 0; indx2 < Tn; indx2++) {
	    Tvect[indx2] = Tvect[TLam[indx2]] - 1;
	    if (Tvect[indx2] != 0) {
		Td++;
		TLam[TLamindx] = indx2;
		TLamindx++;
	    }
	}
	Toff = Ioffv (Td, Tm, Tn);
    }
    Tcode1 = Tcode;
    code += (Tcode1 << Tdn);
    return (code);
}



int
factorial_unpack_new (intVar code, int len, int Np, int *retvector)
{
    int Nd;
    int Tcode;
    int code1;
    intVar Tcode1 = 0, Tcode2;
    int Tpos;
    intVar pos;
    int pos1;
    intVar const01 = 0x01;
    int Spos;
    int *posvect;
    int TNd;
    int Tlen;
    int indx;
    int signvector[160];
    int pv_offset;
    static long count = 0;
    int TNd1, TNd2;

    Tlen = len;

    for (indx = 0; indx < Tlen; indx++) {
	retvector[indx] = 0;
	signvector[indx] = 1;
    }
    Nd = Np;
    do {
	Tcode2 = Tcode1;
	Tcode1 += ((F_n (Tlen, Nd) * Dv (Np, Nd)) << Nd);
	Nd--;
	if (Nd == 0)
	    break;
    }
    while (Tcode1 <= code);
    Nd++;
    Spos = ((int) code.getKthWord (2) << 15) + code.getKthWord (1);
    Spos = Spos & ((0x01 << Nd) - 1);
    Tcode1 = code - Tcode2;
    Tcode1 = Tcode1 >> Nd;
    pos = Tcode1 / Dv (Np, Nd);
    Tcode1 = Tcode1 - pos * Dv (Np, Nd);
    code1 = ((int) Tcode1.getKthWord (2) << 15) + Tcode1.getKthWord (1);

    posvect = (int *) malloc (Nd * sizeof (int));
    for (indx = 0; indx < Nd; indx++) {
	int s2, s3, s4, s5;
	intVar tmp;

	if (indx == 0)
	    TNd1 = Tlen;
	else
	    TNd1 = TNd2;
	TNd2 = Nd - indx - 1;

	while ((TNd1 - TNd2) > 1) {
	    s2 = ((TNd1 + TNd2) >> 1);
	    if (F_n (s2, (Nd - indx)) <= pos) {
		TNd2 = s2;
	    }
	    else {
		TNd1 = s2;
	    }
	}
	TNd = TNd2;

	posvect[Nd - indx - 1] = TNd;
	retvector[TNd]++;
	signvector[TNd] = ((Spos & (0x01 << (indx))) > 0) ? -1 : 1;
	pos = pos - F_n (TNd, Nd - indx);
    }
    Np = Np - Nd;
    {
	if (pos > 0)
	    return (1);

    }

    //  Np = 0;
    Tlen = Nd;
    pv_offset = Tlen;
    while (Np > 0) {

	Nd = min (Np, Tlen);
	Tcode = 0;
	do {
	    Tcode += Fv (Tlen, Nd) * Dv (Np, Nd);
	    Nd--;
	} while (Tcode <= code1);
	Nd++;
	Tcode -= Fv (Tlen, Nd) * Dv (Np, Nd);
	Tcode = (code1 - Tcode);
	pos1 = Tcode / Dv (Np, Nd);
	for (indx = 0; indx < Nd; indx++) {
	    int s2;

	    if (indx == 0)
		TNd1 = Tlen;
	    else
		TNd1 = TNd2;
	    TNd2 = Nd - indx - 1;
	    while ((TNd1 - TNd2) > 1) {
		s2 = ((TNd1 + TNd2) >> 1);
		if (Fv (s2, (Nd - indx)) <= pos1) {
		    TNd2 = s2;
		}
		else {
		    TNd1 = s2;
		}
	    }
	    TNd = TNd2;

	    retvector[posvect[TNd + pv_offset - Tlen]]++;
	    posvect[pv_offset - indx - 1] = posvect[TNd + pv_offset - Tlen];
	    pos1 = pos1 - Fv (TNd, Nd - indx);
	}
	Tcode = (code1 - Ioffv (Nd, Np, Tlen));
	pos1 = Tcode / Dv (Np, Nd);
	code1 = Tcode - pos1 * Dv (Np, Nd);
	Np = Np - Nd;
	Tlen = Nd;
    }
    for (indx = 0; indx < len; indx++) {
	retvector[indx] = retvector[indx] * signvector[indx];
    }

    free (posvect);
    return (0);
}
