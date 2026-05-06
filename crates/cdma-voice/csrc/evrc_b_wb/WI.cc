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
    "$Id: WI.cc,v 1.12.6.3 2007/06/24 06:43:03 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <iostream>
using namespace std;
#include <assert.h>
#include "defines.h"
#include "struct.h"
#include "globs.h"
#include "proto.h"

#define AMP_UNQUANT 0
#define POWER_UNQUANT 0

#define AMP_UNQUANT_DIFF 0
#define POWER_UNQUANT_DIFF 0

#define	_POLY1(x, c)	((c)[0] * (x) + (c)[1])
#define	_POLY2(x, c)	(_POLY1((x), (c)) * (x) + (c)[2])
#define	_POLY3(x, c)	(_POLY2((x), (c)) * (x) + (c)[3])



//The constructor for DTFS class
DTFS::DTFS ()
{
    int i;

    lag = 0;
    for (i = 0; i < MAXLAG_WI; i++)
	a[i] = b[i] = 0.0;
}

//Operator '=' Overload
DTFS
DTFS::operator= (DTFS X2)
{
    int i;

    for (i = 0; i <= lag >> 1; i++) {
	a[i] = X2.a[i];
	b[i] = X2.b[i];
	lag = X2.lag;
    }
    return *this;
}

//Operator '+' Overload
//Sum of A and B coefficients in cartesian domain
//Equivalent to time domain addition
DTFS
DTFS::operator+ (DTFS X2)
{
    DTFS tmp;
    int i;

    for (i = 0; i <= lag / 2; i++) {
	tmp.a[i] = a[i];
	tmp.b[i] = b[i];
    }
    for (i = 0; i <= X2.lag / 2; i++) {
	tmp.a[i] += X2.a[i];
	tmp.b[i] += X2.b[i];
    }
    tmp.lag = MAX (lag, X2.lag);
    return tmp;
}

//Operator '-' Overload
//Difference of A and B coefficients in cartesian domain
//Equivalent to time domain subtraction
DTFS
DTFS::operator- (DTFS X2)
{
    DTFS tmp;
    int i;

    for (i = 0; i <= lag / 2; i++) {
	tmp.a[i] = a[i];
	tmp.b[i] = b[i];
    }
    for (i = 0; i <= X2.lag / 2; i++) {
	tmp.a[i] -= X2.a[i];
	tmp.b[i] -= X2.b[i];
    }
    tmp.lag = MAX (lag, X2.lag);
    return tmp;
}

//Operator '*' Overload
//Multiply A and B coefficients by a constant factor in cartesian domain
//Equivalent to energy scaling in time domain
DTFS
DTFS::operator* (float factor)
{
    DTFS tmp;
    int i;

    for (i = 0; i <= lag / 2; i++) {
	tmp.a[i] = a[i] * factor;
	tmp.b[i] = b[i] * factor;
    }
    tmp.lag = lag;
    return tmp;
}

//Operator '==' Overload
//Check whether two DTFS are bit exact
int
DTFS::operator== (DTFS X2)
{
    int i;

    if (lag != X2.lag)
	return FALSE;
    for (i = 0; i <= lag / 2; i++) {
	if (a[i] != X2.a[i])
	    return FALSE;
	if (b[i] != X2.b[i])
	    return FALSE;
    }
    return TRUE;
}

//Operator '^' Overload
//Computing the dot product (cross-correlation) between two DTFS
//It can be band specific
float
DTFS::operator^ (DTFS X2)
{
    int k;
    float corr = 0.0;

    if (lag > X2.lag)
	X2.zeroPadd (lag);

    //for (k=1; k<=(lag-1)>>1; k++)
    for (k = lag >> 2; k <= (lag - 1) >> 1; k++)
	corr += a[k] * X2.a[k] + b[k] * X2.b[k];

    corr /= 2.0;
    //  corr += a[0]*X2.a[0] ; 

    if (lag % 2 == 0)
	corr += a[k] * X2.a[k] + b[k] * X2.b[k];

    return corr;
}

float
DTFS::alignment (DTFS X2, float Eshift)
{				//Eshift is w.r.t  X2 
    int k;
    float maxcorr, corr, tmp, tmp1, fshift, n;
    DTFS X1;
    static int ALIGN_CB[16] =
	{ -10, -7, -6, -5, -4, -3, -2, -1, 0, 1, 2, 3, 4, 5, 6, 9 };

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    maxcorr = -HUGE_VAL;
    fshift = Eshift;

    int i;

    for (i = 0; i < 16; i++) {
	assert (Eshift == 0.0);
	n = (float) ALIGN_CB[i];
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr * (1 - 0.01 * fabs (n - Eshift)) > maxcorr) {
	    fshift = n;
	    maxcorr = corr;
	}
    }
    return fshift;
}

unsigned int
DTFS::glbl_alignment_QPPP (DTFS X2, float Eshift, int *ALIGN_CB,
			   unsigned int size)
{				//Eshift is w.r.t  X2 
    unsigned int i, fshift_idx = 8;	//Q_ROT_CB[8]=0;
    int k;
    float maxcorr, corr, tmp, tmp1, n;
    DTFS X1;

    assert (Eshift == 0.0);	//For the current setup in ppp.cc

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    maxcorr = -HUGE_VAL;

    for (i = 0; i < size; i++) {
	n = (float) ALIGN_CB[i];
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr * (1 - 0.01 * fabs (n - Eshift)) > maxcorr) {
	    fshift_idx = i;
	    maxcorr = corr;
	}
    }
    return fshift_idx;
}


double
DTFS::freq_corr (DTFS X2, float lband, float hband)
{
    int k;
    double corr, fdiff, freq;
    DTFS X1;

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    corr = freq = 0.0;
    fdiff = 8000.0 / X2.lag;

    for (k = 0; k <= X2.lag >> 1; k++, freq += fdiff) {
	assert (freq <= 4001.0);
	if (freq < hband && freq >= lband)
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]);
    }

    return corr / sqrt (X1.getEngy (lband, hband) *
			X2.getEngy (lband, hband));
}

float
DTFS::alignment_weight (DTFS X2, float Eshift, const float *LPC1,
			const float *LPC2)
{				//Eshift is w.r.t  X2 
    int k;
    float maxcorr, corr, Adiff, diff, tmp, tmp1, fshift, n, wcorr;
    DTFS X1;
    float pwf = 0.7, tmplpc[ORDER];

    X1 = *this;
    X1.adjustLag (X2.lag);

    X1.poleFilter (LPC1, ORDER);
    for (k = 0, tmp = 1.0; k < ORDER; k++)
	tmplpc[k] = LPC1[k] * (tmp *= pwf);
    X1.zeroFilter (tmplpc, ORDER);

    X2.poleFilter (LPC2, ORDER);
    for (k = 0, tmp = 1.0; k < ORDER; k++)
	tmplpc[k] = LPC2[k] * (tmp *= pwf);
    X2.zeroFilter (tmplpc, ORDER);
    maxcorr = -HUGE_VAL;
    fshift = Eshift;
    Adiff = MAX (6, 0.15 * X2.lag);
    if (X2.lag < 60)
	diff = 0.25;
    else
	diff = 0.5;

    for (n = Eshift - Adiff; n <= Eshift + Adiff; n += diff) {
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	wcorr = corr * (1 - 0.01 * fabs (n - Eshift));
	if (wcorr > maxcorr) {
	    //if (corr > maxcorr) {
	    fshift = n;
	    maxcorr = wcorr;
	}
    }

    return fshift;


}


float
DTFS::alignment_extract (DTFS X2, float Eshift, const float *LPC2)
{				//Eshift is w.r.t  X2 
    int k;
    float maxcorr, corr, Adiff, diff, tmp, tmp1, fshift, n;
    DTFS X1;
    float pwf = 0.7, tmplpc[ORDER];

    X1 = *this;
    X1.adjustLag (X2.lag);

    X1.poleFilter (LPC2, ORDER);
    X2.poleFilter (LPC2, ORDER);
    for (k = 0, tmp = 1.0; k < ORDER; k++)
	tmplpc[k] = LPC2[k] * (tmp *= pwf);
    X1.zeroFilter (tmplpc, ORDER);
    X2.zeroFilter (tmplpc, ORDER);

    maxcorr = -HUGE_VAL;
    fshift = Eshift;
    Adiff = MAX (4.0, lag / 8);
    diff = 1.0;			//Non-Fractional alignment

    for (n = Eshift - Adiff; n <= Eshift + Adiff; n += diff) {
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr * (1 - 0.01 * fabs (n - Eshift)) > maxcorr) {
	    fshift = n;
	    maxcorr = corr;
	}
    }
    return fshift;
}

