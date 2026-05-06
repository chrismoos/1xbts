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
    "$Id: filt.cc,v 1.11.6.3 2007/06/24 06:43:07 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


#include "defines.h"
#include "struct.h"
#include "filt.h"
#include "proto.h"

static float FR_INTERP_COEFF[8] = {
    -0.0073, 0.0322, -0.1363, 0.6076, 0.6076, -0.1363, 0.0322, -0.0073
};


void
zero_filter (float *in, float *out, int N, float *c, ZERO_FILTER & f)
{
    // Y(Z)=(b[0]+b[1]z^(-1)+....+b[L]z^(-L))X(Z)
    // y[n]=c[0]x[n]+c[1]mem[n-1]+....+c[L]mem[n-L], where c[i]=b[i]
    // mem={x[n-L] x[n-L+1] . . . . x[n-2] x[n-1]}
    int i, j;
    float *tmpbuf = new float[f.order + N];

    for (i = 0; i < f.order; i++)
	tmpbuf[i] = f.mem[i];
    for (i = 0; i < N; i++)
	tmpbuf[i + f.order] = in[i];
    for (i = 0; i < N; i++) {
	out[i] = 0.0;
	for (j = 0; j < f.order + 1; j++)
	    out[i] += c[f.order - j] * tmpbuf[i + j];
    }
    for (i = 0; i < f.order; i++)
	f.mem[i] = tmpbuf[i + N];
    delete[]tmpbuf;
}

void
pole_filter (float *in, float *out, int N, float *c, POLE_FILTER & f)
{
    // Y(Z)=X(Z)/(1+a[1]z^(-1)+....+a[M]z^(-M))
    // y[n]=x[n]+c[0]mem[n-1]+....+c[M-1]mem[n-M], where c[i]=-a[i+1]
    // mem={y[n-M] y[n-M+1] . . . . y[n-2] y[n-1]}
    int i, j;
    double *tmpbuf = new double[f.order + N];

    for (i = 0; i < f.order; i++)
	tmpbuf[i] = f.mem[i];
    for (i = 0; i < N; i++)
	tmpbuf[i + f.order] = in[i];
    for (i = 0; i < N; i++) {
	for (j = 0; j < f.order; j++)
	    tmpbuf[i + f.order] += c[f.order - 1 - j] * tmpbuf[i + j];
	out[i] = tmpbuf[i + f.order];
    }
    for (i = 0; i < f.order; i++)
	f.mem[i] = tmpbuf[i + N];
    delete[]tmpbuf;
}

void
polezero_filter (float *in, float *out, int N, float *b, float *a,
		 POLEZERO_FILTER & f)
{
    // Y(Z)=X(Z)(b[0]+b[1]z^(-1)+..+b[L]z^(-L))/(a[0]+a[1]z^(-1)+..+a[M]z^(-M))
    // mem[n]=x[n]+cp[0]mem[n-1]+..+cp[M-1]mem[n-M], where cp[i]=-a[i+1]/a[0]
    // y[n]=cz[0]mem[n]+cz[1]mem[n-1]+..+cz[L]mem[n-L], where cz[i]=b[i]/a[0]
    // mem={mem[n-K] mem[n-F+1] . . . . mem[n-2] mem[n-1]}, where K=max(L,M)
    int i, j;
    float *cp = new float[f.denord], *cz = new float[f.numord + 1];
    double tmp;

    for (i = 0; i < f.denord; i++)
	cp[i] = -a[i + 1] / a[0];
    for (i = 0; i <= f.numord; i++)
	cz[i] = b[i] / a[0];
    for (i = 0; i < N; i++) {

	double *ptr = f.mem + f.order - 1;

	tmp = *ptr + cz[0] * in[i];
	if (f.denord == f.numord) {
	    for (j = 1; j < f.order; j++, ptr--)
		*ptr = ptr[-1] + cz[j] * in[i] + cp[j - 1] * tmp;
	    *ptr = cz[j] * in[i] + cp[j - 1] * tmp;
	}
	else if (f.denord < f.numord) {
	    for (j = 1; j <= f.denord; j++, ptr--)
		*ptr = ptr[-1] + cz[j] * in[i] + cp[j - 1] * tmp;
	    for (; j < f.numord; j++, ptr--)
		*ptr = ptr[-1] + cz[j] * in[i];
	    *ptr = cz[j] * in[i];
	}
	else {
	    for (j = 1; j <= f.numord; j++, ptr--)
		*ptr = ptr[-1] + cz[j] * in[i] + cp[j - 1] * tmp;
	    for (; j < f.denord; j++, ptr--)
		*ptr = ptr[-1] + cp[j - 1] * tmp;
	    *ptr = cp[j - 1] * tmp;
	}
	out[i] = tmp;
    }
    delete[]cp;
    delete[]cz;
}