float
DTFS::alignment_fine (DTFS X2, float Eshift)
{
    int k;
    float maxcorr, corr, Adiff, diff, tmp, tmp1, fshift, n;
    DTFS X1;

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    maxcorr = -HUGE_VAL;
    fshift = Eshift;
    Adiff = 4.0;
    //Adiff = MAX(6,0.15*X2.lag) ;
    diff = 0.25;		// 0.5 ;

    for (n = Eshift - Adiff; n <= Eshift + Adiff; n += diff) {
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr * (1 - 0.01 * fabs (n - Eshift)) > maxcorr) {
	    fshift = n;
	    maxcorr = corr;
	}
    }
    return fshift;
}

float
DTFS::alignment_full (DTFS X2, int num_steps)
{
    int k;
    float maxcorr, corr, tmp, tmp1, fshift, n, diff;
    DTFS X1;

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    maxcorr = -HUGE_VAL;
    diff = (float) X2.lag / num_steps;

    for (fshift = n = 0.0; n < (float) X2.lag; n += diff) {
	corr = tmp = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr > maxcorr) {
	    fshift = n;
	    maxcorr = corr;
	}
    }
    return fshift;
}


unsigned int
DTFS::glbl_alignment_FPPP (DTFS X2, int num_steps)
{
    int k;
    unsigned int n, idx = 0;
    float maxcorr, corr, tmp, tmp1, diff;
    DTFS X1;

    X1 = *this;
    if (lag < X2.lag)
	X1.zeroPadd (X2.lag);

    maxcorr = -HUGE_VAL;

    for (n = 0; n < num_steps; n++) {
	corr = tmp = 0.0;
	tmp1 = TWO_PI * (float) n / num_steps;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1) {
	    corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
	    corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	}
	if (corr > maxcorr) {
	    idx = n;
	    maxcorr = corr;
	}
    }
    return idx;
}

void
DTFS::phaseShift (float ph)
  // ph is the amount of phase shift (between 0 and 2pi)
  // + => shift to the left; - => shift to the right
{
    int k;
    float tmp, tmp2 = 0.0;

    for (k = 0; k <= lag >> 1; k++, tmp2 += ph) {
	tmp = a[k];
	a[k] = tmp * cos (tmp2) - b[k] * sin (tmp2);
	b[k] = tmp * sin (tmp2) + b[k] * cos (tmp2);
    }
}

void
DTFS::zeroInsert (int I)
{
    short nH, i, j;
    float tmp[MAXLAG_WI];

    if (I <= 1)
	return;

    nH = (I * lag) >> 1;

    for (i = 0; i <= nH; i++)
	tmp[i] = 0.0;
    for (i = 0, j = 0; i <= nH; i += I, j++)
	tmp[i] = a[j];
    for (i = 0; i <= nH; i++)
	a[i] = tmp[i];

    for (i = 0; i <= nH; i++)
	tmp[i] = 0.0;
    for (i = 0, j = 0; i <= nH; i += I, j++)
	tmp[i] = b[j];
    for (i = 0; i <= nH; i++)
	b[i] = tmp[i];

    lag *= I;
    if (lag > MAXLAG_WI) {
	cerr << "\n pitch has multipled and the lag is: " << lag;
	exit (0);
    }
}

void
DTFS::zeroPadd (int M)
{
    int i;

    if (M == lag)
	return;
    if (M < lag) {
	cerr << "\n**************ERROR IN ZEROPADD*************\n";
	exit (1);
    }
    for (i = (lag >> 1) + 1; i <= M >> 1; i++) {
	a[i] = b[i] = 0.0;
    }
    lag = M;
}

void
DTFS::to_fs (const float *x, int M)
{
    int nH, k, n;
    float sum, tmp;

    lag = M;

    // Number of harmonics excluding the ones at 0 and at pi
    nH = (M - 1) >> 1;

    // The DC component
    *a = 0.0;
    *b = 0.0;
    for (n = 0; n < M; n++)
	*a += x[n];
    *a /= M;

    //Strictly set the DC componet to zero
    *a = 0.0;

    // The harmonics excluding the one at pi
    for (k = 1; k <= nH; k++) {

	//When n=0
	a[k] = x[0];
	b[k] = 0.0;
	sum = tmp = TWO_PI * k / M;
	for (n = 1; n < M; n++, sum += tmp) {
	    a[k] += x[n] * cos (sum);
	    b[k] += x[n] * sin (sum);
	}
	a[k] *= (2.0 / M);
	b[k] *= (2.0 / M);
    }

    // The harmonic at 'pi'
    if (M % 2 == 0) {
	a[k] = 0.0;
	tmp = 1.0;
	for (n = 0; n < M; n++, tmp *= -1.0)
	    a[k] += x[n] * tmp;
	a[k] /= M;
	b[k] = 0.0;
    }
}

void
DTFS::fs_inv (float *x, int N, float ph0)
  // x - output temporal vector
  // N - the size of the output
  // ph0 - phase shift applied to the output vector
{
    float phase, tmp;
    int k, n;

    for (n = 0; n < N; n++) {
	x[n] = *a;
	tmp = phase = TWO_PI * n / lag + ph0;
	for (k = 1; k <= lag >> 1; k++, tmp += phase)
	    x[n] += a[k] * cos (tmp) + b[k] * sin (tmp);
    }
}


void
DTFS::print ()
{
    int i;

    cout << "\n******** " << lag << " *********************";
    for (i = 0; i <= lag >> 1; i++) {
	cout << "\n" << a[i] << " \t\t" << b[i];
    }
    cout << "\n**********************************\n";
}

void
DTFS::print_time ()
{
    int i;

    cout << "\n******** " << lag << " *********************";
    float tmp[MAXLAG];

    this->fs_inv (tmp, lag, 0.0);
    cout << "\n";
    for (i = 0; i < lag; i++)
	cout << tmp[i] << " ";
    for (; i < MAXLAG; i++)
	cout << "0 ";
    cout << "\n**********************************\n";
}


void
DTFS::print_time (float lband, float hband)
{
    int k;
    float tmp[MAXLAG], freq, fdiff = 8000.0 / lag;
    DTFS Xtmp = *this;

    for (freq = k = 0; k <= lag >> 1; k++, freq += fdiff)
	if (!(freq < hband && freq >= lband))
	    Xtmp.a[k] = Xtmp.b[k] = 0.0;

    Xtmp.fs_inv (tmp, lag, 0.0);
    cout << "\n";
    for (k = 0; k < lag; k++)
	cout << tmp[k] << " ";
    for (; k < MAXLAG; k++)
	cout << "0 ";
}



void
DTFS::transform (DTFS X2, const float *phase, float *out, int M)
{
    DTFS tmp1, tmp2, tmp3;
    int i, j, j1;
    float w, tmp;

#define threshld 0.8
#define sample_thld 20

    tmp1 = *this;
    tmp2 = X2;
    if (tmp1.lag < tmp2.lag)
	tmp1.zeroPadd (tmp2.lag);
    else if (tmp1.lag > tmp2.lag)
	tmp2.zeroPadd (tmp1.lag);

    tmp3.lag = tmp1.lag;
    tmp = log (1.0 - threshld) / (lag - M);
    for (i = 0; i < M; i++) {

	//    w = (float) (i + lag + 1) / (M + lag) ;
	if (M - sample_thld > lag)
	    w = 1.0 - exp (-(i + 1) * tmp);
	else
	    w = (float) (i + 1) / M;

	for (j = 0; j <= tmp3.lag >> 1; j++) {
	    tmp3.a[j] = (1 - w) * tmp1.a[j] + w * tmp2.a[j];
	    tmp3.b[j] = (1 - w) * tmp1.b[j] + w * tmp2.b[j];
	}

	tmp3.fs_inv (out + i, 1, phase[i]);

    }

}

void
DTFS::zeroFilter (const float *LPC, int N)
{
    float tmp, tmp1, tmp2, sum1, sum2;
    int k, n;

    tmp1 = TWO_PI / lag;

    for (k = 0; k <= lag >> 1; k++) {

	tmp = tmp2 = k * tmp1;

	/* Calculate sum1 and sum2 */
	sum1 = 1.0;
	sum2 = 0.0;
	for (n = 0; n < N; n++, tmp2 += tmp) {
	    sum1 += LPC[n] * cos (tmp2);
	    sum2 += LPC[n] * sin (tmp2);
	}

	/* Calculate the circular convolution */
	tmp = a[k];
	a[k] = tmp * sum1 - b[k] * sum2;
	b[k] = b[k] * sum1 + tmp * sum2;
    }
}

void
DTFS::poleFilter (const float *LPC, int N)
{
    float tmp, tmp1, tmp2, sum1, sum2;
    int k, n;

    tmp1 = TWO_PI / lag;

    for (k = 0; k <= lag >> 1; k++) {

	tmp = tmp2 = k * tmp1;

	/* Calculate sum1 and sum2 */
	sum1 = 1.0;
	sum2 = 0.0;
	for (n = 0; n < N; n++, tmp2 += tmp) {
	    sum1 += LPC[n] * cos (tmp2);
	    sum2 += LPC[n] * sin (tmp2);
	}

	/* Calculate the circular convolution */
	tmp = a[k];
	tmp2 = sum1 * sum1 + sum2 * sum2;
	a[k] = (tmp * sum1 + b[k] * sum2) / tmp2;
	b[k] = (-tmp * sum2 + b[k] * sum1) / tmp2;
    }
}

void
DTFS::adjustLag (int M)
{
    if (M == lag)
	return;
    if (M > lag)
	this->zeroPadd (M);
    else {
	int k;
	float en;

	en = this->getEngy ();
	for (k = (M >> 1) + 1; k <= lag >> 1; k++) {
	    a[k] = 0.0;
	    b[k] = 0.0;
	}
	this->setEngy (en);
	lag = M;
    }
}

float
DTFS::getEngy ()
{
    int k;
    float en = 0.0;

    for (k = 1; k <= (lag - 1) >> 1; k++)
	en += a[k] * a[k] + b[k] * b[k];

    en /= 2.0;
    en += a[0] * a[0];

    if (lag % 2 == 0)
	en += a[k] * a[k] + b[k] * b[k];

    return en;
}

float
DTFS::getEngy (float lband, float hband)
{
    int k;
    float en = 0.0, freq, fdiff = 8000.0 / lag;

    for (freq = fdiff, k = 1; k <= (lag - 1) >> 1; k++, freq += fdiff)
	if (freq < hband && freq >= lband)
	    en += a[k] * a[k] + b[k] * b[k];

    en /= 2.0;

    if (lband == 0.0)
	en += a[0] * a[0];

    if ((lag % 2 == 0) && (hband == 4000.0))
	en += a[k] * a[k] + b[k] * b[k];

    return en;
}

float
DTFS::setEngy (float en2)
{
    int k;
    float en1, tmp;

    en1 = this->getEngy ();
    if (en1 == 0.0)
	return 0.0;

    tmp = sqrt (en2 / en1);

    for (k = 0; k <= lag >> 1; k++) {
	a[k] *= tmp;
	b[k] *= tmp;
    }
    return en1;
}

void
DTFS::car2pol ()
{
    int k;
    float tmp;

    for (k = 1; k <= (lag - 1) >> 1; k++) {
	tmp = a[k];
	a[k] = 0.5 * hypot (tmp, b[k]);
	b[k] = atan2 (b[k], tmp);
    }
    if (lag % 2 == 0) {
	tmp = a[k];
	a[k] = hypot (tmp, b[k]);
	b[k] = atan2 (b[k], tmp);
    }
}

void
DTFS::pol2car ()
{
    int k;
    float tmp;

    for (k = 1; k <= (lag - 1) >> 1; k++) {
	tmp = b[k];
	b[k] = 2.0 * a[k] * sin (tmp);
	a[k] = 2.0 * a[k] * cos (tmp);
    }
    if (lag % 2 == 0) {
	tmp = b[k];
	b[k] = a[k] * sin (tmp);
	a[k] = a[k] * cos (tmp);
    }
}

float
DTFS::setEngyHarm (float f1, float f2, float g1, float g2, float en2)
{
    int k, count = 0;
    float en1 = 0.0, tmp, factor, diff = 8000.0 / lag;

    assert (f1 >= 0.0 && f2 <= 4000.0 && f1 < f2);
    assert (g1 >= 0.0 && g2 <= 4000.0 && g1 < g2);

    if (f1 == 0.0) {
	en1 += a[0] * a[0];
	count++;
    }


    for (k = 1, tmp = diff; k <= (lag - 1) >> 1; k++, tmp += diff) {
	assert (tmp <= 4000.0);
	assert (a[k] >= 0.0);
	if (tmp > f1 && tmp <= f2) {
	    en1 += a[k] * a[k];
	    count++;
	}
    }

    assert (count > 0);
    en1 /= count;

    assert (en2 >= 0.0);

    if (en1 > 0.0)
	factor = sqrt (en2 / en1);
    else
	factor = 0.0;

    if (g1 == 0.0)
	a[k] *= factor;

    for (k = 1, tmp = diff; k <= (lag - 1) >> 1; k++, tmp += diff) {
	if (tmp > g1 && tmp <= g2)
	    a[k] *= factor;
    }


    return en1 + 1e-20;
    //return en1;
}

void
DTFS::randph ()
{
    int k;

    for (k = 1; k <= (lag - 1) >> 1; k++) {
	b[k] = TWO_PI * rand () / 32768.0;
	if (a[k] < 0.0) {
	    cerr << "\n" << "No Negative amplitude" << "\n";
	    exit (0);
	}
    }
    if (lag % 2 == 0)
	b[k] = 0.0;
    b[0] = 0.0;
}

float
DTFS::SCR (DTFS X2)
{
    int i, nH = (MIN (lag, X2.lag)) >> 1;
    float en1 = 0.0, en2 = 0.0, endiff = 0.0;

    //  for (i=0;i<nH/4;i++) {
    //  for(i=nH/4; i<nH/2; i++) {
    //  for (i=nH/2; i<nH; i++) {
    for (i = 2; i <= nH / 2; i++) {
	en1 += a[i] * a[i] + b[i] * b[i];
	en2 += X2.a[i] * X2.a[i] + X2.b[i] * X2.b[i];
	/*
	   endiff += (X2.a[i] - a[i]) * (X2.a[i] - a[i]) ;
	   endiff += (X2.b[i] - b[i]) * (X2.b[i] - b[i]) ;
	 */
	endiff += a[i] * X2.a[i] + b[i] * X2.b[i];
    }

    if ((en1 == 0.0) || (en2 == 0.0))
	return 0.0;
    return (1.0 / (1.0 - endiff * endiff / en1 / en2));
}

void
DTFS::injectnoise (float scr)
{
    int i, nH = lag >> 1;

    if (scr == 0.0)
	return;
    DTFS tmp = *this;

    for (i = 0; i < nH / 2; i++)
	tmp.a[i] = tmp.b[i] = 0.0;

    for (; i < nH; i++) {
	//    tmp.a[i] = a[i]*sqrt(1.0/scr) ;
	//    tmp.b[i] = b[i]*sqrt(1.0/scr) ;
	tmp.a[i] = a[i] * scr;
	tmp.b[i] = b[i] * scr;
    }

    tmp.car2pol ();
    tmp.randph ();
    tmp.pol2car ();

    *this = *this + tmp;

}

void
cubicPhase (float ph1, float ph2, float L1, float L2, int N, float *phOut)
{
    float coef[4], f1, f2, c1, c2, factor;
    int n;

    N -= (int) L2;
    assert (N >= 0);

    // Computation of the coefficients of the cubic phase function
    f1 = TWO_PI / L1;
    f2 = TWO_PI / L2;

    ph1 = fmod (double (ph1), double (TWO_PI));
    ph2 = fmod (double (ph2), double (TWO_PI));

    coef[3] = ph1;
    coef[2] = f1;

    factor = anint ((ph1 - ph2 + 0.5 * N * (f2 + f1)) / TWO_PI);

    c1 = f2 - f1;
    c2 = ph2 - ph1 - N * f1 + TWO_PI * factor;

    coef[0] = (N * c1 - 2 * c2) / (N * N * N);
    coef[1] = (c1 - 3 * N * N * coef[0]) / (2 * N);

    // Computation of the phase value at each sample point
    phOut[0] = ph1;
    for (n = 1; n < N; n++)
	phOut[n] = _POLY3 (n, coef);

    double diff = TWO_PI / L2;

    for (; n < N + (int) L2; n++)
	phOut[n] = phOut[n - 1] + diff;
}