void
pitch_filter (float *in, float *out, int N, float b, float l,
	      PITCH_FILTER & f)
{
    int i, j, L;
    double *tmpbuf = new double[f.order + N];

    for (i = 0; i < f.order; i++)
	tmpbuf[i] = f.mem[i];
    for (i = 0; i < N; i++)
	tmpbuf[i + f.order] = in[i];
    if (l == (L = (int) floor (l))) {
	for (i = 0; i < N; i++)
	    out[i] = (tmpbuf[i + f.order] += b * tmpbuf[i + f.order - L]);
    }
    else {
	L += FR_INTERP_WIDTH / 2;
	for (i = 0; i < N; i++) {
	    out[i] = 0.0;
	    for (j = 0; j < FR_INTERP_WIDTH; j++)
		out[i] += tmpbuf[i + f.order - L + j] * FR_INTERP_COEFF[j];
	    out[i] = (tmpbuf[i + f.order] += b * out[i]);
	}
    }
    for (i = 0; i < f.order; i++)
	f.mem[i] = tmpbuf[i + N];
    delete[]tmpbuf;
}

void
circ_pole_filter (float *in, float *out, int N, float *c, int order)
{
    int i;
    POLE_FILTER f;
    float *v = new float[10 * N];

    f.order = order;
    f.mem = new double[order];

    for (i = 0; i < order; i++)
	f.mem[i] = 0.0;
    pole_filter (in, out, N, c, f);
    zir_p (v, 10 * N, c, f);
    for (i = 0; i < 10 * N; i++)
	out[i % N] += v[i];
    delete[]v;
    delete[]f.mem;
}

void
circ_polezero_filter (float *in, float *out, int N, float *a1, float *a2,
		      float *a3, int order)
{
    int i;
    POLE_FILTER f1;
    POLE_FILTER f3;
    ZERO_FILTER f2;
    float *v = new float[10 * N];

    f1.order = order;
    f1.mem = new double[order];

    for (i = 0; i < order; i++)
	f1.mem[i] = 0.0;
    f2.order = order;
    f2.mem = new float[order];

    for (i = 0; i < order; i++)
	f2.mem[i] = 0.0;
    f3.order = order;
    f3.mem = new double[order];

    for (i = 0; i < order; i++)
	f3.mem[i] = 0.0;

    pole_filter (in, out, N, a1, f1);
    zero_filter (out, out, N, a2, f2);
    pole_filter (out, out, N, a3, f3);
    zir_p (v, 10 * N, a1, f1);
    zero_filter (v, v, 10 * N, a2, f2);
    pole_filter (v, v, 10 * N, a3, f3);
    for (i = 0; i < 10 * N; i++)
	out[i % N] += v[i];
    delete[]v;
    delete[]f1.mem;
    delete[]f2.mem;
    delete[]f3.mem;
}

void
zsr_p (float *in, float *out, int N, float *c, int order)
{
    int i;
    POLE_FILTER f;

    f.order = order;
    f.mem = new double[order];

    for (i = 0; i < order; i++)
	f.mem[i] = 0.0;
    pole_filter (in, out, N, c, f);
    delete[]f.mem;
}