float
getSCR (const float *resid, int lag)
{
    int i, num_PP;
    float TMP[FSIZE], tmp, tmp1, aligntmp = 0.0;

    num_PP = 1 + (int) (anint ((float) FSIZE / lag));
    if (num_PP == 1)
	return 0.0;

    DTFS *CW = new DTFS[num_PP];

    for (i = 1; i <= num_PP; i++) {
	ppp_extract_pitch_period (resid - FSIZE + FSIZE * i / num_PP, TMP,
				  lag);
	CW[i - 1].to_fs (TMP, lag);
    }

    for (i = num_PP - 1; i > 0; i--) {
	tmp = fmod ((aligntmp - (FSIZE / num_PP) % CW[i].lag), CW[i].lag);
	aligntmp = CW[i].alignment_fine (CW[i - 1], tmp);
	tmp = TWO_PI * aligntmp / CW[i - 1].lag;
	CW[i - 1].phaseShift (tmp);
    }

    tmp = tmp1 = 0.0;
    for (i = 0; i < num_PP - 1; i++) {

	tmp =
	    pow ((CW[i] ^ CW[i + 1]),
		 2) / (CW[i] ^ CW[i]) / (CW[i + 1] ^ CW[i + 1]);
	tmp = MIN (1.0, tmp * 1.15);
	tmp1 += sqrt (1.0 - tmp);

	/*
	   tmp = pow((CW[i]^CW[i+1]),2)/(CW[i]^CW[i])/(CW[i+1]^CW[i+1]) ;
	   tmp1 += (1.0-tmp) ;
	 */
    }

    delete[]CW;
    tmp1 /= (num_PP - 1);

    //  tmp1 = MIN(1.0, tmp1*1.15);

    //cout<<"\n"<<anint(sqrt(1.0-tmp1)*10.0)/10.0;
    //return sqrt(1.0-tmp1) ;
    //return (anint(sqrt(1.0-tmp1)*10.0)/10.0) ;
    return (anint (tmp1 * 10.0)) / 10.0;
}

static float erb[NUM_ERB + 1] =
    { 0.0, 92.8061, 185.6121, 278.4182, 371.2243, 464.0304, 556.8364,
649.6425, 746.4, 853.6, 972.5, 1104.0, 1251.8, 1415.8, 1599.2, 1804.6, 2035.2, 2294.9, 2588.4,
2921.2, 3300.1, 3733.7, 4000.0 + 1.0 };

void
DTFS::to_erb (float *out)
{
    unsigned short i, j, count[NUM_ERB];
    float freq, diff;

    for (i = 0; i < NUM_ERB; i++) {
	count[i] = 0;
	out[i] = 0.0;
    }

    diff = 8000.0 / lag;
    for (i = j = 0, freq = 0.0; i <= lag >> 1; i++, freq += diff) {
	assert (freq <= erb[NUM_ERB]);
	for (; j < NUM_ERB; j++) {
	    if (freq < erb[j + 1]) {
		assert (a[i] >= 0.0);
		out[j] += a[i];
		count[j]++;
		break;
	    }
	}
    }

    for (i = 0; i < NUM_ERB; i++)
	if (count[i] > 1)
	    out[i] /= count[i];
}

void
erb_slot (int lag, int *out, float *mfreq)
{
    unsigned short i, j;
    float freq, diff;

    for (i = 0; i < NUM_ERB; i++) {
	out[i] = 0;
	mfreq[i] = 0.0;
    }

    diff = 8000.0 / lag;
    for (i = j = 0, freq = 0.0; i <= lag >> 1; i++, freq += diff) {
	assert (freq <= erb[NUM_ERB]);
	freq = MIN (freq, 4000.0);
	for (; j < NUM_ERB; j++) {
	    if (freq < erb[j + 1]) {
		mfreq[j] += freq;
		out[j]++;
		break;
	    }
	}
    }
    for (j = 0; j < NUM_ERB; j++)
	if (out[j] > 1)
	    mfreq[j] /= out[j];
}

void
DTFS::erb_inv (float *in, int *slot, float *mfreq)
{
    short i, j, m = 0;
    float diff;
    float freq, f[NUM_ERB + 2], amp[NUM_ERB + 2];

    f[m] = 0.0;
    amp[m] = 0.0;
    m++;
    for (i = 0; i < NUM_ERB; i++) {
	if (slot[i] != 0) {
	    f[m] = mfreq[i];
	    amp[m] = in[i];
	    m++;
	}
    }
    f[m] = 4000.0;
    amp[m] = 0.0;
    m++;

    diff = 8000.0 / lag;
    for (i = 0, j = 1, freq = 0.0; i <= lag >> 1; i++, freq += diff) {
	assert (freq <= erb[NUM_ERB]);
	assert (m <= NUM_ERB + 2);
	if (freq > 4000.0)
	    freq = 4000.0;
	for (; j < m; j++) {
	    if (freq <= f[j]) {
		a[i] =
		    amp[j] * (freq - f[j - 1]) + amp[j - 1] * (f[j] - freq);
		if (f[j] != f[j - 1])
		    a[i] /= (f[j] - f[j - 1]);
		break;
	    }
	}
	assert (a[0] == 0.0);
    }
}

void
LPCPowSpect (const float *freq, int Nf, const float *LPC, int Np, float *out)
{
    float w, tmp, Re, Im;
    int i, k;

    for (k = 0; k < Nf; k++) {
	Re = 1.0;
	Im = 0.0;
	//Note that freq ranges between [0 4000.0]
	tmp = freq[k] / 8000.0 * TWO_PI;
	for (i = 0, w = tmp; i < Np; i++, w += tmp) {
	    Re += LPC[i] * cos (w);
	    Im -= LPC[i] * sin (w);
	}
	out[k] = 1.0 / (Re * Re + Im * Im);
    }
}


float
DTFS::getSpEngyFromResAmp (float lband, float hband, const float *curr_lsp)
{
    int i, k;
    float w, tmp, Re, Im;
    double en = 0.0, freq, fdiff = 8000.0 / lag;

    assert (lband < hband);
    assert (lband >= 0.0 && hband <= 4000.0);

    if (hband == 4000.0)
	hband = 4001.0;

    for (freq = 0.0, k = 0; k <= lag >> 1; k++, freq += fdiff) {
	assert (a[k] >= 0.0);

	if (freq < hband && freq >= lband) {
	    Re = 1.0;
	    Im = 0.0;
	    tmp = TWO_PI * freq / 8000.0;
	    for (i = 0, w = tmp; i < ORDER; i++, w += tmp) {
		Re += curr_lsp[i] * cos (w);
		Im -= curr_lsp[i] * sin (w);
	    }
	    if (k == 0 || (lag % 2 == 0 && k == lag >> 1))
		en += a[k] * a[k] / (Re * Re + Im * Im);
	    else
		en += 2.0 * a[k] * a[k] / (Re * Re + Im * Im);
	}
    }
    return en;
}

void
DTFS::deNormalizeResAmp (float f1, float f2, float g1, float g2, float SpEngy,
			 const float *curr_lsp)
{
    int i, k;
    float w, tmp, Re, Im;
    double en = 0.0, freq, fdiff = 8000.0 / lag;

    assert (f1 < f2 && f1 >= 0.0 && f2 <= 4000.0);
    assert (g1 < g2 && g1 >= 0.0 && g2 <= 4000.0);

    if (f2 == 4000.0)
	f2 = 4001.0;
    if (g2 == 4000.0)
	g2 = 4001.0;

    for (freq = 0.0, k = 0; k <= lag >> 1; k++, freq += fdiff) {
	assert (a[k] >= 0.0);

	if (freq < f2 && freq >= f1) {
	    Re = 1.0;
	    Im = 0.0;
	    tmp = TWO_PI * freq / 8000.0;
	    for (i = 0, w = tmp; i < ORDER; i++, w += tmp) {
		Re += curr_lsp[i] * cos (w);
		Im -= curr_lsp[i] * sin (w);
	    }
	    if (k == 0 || (lag % 2 == 0 && k == lag >> 1))
		en += a[k] * a[k] / (Re * Re + Im * Im);
	    else
		en += 2.0 * a[k] * a[k] / (Re * Re + Im * Im);
	}
    }
    tmp = sqrt (SpEngy / en);

    for (freq = 0.0, k = 0; k <= lag >> 1; k++, freq += fdiff)
	if (freq < g2 && freq >= g1)
	    a[k] *= tmp;
}

#define ERB_CBSIZE1 64
#define ERB_CBSIZE2 64
#include "AmpCB1.64"
#include "AmpCB2.64"

void
erb_diff (const float *prev_erb, int pl, const float *curr_erb, int l,
	  const float *curr_lsp, float *out, int *index)
{
    int i, j, pslot[NUM_ERB], cslot[NUM_ERB], mmseindex;
    float tmp, t_prev_erb[NUM_ERB], LPC[ORDER], mfreq[NUM_ERB],
	PowSpect[NUM_ERB], mmse;

    erb_slot (l, cslot, mfreq);
    erb_slot (pl, pslot, t_prev_erb);

    for (i = 0, tmp = 1.0; i < ORDER; i++)
	LPC[i] = curr_lsp[i] * (tmp *= 0.78);
    LPCPowSpect (mfreq, NUM_ERB, LPC, ORDER, PowSpect);

    for (i = 0; i < NUM_ERB; i++)
	t_prev_erb[i] = prev_erb[i];

    if (pl > l) {
	tmp = t_prev_erb[0];
	for (i = 0; i < NUM_ERB; i++) {
	    assert (pslot[i] >= 0);
	    if (pslot[i] != 0)
		tmp = t_prev_erb[i];
	    else
		t_prev_erb[i] = tmp;
	}
    }
    else if (l > pl) {
	tmp = t_prev_erb[NUM_ERB - 1];
	for (i = NUM_ERB - 1; i >= 0; i--) {
	    if (pslot[i] != 0)
		tmp = t_prev_erb[i];
	    else
		t_prev_erb[i] = tmp;
	}
    }

    for (i = 0; i < NUM_ERB; i++)
	out[i] = curr_erb[i] - t_prev_erb[i];

    //First Band Amplitude Search
    mmse = HUGE_VAL;
    mmseindex = -1;
    for (j = 0; j < ERB_CBSIZE1; j++) {
	tmp = 0.0;
	for (i = 1; i < 11; i++) {
	    if (cslot[i] != 0) {
		float tmp1;

		if (AmpCB1[j][i - 1] < -t_prev_erb[i])
		    tmp1 = PowSpect[i] * SQR (curr_erb[i]);
		else
		    tmp1 = PowSpect[i] * SQR (out[i] - AmpCB1[j][i - 1]);
		if (AmpCB1[j][i - 1] < out[i])
		    tmp1 *= 0.9;
		tmp += tmp1;
	    }
	}
	if (tmp < mmse) {
	    mmse = tmp;
	    mmseindex = j;
	}
    }
    assert (mmseindex < ERB_CBSIZE1 && mmseindex >= 0);
    index[0] = mmseindex;

    //Second Band Amplitude Search
    mmse = HUGE_VAL;
    mmseindex = -1;
    for (j = 0; j < ERB_CBSIZE2; j++) {
	tmp = 0.0;
	for (i = 11; i < 20; i++) {
	    if (cslot[i] != 0) {
		float tmp1;

		if (AmpCB2[j][i - 11] < -t_prev_erb[i])
		    tmp1 = PowSpect[i] * SQR (curr_erb[i]);
		else
		    tmp1 = PowSpect[i] * SQR (out[i] - AmpCB2[j][i - 11]);
		if (AmpCB2[j][i - 11] < out[i])
		    tmp1 *= 0.9;
		tmp += tmp1;
	    }
	}
	if (tmp < mmse) {
	    mmse = tmp;
	    mmseindex = j;
	}
    }
    assert (mmseindex < ERB_CBSIZE2 && mmseindex >= 0);
    index[1] = mmseindex;

}

void
erb_add (float *curr_erb, int l, const float *prev_erb, int pl, int *index)
{
    int i, pslot[NUM_ERB], cslot[NUM_ERB];
    float tmp, t_prev_erb[NUM_ERB];
    void erb_slot (int, int *, float *);

    erb_slot (l, cslot, t_prev_erb);
    erb_slot (pl, pslot, t_prev_erb);

    for (i = 0; i < NUM_ERB; i++)
	t_prev_erb[i] = prev_erb[i];

    if (pl > l) {
	tmp = t_prev_erb[0];
	for (i = 0; i < NUM_ERB; i++) {
	    assert (pslot[i] >= 0);
	    if (pslot[i] != 0)
		tmp = t_prev_erb[i];
	    else
		t_prev_erb[i] = tmp;
	}
    }
    else if (l > pl) {
	tmp = t_prev_erb[NUM_ERB - 1];
	for (i = NUM_ERB - 1; i >= 0; i--) {
	    if (pslot[i] != 0)
		tmp = t_prev_erb[i];
	    else
		t_prev_erb[i] = tmp;
	}
    }

    for (i = 1; i < 11; i++) {
	if (cslot[i] != 0) {
	    curr_erb[i] = AmpCB1[index[0]][i - 1] + t_prev_erb[i];
	    curr_erb[i] = MAX (0.0, curr_erb[i]);
	}
	else
	    curr_erb[i] = 0.0;
    }

    for (i = 11; i < 20; i++) {
	if (cslot[i] != 0) {
	    curr_erb[i] = AmpCB2[index[1]][i - 11] + t_prev_erb[i];
	    curr_erb[i] = MAX (0.0, curr_erb[i]);
	}
	else
	    curr_erb[i] = 0.0;
    }
}


float G_CURR_ERB[NUM_ERB];
float G_POWER[2];
float G_A_POWER[2];


#define P_CBSIZE 64
#include "PowerCB.64"

#define A_P_CBSIZE 256
#include "A_PowerCB.256"


#define A_ERB_CBSIZE0 64
#define A_ERB_CBSIZE1 64
#define A_ERB_CBSIZE2 256
#include "A_AmpCB0.64"
#include "A_AmpCB1.64"
#include "A_AmpCB2.256"


void
erb_quant (const float *curr_erb, int l, const float *curr_lsp, int *index)
{
    int i, j, cslot[NUM_ERB], mmseindex;
    float tmp, LPC[ORDER], mfreq[NUM_ERB], PowSpect[NUM_ERB], mmse;

    erb_slot (l, cslot, mfreq);

    for (i = 0, tmp = 1.0; i < ORDER; i++)
	LPC[i] = curr_lsp[i] * (tmp *= 0.78);
    LPCPowSpect (mfreq, NUM_ERB, LPC, ORDER, PowSpect);

    //First Band Amplitude Search
    mmse = HUGE_VAL;
    mmseindex = -1;
    for (j = 0; j < A_ERB_CBSIZE0; j++) {
	tmp = 0.0;
	for (i = 1; i < 6; i++) {
	    if (cslot[i] != 0) {
		float tmp1;

		tmp1 = PowSpect[i] * SQR (curr_erb[i] - A_AmpCB0[j][i - 1]);
		if (A_AmpCB0[j][i - 1] < curr_erb[i]) {
		    tmp += (float) (tmp1 * 0.9);
		}
		else {
		    tmp += tmp1;
		}
	    }
	}
	if (tmp < mmse) {
	    mmse = tmp;
	    mmseindex = j;
	}
    }
    assert (mmseindex < A_ERB_CBSIZE0 && mmseindex >= 0);
    index[0] = mmseindex;

    //Second Band Amplitude Search
    mmse = HUGE_VAL;
    mmseindex = -1;
    for (j = 0; j < A_ERB_CBSIZE1; j++) {
	tmp = 0.0;
	for (i = 6; i < 11; i++) {
	    if (cslot[i] != 0) {
		float tmp1;

		tmp1 = PowSpect[i] * SQR (curr_erb[i] - A_AmpCB1[j][i - 6]);
		if (A_AmpCB1[j][i - 6] < curr_erb[i]) {
		    tmp += (float) (tmp1 * 0.9);
		}
		else {
		    tmp += tmp1;
		}
	    }
	}
	if (tmp < mmse) {
	    mmse = tmp;
	    mmseindex = j;
	}
    }
    assert (mmseindex < A_ERB_CBSIZE1 && mmseindex >= 0);
    index[1] = mmseindex;

    //Third Band Amplitude Search
    mmse = HUGE_VAL;
    mmseindex = -1;
    for (j = 0; j < A_ERB_CBSIZE2; j++) {
	tmp = 0.0;
	for (i = 11; i < 20; i++) {
	    if (cslot[i] != 0) {
		float tmp1;

		tmp1 = PowSpect[i] * SQR (curr_erb[i] - A_AmpCB2[j][i - 11]);
		if (A_AmpCB2[j][i - 11] < curr_erb[i]) {
		    tmp += (float) (tmp1 * 0.9);
		}
		else {
		    tmp += tmp1;
		}
	    }
	}
	if (tmp < mmse) {
	    mmse = tmp;
	    mmseindex = j;
	}
    }
    assert (mmseindex < A_ERB_CBSIZE2 && mmseindex >= 0);
    index[2] = mmseindex;
}

void
erb_dequant (float *curr_erb, int l, int *index)
{
    int i, cslot[NUM_ERB];
    float JUNK[NUM_ERB];
    void erb_slot (int, int *, float *);

    erb_slot (l, cslot, JUNK);

    for (i = 1; i < 6; i++) {
	if (cslot[i] != 0)
	    curr_erb[i] = A_AmpCB0[index[0]][i - 1];
	else
	    curr_erb[i] = 0.0;
    }

    for (i = 6; i < 11; i++) {
	if (cslot[i] != 0)
	    curr_erb[i] = A_AmpCB1[index[1]][i - 6];
	else
	    curr_erb[i] = 0.0;
    }

    for (i = 11; i < 20; i++) {
	if (cslot[i] != 0)
	    curr_erb[i] = A_AmpCB2[index[2]][i - 11];
	else
	    curr_erb[i] = 0.0;
    }
}