void
zir_p (float *out, int N, float *c, POLE_FILTER & f)
{
    int i;
    double *mem = new double[f.order];

    for (i = 0; i < f.order; i++)
	mem[i] = f.mem[i];
    for (i = 0; i < N; i++)
	out[i] = 0.0;
    pole_filter (out, out, N, c, f);
    for (i = 0; i < f.order; i++)
	f.mem[i] = mem[i];
    delete[]mem;
}

void
ir_p (float *out, int N, float *c, int order)
{
    int i;
    POLE_FILTER f;
    float *v = new float[N];

    f.order = order;
    f.mem = new double[order];

    for (i = 0; i < order; i++)
	f.mem[i] = 0.0;
    for (i = 1, v[0] = 1.0; i < N; i++)
	v[i] = 0.0;
    pole_filter (v, out, N, c, f);
    delete[]v;
    delete[]f.mem;
}

void
zir_pitch (float *out, int N, float b, float l, PITCH_FILTER & f)
{
    int i;
    double *mem = new double[f.order];

    for (i = 0; i < f.order; i++)
	mem[i] = f.mem[i];
    for (i = 0; i < N; i++)
	out[i] = 0.0;
    pitch_filter (out, out, N, b, l, f);
    for (i = 0; i < f.order; i++)
	f.mem[i] = mem[i];
    delete[]mem;
}

void
free_zero_filter (ZERO_FILTER & f)
{
    delete[]f.mem;
}

void
free_pole_filter (POLE_FILTER & f)
{
    delete[]f.mem;
}

void
free_polezero_filter (POLEZERO_FILTER & f)
{
    delete[]f.mem;
}