void
DTFS::quant_cw_memless (const float *curr_lsp, int &A_POWER_IDX,
			int *A_AMP_IDX)
{
    float tmp, w[2], target1, target2, error, minerror;
    float mfreq[NUM_ERB], curr_erb[NUM_ERB];
    int j, slot[NUM_ERB];

    //Getting the Speech Domain Energy LOG Ratio
    w[0] =
	MAX (0.0, log10 (this->getSpEngyFromResAmp (0.0, 1104.5, curr_lsp)));
    w[1] =
	MAX (0.0,
	     log10 (this->getSpEngyFromResAmp (1104.5, 4000.0, curr_lsp)));
    tmp = w[0] + w[1];
    w[0] /= tmp;
    w[1] /= tmp;

    //Power Quantization
    target1 = G_A_POWER[0] =
	log10 (lag * this->setEngyHarm (92.0, 1104.5, 0., 1104.5, 1.0));
    target2 = G_A_POWER[1] =
	log10 (lag * this->setEngyHarm (1104.5, 3300, 1104.5, 4000, 1));

    /*
       {
       static FILE *fp1, *fp2;
       static int INIT=0 ;
       if (INIT==0) {
       if ( (fp1=fopen ("APOWER","w")) == NULL) {
       fprintf(stderr, "Cannot power data");
       exit(1);
       }
       if ( (fp2=fopen ("APOWERWEIGHT","w")) == NULL) {
       fprintf(stderr, "Cannot power weight data");
       exit(1);
       }
       INIT = 1;
       }
       fwrite(G_A_POWER,sizeof(float),2,fp1);
       fwrite(w,sizeof(float),2,fp2);
       }
     */

    minerror = HUGE_VAL;
    A_POWER_IDX = -1;
    for (j = 0; j < A_P_CBSIZE; j++) {
	error =
	    w[0] * fabs (target1 - A_PowerCB[j][0]) + w[1] * fabs (target2 -
								   A_PowerCB
								   [j][1]);
	if ((target1 >= A_PowerCB[j][0]) && (target2 >= A_PowerCB[j][1]))
	    error *= 0.8;
	if (error < minerror) {
	    minerror = error;
	    A_POWER_IDX = j;
	}
    }

    //Amplitude Quantization
    this->to_erb (curr_erb);

    for (j = 0; j < NUM_ERB; j++)
	G_CURR_ERB[j] = curr_erb[j];

    erb_slot (lag, slot, mfreq);
    erb_quant (curr_erb, lag, curr_lsp, A_AMP_IDX);


    //Amplitude Dequantization
    erb_dequant (curr_erb, lag, A_AMP_IDX);
    curr_erb[0] = curr_erb[1] * 0.3;
    curr_erb[20] = curr_erb[19] * 0.3;
    curr_erb[21] = 0.0;


    this->erb_inv (curr_erb, slot, mfreq);

    //Power Dequantization
    target1 = A_PowerCB[A_POWER_IDX][0];
    target2 = A_PowerCB[A_POWER_IDX][1];

    target1 = pow (double (10.0), double (target1)) / lag;

    assert (target1 >= 0.0);
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, target1);

    target2 = pow (double (10.0), double (target2)) / lag;

    assert (target2 >= 0.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, target2);
}


void
DTFS::dequant_cw_memless (int &A_POWER_IDX, int *A_AMP_IDX)
{
    float tmp, tmp1, tmp2, mfreq[NUM_ERB], curr_erb[NUM_ERB];
    int slot[NUM_ERB];

    //Amplitude Dequantization
    erb_dequant (curr_erb, lag, A_AMP_IDX);
    curr_erb[0] = curr_erb[1] * 0.3;
    curr_erb[20] = curr_erb[19] * 0.3;
    curr_erb[21] = 0.0;
    erb_slot (lag, slot, mfreq);


    this->erb_inv (curr_erb, slot, mfreq);

    //Power Dequantization
    tmp1 = A_PowerCB[A_POWER_IDX][0];
    tmp2 = A_PowerCB[A_POWER_IDX][1];

    tmp = pow (double (10.0), double (tmp1)) / lag;

    assert (tmp >= 0.0);
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, tmp);

    tmp = pow (double (10.0), double (tmp2)) / lag;

    assert (tmp >= 0.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, tmp);
}

bool
DTFS::quant_cw (int pl, const float *curr_lsp, int &POWER_IDX, int *AMP_IDX,
		float &lastLgainE, float &lastHgainE, float *lasterbE)
{
    //extern float lastLgainE, lastHgainE, lasterbE[NUM_ERB] ;
    float tmp, w[2], target1, target2, error, minerror;
    float mfreq[NUM_ERB], diff_erb[NUM_ERB], curr_erb[NUM_ERB];
    int j, slot[NUM_ERB];
    bool returnFlag = TRUE;

    //Getting the Speech Domain Energy LOG Ratio
    w[0] =
	MAX (0.0, log10 (this->getSpEngyFromResAmp (0.0, 1104.5, curr_lsp)));
    w[1] =
	MAX (0.0,
	     log10 (this->getSpEngyFromResAmp (1104.5, 4000.0, curr_lsp)));
    tmp = w[0] + w[1];
    w[0] /= tmp;
    w[1] /= tmp;

    //Power Quantization
    G_A_POWER[0] =
	log10 (lag * this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
    G_A_POWER[1] =
	log10 (lag * this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
    G_POWER[0] = target1 = G_A_POWER[0] - lastLgainE;
    G_POWER[1] = target2 = G_A_POWER[1] - lastHgainE;
    /*
       {
       static FILE *fp1, *fp2;
       static int INIT=0 ;
       if (INIT==0) {
       if ( (fp1=fopen ("/tmp/POWER","w")) == NULL) {
       fprintf(stderr, "Cannot open power data");
       exit(1);
       }
       if ( (fp2=fopen ("/tmp/POWERWEIGHT","w")) == NULL) {
       fprintf(stderr, "Cannot open power weight data");
       exit(1);
       }
       INIT = 1;
       }
       fwrite(G_POWER,sizeof(float),2,fp1);
       fwrite(w,sizeof(float),2,fp2);
       }
     */
    minerror = HUGE_VAL;
    POWER_IDX = -1;
    for (j = 0; j < P_CBSIZE; j++) {
	error =
	    w[0] * fabs (target1 - PowerCB[j][0]) + w[1] * fabs (target2 -
								 PowerCB[j]
								 [1]);
	if ((target1 >= PowerCB[j][0]) && (target2 >= PowerCB[j][1]))
	    error *= 0.8;
	if (error < minerror) {
	    minerror = error;
	    POWER_IDX = j;
	}
    }

    //Amplitude Quantization
    this->to_erb (curr_erb);

    for (j = 0; j < NUM_ERB; j++)
	G_CURR_ERB[j] = curr_erb[j];

    erb_slot (lag, slot, mfreq);
    erb_diff (lasterbE, pl, curr_erb, lag, curr_lsp, diff_erb, AMP_IDX);
    /*
       {
       static FILE *fp1, *fp2, *fp3, *fp4 ;
       static int INIT=0 ;
       float fslot[NUM_ERB];

       if (INIT==0) {
       if ( (fp1=fopen ("/tmp/data1","w")) == NULL) {
       fprintf(stderr, "Cannot open data");
       exit(1);
       }
       if ( (fp2=fopen ("/tmp/weight1","w")) == NULL) {
       fprintf(stderr, "Cannot open weight");
       exit(1);
       }
       if ( (fp3=fopen ("/tmp/data2","w")) == NULL) {
       fprintf(stderr, "Cannot open data");
       exit(1);
       }
       if ( (fp4=fopen ("/tmp/weight2","w")) == NULL) {
       fprintf(stderr, "Cannot open weight");
       exit(1);
       }
       INIT = 1;
       }

       for (j=0 ;j<NUM_ERB; j++) {
       if (slot[j] == 0)
       fslot[j] = 0.0 ;
       else
       fslot[j] = 1.0 ;
       }

       fwrite(diff_erb+1,sizeof(float),10,fp1);
       fwrite(fslot+1,sizeof(float),10,fp2);
       fwrite(diff_erb+11,sizeof(float),9,fp3);
       fwrite(fslot+11,sizeof(float),9,fp4);
       }
     */
    //Amplitude Dequantization
    erb_add (curr_erb, lag, lasterbE, pl, AMP_IDX);
    curr_erb[0] = curr_erb[1] * 0.3;
    curr_erb[20] = curr_erb[19] * 0.3;
    curr_erb[21] = 0.0;

    //Determine if the amplitude quantization is good enough
    float amperror = 0.0;
    int bincount = 0;

    for (j = 1; j < 10; j++) {
	if (slot[j] != 0) {
	    amperror += fabs (G_CURR_ERB[j] - curr_erb[j]);
	    bincount++;
	}
    }
    amperror /= bincount;
    if ((amperror > 0.47) && (target1 > -0.4))
	returnFlag = FALSE;	//Bumping up


    this->erb_inv (curr_erb, slot, mfreq);

    //Back up the lasterbE memory after power normalization
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0);
    this->to_erb (lasterbE);

    //Power Dequantization
    lastLgainE += PowerCB[POWER_IDX][0];
    lastHgainE += PowerCB[POWER_IDX][1];

    target1 = pow (double (10.0), double (lastLgainE)) / lag;

    assert (target1 >= 0.0);
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, target1);

    target2 = pow (double (10.0), double (lastHgainE)) / lag;

    assert (target2 >= 0.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, target2);

    return returnFlag;
}

void
DTFS::dequant_cw (int pl, int &POWER_IDX, int *AMP_IDX, float &lastLgainD,
		  float &lastHgainD, float *lasterbD)
{
    //extern float lastLgainD, lastHgainD, lasterbD[NUM_ERB];
    float tmp, mfreq[NUM_ERB], curr_erb[NUM_ERB];
    int slot[NUM_ERB];

    //Amplitude Dequantization
    erb_add (curr_erb, lag, lasterbD, pl, AMP_IDX);
    curr_erb[0] = curr_erb[1] * 0.3;
    curr_erb[20] = curr_erb[19] * 0.3;
    curr_erb[21] = 0.0;
    erb_slot (lag, slot, mfreq);


    this->erb_inv (curr_erb, slot, mfreq);

    //Back up the lasterbD memory after power normalization
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0);
    this->to_erb (lasterbD);


    //Power Dequantization
    lastLgainD += PowerCB[POWER_IDX][0];
    lastHgainD += PowerCB[POWER_IDX][1];


    tmp = pow (double (10.0), double (lastLgainD)) / lag;

    assert (tmp >= 0.0);
    this->setEngyHarm (92.0, 1104.5, 0.0, 1104.5, tmp);

    tmp = pow (double (10.0), double (lastHgainD)) / lag;

    assert (tmp >= 0.0);
    this->setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, tmp);
}

void
timeadjust (const float *in, int M1, float *out, int M2)
//Truncate or padd zero in an array (time-domain prototype)
{
    int i, j, k, diff = abs (M1 - M2), lowestindex = -1;
    float tmp[130], sum, lowestsum = HUGE_VAL;

    if (M1 == M2) {
	for (i = 0; i < M1; i++)
	    out[i] = in[i];
	return;
    }

    for (i = 0; i < M1; i++)
	tmp[i] = in[i] * in[i];

    for (i = 0; i < M1; i++) {
	sum = 0.0;
	for (j = i - diff; j <= i + diff; j++)
	    sum += tmp[(j + M1) % M1];
	if (sum < lowestsum) {
	    lowestindex = i;
	    lowestsum = sum;
	}
    }

    if (M1 > M2) {
	for (i = 0; i < M1; i++)
	    tmp[i] = 1.0;
	for (i = lowestindex - diff / 2; i < lowestindex - diff / 2 + diff;
	     i++)
	    tmp[(i + M1) % M1] = 0.0;
	for (i = j = 0; i < M1; i++)
	    if (tmp[i] == 1.0)
		out[j++] = in[i];
    }
    else if (M1 < M2) {
	for (i = 0; i < M1; i++)
	    tmp[i] = in[i];
	for (i = j = 0; i < lowestindex; i++)
	    out[j++] = tmp[i];
	for (k = 0; k < diff; k++)
	    out[j++] = 0.0;
	for (; i < M1; i++)
	    out[j++] = tmp[i];
    }
}

void
DTFS::sortamp (float *outfreq, int M)
//Amplitude sorting
//Return the frequency locations of the M harmonics (the outfreq array)
{
    int i, j, nH = lag >> 1, count;
    float freq[65], *tamp, tmp, diff = 8000.0 / lag;
    DTFS X1 = *this;

    X1.car2pol ();
    tamp = X1.a;

    for (i = 0, tmp = 0.0; i <= nH; i++, tmp += diff)
	freq[i] = tmp;

    // Sort by the amplitudes (descending order)
    for (i = 0; i < nH; i++)
	for (j = i + 1; j <= nH; j++)
	    if (tamp[j] > tamp[i]) {
		tmp = tamp[i];
		tamp[i] = tamp[j];
		tamp[j] = tmp;
		tmp = freq[i];
		freq[i] = freq[j];
		freq[j] = tmp;
	    }

    // Handling 0 to 1k. Putting 4 bands
    outfreq[0] = 200;
    outfreq[1] = 400;
    outfreq[2] = 600;
    outfreq[3] = 800;
    outfreq[4] = 1000;
    for (i = 0; i <= nH; i++) {	// Blocking out harms <= 1000 Hz
	if (freq[i] <= 1000)
	    tamp[i] = -1;
    }

    // Filling the outfreq array.  At the same time, blocking
    // out the surrounding harmonics.
    for (i = 0, count = 5; count < M && i <= nH; i++) {
	if (tamp[i] >= 0.0) {
	    outfreq[count++] = freq[i];
	    //Variable bandwidth for the low, medium and high bands.
	    if (freq[i] < 1000.0)
		tmp = 60.0;
	    else if (freq[i] < 2000.0)
		tmp = 150.0;	//130
	    else
		tmp = 181.0;
	    for (j = i + 1; j <= nH; j++) {
		if (fabs (freq[j] - freq[i]) < tmp)
		    tamp[j] = -1.0;	//This is the blocking!
	    }
	}
    }

    for (; count < M; count++)
	outfreq[count] = 4000;

    //Sorting by frequency (Hz) now in ascending order.
    for (i = 0; i < M - 1; i++)
	for (j = i + 1; j < M; j++)
	    if (outfreq[j] < outfreq[i]) {
		tmp = outfreq[i];
		outfreq[i] = outfreq[j];
		outfreq[j] = tmp;
	    }
}

void
DTFS::phaseShift_band (float ph, float lband, float hband)
  // ph is the amount of phase shift (between 0 and 2pi)
  // + => shift to the left; - => shift to the right
{
    int k;
    float tmp, tmp2 = 0.0;
    double fdiff, freq;

    if (lband == hband)
	return;
    assert (lband < hband);
    if (hband == 4000.0)
	hband = 4001.0;		//avoid numerical errors

    fdiff = 8000.0 / lag;
    for (k = 0, freq = 0.0; k <= lag >> 1; k++, tmp2 += ph, freq += fdiff) {
	if (freq < hband && freq >= lband) {
	    tmp = a[k];
	    a[k] = tmp * cos (tmp2) - b[k] * sin (tmp2);
	    b[k] = tmp * sin (tmp2) + b[k] * cos (tmp2);
	}
    }
}

float
DTFS::alignment_band (DTFS X2, float Eshift, float lband, float hband,
		      float range, float diff)
{				//Eshift is w.r.t  X2
    //range: alignment range, [0,1]
    //diff: alignment resolution in samples
    int k, flag = 0, idx, fshift;
    float maxcorr, corr, tmp, tmp1, n, Adiff;
    double fdiff, freq;
    DTFS X1;

    if (lband == hband)
	return 9999.0;
    assert (lband < hband);
    if (hband == 4000.0)
	hband = 4001.0;		//avoid numerical errors

    X1 = *this;
    assert (X1.lag == X2.lag);

    maxcorr = -HUGE_VAL;
    Adiff = X2.lag / 2.0 * range;
    fdiff = 8000.0 / X2.lag;

    for (idx = 0, n = Eshift - Adiff; n < Eshift + Adiff; n += diff, idx++) {
	corr = tmp = freq = 0.0;
	tmp1 = TWO_PI * n / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1, freq += fdiff) {
	    if (freq < hband && freq >= lband) {
		flag = 1;
		corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
		corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	    }
	}
	if ((flag == 1) && (corr > maxcorr)) {
	    fshift = idx;
	    maxcorr = corr;
	}
    }
    if (flag == 0)
	return 9999.0;
    else
	return (float) fshift;
}

void
numHarmPerBin (int *out, int lag, const int *FREQ, int M)
{
    int j, k;
    double fdiff, freq, lband, hband;

    for (j = 0; j < M; j++)
	out[j] = 0;

    fdiff = 8000.0 / lag;
    freq = 0.0;

    for (k = 0; k <= lag >> 1; k++, freq += fdiff) {
	for (j = 0; j < M; j++) {
	    lband = (float) FREQ[j];
	    hband = (float) FREQ[j + 1];
	    if (hband == 4000.0)
		hband = 4001.0;
	    if (freq < hband && freq >= lband) {
		out[j]++;
		j = M;
	    }
	}
    }
}