void
free_pitch_filter (PITCH_FILTER & f)
{
    delete[]f.mem;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include "macro.h"

/*----------------------------------------------------------------------*/
/*  Module:     iir and fir                                             */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

void
iir (float *output, float *input, float *coef, float *IIRmemory, short order,
     short length)
{

    register short i, j;
    float SUM;

    for (i = 0; i < length; i++) {
	SUM = input[i];
	for (j = order - 1; j > 0; j--) {
	    SUM -= coef[j] * IIRmemory[j];
	    IIRmemory[j] = IIRmemory[j - 1];
	}
	SUM -= coef[0] * IIRmemory[0];
	output[i] = IIRmemory[0] = SUM;
    }
}

void
fir (float *output, float *input, float *coef, float *FIRmemory, short order,
     short length)
{

    register short i, j;
    float SUM;

    for (i = 0; i < length; i++) {
	SUM = input[i];
	for (j = order - 1; j > 0; j--) {
	    SUM += coef[j] * FIRmemory[j];
	    FIRmemory[j] = FIRmemory[j - 1];
	}
	SUM += coef[0] * FIRmemory[0];
	FIRmemory[0] = input[i];
	output[i] = SUM;
    }
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     synthesis filter                                        */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     07/06/93  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: SynthesisFilter                                         *
* Function: Synthesis filter.                                           *
* Inputs: input - input excitation (or residual signal).                *
*         coef  - interpolated LPC coefficients.                        *
*         memory - filter's memory.                                     *
*         pcQ   - Q value of predicition coefficients.                  *
*         order - filter order.                                         *
*         length - input/output buffers length.                         *
* Output: output - filter output.                                       *
************************************************************************/

void
SynthesisFilter (float *output, float *input, float *coef, float *memory,
		 short order, short length)
{
    register short i, j;
    float acc;

    /* iir filter for each subframe */
    for (i = 0; i < length; i++) {
	for (j = order - 1, acc = *input++; j > 0; j--) {
	    acc -= coef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	acc -= coef[0] * memory[0];
	*output++ = acc;
	memory[0] = acc;
    }
}

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
/*  Module:     zeroinpt                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************************************
* Routine name: ZeroInput                                               *
* Function: Calculates zero input response.                             *
************************************************************************/
#include "globs.h"

void
FGV_MEM::ZeroInput (float *output, float *coef_uq, float *coef, float *input,
		    float gamma1, float gamma2, short order, short length,
		    short type)
{
    register short i;
    float memory[ORDER];	/* filter memory */
    float wcoef1[ORDER], wcoef2[ORDER];	/* weighting coeffs */

    /* primary copy of filter memory */
    //static float memA[ORDER], memA1[ORDER], memA2[ORDER];
    /* secondary copy of filter memory for half rate processing */
    //static float memB[ORDER], memB1[ORDER], memB2[ORDER];       

    // static int zeroInputFirstTime = 1;  /* init flag - sim only */

    /* initialization section -- should be in init routine for implementation */
    if (zeroInputFirstTime) {
	zeroInputFirstTime = 0;
	for (i = 0; i < order; i++) {
	    memA[i] = memA1[i] = memA2[i] = 0.0;
	    memB[i] = memB1[i] = memB2[i] = 0.0;
	}
    }

    weight (wcoef1, coef_uq, gamma1, order);
    if (data_packet.WB_MODE_BIT == 1) {
	weight (wcoef2, coef_uq, gamma2, order);
    }
    else {			//narrowband mode
	weight (wcoef2, coef_uq, gamma2, order);

    }
    if (type == 1) {
	/* Update filters memory */
	/* run synthesis filter 1/A[z] */
	iir (output, input, coef, memA, order, length);

	/* run A(z/g1) */
	fir (output, output, wcoef1, memA1, order, length);
	if (data_packet.WB_MODE_BIT == 1) {

	    /* run 1/1-DEMPH_FAC*z^-1 */
	    wcoef2[0] = DEEMPH_FAC;
	    iir (output, output, wcoef2, memA2, 1, length);
	}
	else {
	    /* run 1/A(z/g2) */
	    iir (output, output, wcoef2, memA2, order, length);

	}
	/* Synchronize the memory */
	for (i = 0; i < order; i++)
	    memB[i] = memA[i];
	for (i = 0; i < order; i++)
	    memB1[i] = memA1[i];
	for (i = 0; i < order; i++)
	    memB2[i] = memA2[i];
    }

    else {
	if (type == 0) {
	    /* Get zir */
	    /* ring ZIR to the future with zero input */
	    for (i = 0; i < length; i++)
		output[i] = 0.0;

	    /* run synthesis filter 1/A[z] */
	    for (i = 0; i < order; i++)
		memory[i] = memA[i];
	    iir (output, output, coef, memory, order, length);

	    /* run A(z/g1) */
	    for (i = 0; i < order; i++)
		memory[i] = memA1[i];
	    fir (output, output, wcoef1, memory, order, length);
	    if (data_packet.WB_MODE_BIT == 1) {
		wcoef2[0] = DEEMPH_FAC;
		for (i = 0; i < 1; i++)
		    memory[i] = memA2[i];
		iir (output, output, wcoef2, memory, 1, length);
	    }
	    else {

		/* run 1/A(z/g2) */
		for (i = 0; i < order; i++)
		    memory[i] = memA2[i];
		iir (output, output, wcoef2, memory, order, length);

	    }
	}

	else {			/* This is for half rate only */
	    /* Update filters memory */
	    /* run synthesis filter 1/A[z] */
	    iir (output, input, coef, memB, order, length);

	    /* run A(z/g1) */
	    fir (output, output, wcoef1, memB1, order, length);
	    if (data_packet.WB_MODE_BIT == 1) {
		wcoef2[0] = DEEMPH_FAC;
		for (i = 0; i < 1; i++)
		    memory[i] = memA2[i];
		iir (output, output, wcoef2, memory, 1, length);
	    }
	    else {		//narrowband case

		/* run 1/A(z/g2) */
		iir (output, output, wcoef2, memB2, order, length);

	    }
	}
    }
}