unsigned int
DTFS::align_band_FPPP (DTFS X2, float Eshift, float lband, float hband,
		       float samp_resol, int num_steps)
{				//Eshift is w.r.t  X2
    //range: alignment range, [0,1]
    //diff: alignment resolution in samples
    unsigned int idx = 0, n = 0;
    int k, flag = 0;
    float maxcorr = -HUGE_VAL, corr, tmp, tmp1, shift, Adiff;
    double fdiff, freq;
    DTFS X1;

    if (lband == hband)
	return 9999;
    assert (lband < hband);
    if (hband == 4000.0)
	hband = 4001.0;		//avoid numerical errors

    X1 = *this;
    assert (X1.lag == X2.lag);

    fdiff = 8000.0 / X2.lag;

    shift = samp_resol * num_steps / 2;

    for (n = 0; n < num_steps; n++) {
	corr = tmp = freq = 0.0;
	tmp1 = TWO_PI * (Eshift - shift + samp_resol * n) / X2.lag;
	for (k = 0; k <= X2.lag >> 1; k++, tmp += tmp1, freq += fdiff) {
	    if (freq < hband && freq >= lband) {
		flag = 1;
		corr += (X1.a[k] * X2.a[k] + X1.b[k] * X2.b[k]) * cos (tmp);
		corr += (X1.b[k] * X2.a[k] - X1.a[k] * X2.b[k]) * sin (tmp);
	    }
	}
	if ((flag == 1) && (corr > maxcorr)) {
	    idx = n;
	    maxcorr = corr;
	}
    }
    if (flag == 0)
	return 9999;
    else
	return idx;
}

#define MAX_CB_DIM 10
int
DTFS::alignment_CB (DTFS X2, float Eshift, const float *border, double *CB,
		    int DIM, int SIZE, float *snr)
{				//Eshift is w.r.t  X2
    int i, k, n, bestentry = -1;
    float en1, en2, maxcorr, corr, tmp[MAX_CB_DIM], tmp1[MAX_CB_DIM], fdiff;
    double freq;
    DTFS X1;

    assert (DIM <= MAX_CB_DIM);

    X1 = *this;

    maxcorr = -HUGE_VAL;
    fdiff = 8000.0 / X2.lag;

    for (n = 0; n < SIZE; n++) {
	corr = freq = 0.0;
	for (i = 0; i < DIM; i++) {
	    tmp[i] = 0.0;
	    tmp1[i] = TWO_PI * Eshift / X2.lag + CB[n * DIM + i];
	}
	for (k = 0; k <= X2.lag >> 1; k++, freq += fdiff) {
	    for (i = 0; i < DIM; i++) {
		if (freq < border[i + 1] && freq >= border[i]) {
		    corr +=
			(X1.a[k] * X2.a[k] +
			 X1.b[k] * X2.b[k]) * cos (tmp[i]);
		    corr +=
			(X1.b[k] * X2.a[k] -
			 X1.a[k] * X2.b[k]) * sin (tmp[i]);
		}
		tmp[i] += tmp1[i];
	    }
	}
	if (corr > maxcorr) {
	    bestentry = n;
	    maxcorr = corr;
	}
    }
    //Assure a codebook entry has been chosen
    assert (bestentry >= 0);

    for (freq = en1 = en2 = k = 0; k <= X2.lag >> 1; k++, freq += fdiff) {
	for (i = 0; i < DIM; i++) {
	    if (freq < border[i + 1] && freq >= border[i]) {
		en1 += X1.a[k] * X1.a[k] + X1.b[k] * X1.b[k];
		en2 += X2.a[k] * X2.a[k] + X2.b[k] * X2.b[k];
	    }
	}
    }

    *snr = 10.0 * log10 (1.0 / (1.0 - maxcorr * maxcorr / en1 / en2));

    return bestentry;
}

void
WIsyn (DTFS prevCW, DTFS * curr_cw, const float *prev_lsp,
       const float *curr_lsp, float *ph_offset, float *out, int N)
{
#define SCR_PITCH_THRESHLD -1

    int i, j;
    unsigned short I = 1, flag = 0;
    float scr = 0.0, w, alignment, tmp, *phase = new float[N];
    DTFS currCW = *curr_cw;

    //Calculating the expected alignment shift
    alignment = *ph_offset / TWO_PI * prevCW.lag;
    if (flag == 1)
	alignment *= I;

    //Calculating the expected alignment shift
    tmp =
	fmod ((N % ((prevCW.lag + currCW.lag) >> 1) + alignment), currCW.lag);

    // Compute the alignment shift
    //alignment = prevCW.alignment_weight(currCW, tmp, prev_lsp, curr_lsp) ;
    alignment = prevCW.alignment_weight (currCW, tmp, curr_lsp, curr_lsp);

    tmp = TWO_PI * alignment / currCW.lag;
    currCW.phaseShift (tmp);
    curr_cw->phaseShift (TWO_PI * alignment / curr_cw->lag);

    // Compute the cubic phase track and transform to 1-D signal
    cubicPhase (*ph_offset, tmp, prevCW.lag, currCW.lag, N, phase);


    int num_intp_CWs =
	(int) (anint ((float) N / ((prevCW.lag + currCW.lag) >> 1)));
    if ((prevCW.lag <= SCR_PITCH_THRESHLD)
	&& (currCW.lag <= SCR_PITCH_THRESHLD)) {
	// Interpolation and transformation from 2-D to 1-D


	DTFS *t1CW = new DTFS[num_intp_CWs + 1];
	int step, past;

	t1CW[0] = prevCW;
	for (j = 1; j <= num_intp_CWs; j++) {
	    w = (float) j / num_intp_CWs;
	    t1CW[j] = prevCW * (1.0 - w) + currCW * w;
	}

	for (i = 1; i < num_intp_CWs; i++)
	    t1CW[i].injectnoise (scr);

	for (i = 0, past = 0, j = num_intp_CWs; i < num_intp_CWs; i++, j--) {
	    step = (N - past) / j;
	    t1CW[i].transform (t1CW[i + 1], phase + past, out + past, step);
	    past += step;
	}
	delete[]t1CW;
    }
    else
	prevCW.transform (currCW, phase, out, N);

    // Adjust the phase offset and wrap it between 0 and 2pi
    if (flag == 2)
	tmp *= I;
    *ph_offset = fmod (double (tmp), double (TWO_PI));

    delete[]phase;
}

#define CUTFREE_ABS_RANGE 6
#define CUTFREE_REL_RANGE 0.25
void
ppp_extract_pitch_period (const float *in, float *out, int l)
{
    int i;
    int spike = 0;
    float max = 0.0;
    const float *ptr = in + FSIZE - l;
    float en1 = 0.0, en2 = 0.0, tmp;

    float x;

    for (i = 0; i < l; i++) {
	if ((x = fabs (ptr[i])) > max) {
	    max = x;
	    spike = i;
	}
	en1 += ptr[i] * ptr[i];
    }
    int range = (int) anint (MAX (CUTFREE_REL_RANGE * l, CUTFREE_ABS_RANGE));

    if (spike - range < 0) {
	for (i = 0; i < l + (spike - range); i++)
	    out[i] = ptr[i];
	// Grab Remaining From One Lag Before
	ptr -= l;
	for (; i < l; i++)
	    out[i] = ptr[i];
    }
    else if (spike + range >= l) {
	for (i = 0; i < spike - range; i++)
	    out[i] = ptr[i];
	// Grab Remaining From One Lag Before
	if (ptr - l + i >= in)
	    for (ptr -= l; i < l; i++)
		out[i] = ptr[i];
	else
	    for (; i < l; i++)
		out[i] = ptr[i];
    }
    else
	for (i = 0; i < l; i++)
	    out[i] = ptr[i];

    //Energy adjustment added to eliminate artifacts at the end of voicing
    for (i = 0; i < l; i++)
	en2 += out[i] * out[i];
    if (en1 < en2) {
	tmp = sqrt (en1 / en2);
	for (i = 0; i < l; i++)
	    out[i] *= tmp;
    }
}

void
DTFS::peaktoaverage (float *pos, float *neg)
{
    float tmp, time[DMAX], sum = 0.0, maxPosEn = 0.0, maxNegEn = 0.0;
    int i;
    DTFS TMP = *this;

    TMP.fs_inv (time, TMP.lag, 0.0);
    for (i = 0; i < TMP.lag; i++) {
	tmp = SQR (time[i]);
	if (time[i] >= 0) {
	    if (tmp > maxPosEn)
		maxPosEn = tmp;
	}
	else {
	    if (tmp > maxNegEn)
		maxNegEn = tmp;
	}
	sum += tmp;
    }
    if (sum == 0.0)
	*pos = *neg = 0.0;
    else {
	*pos = sqrt (maxPosEn * TMP.lag / sum);
	*neg = sqrt (maxNegEn * TMP.lag / sum);
    }
}
