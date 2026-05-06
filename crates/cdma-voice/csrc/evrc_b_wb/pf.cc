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
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


/*===========================================================================*/
/*  Lucent Technologies Network Wireless Systems                             */
/*                                                                           */
/*  Copyright (C) 1999 Lucent Technologies.  All rights reserved.            */
/*---------------------------------------------------------------------------*/

/*************************************************************************
 *
 *  FILE NAME:   postfilter.c
 *
 * Performs adaptive postfiltering on the synthesis speech
 *
 *  FUNCTIONS INCLUDED:  Init_Post_Filter()  and Post_Filter()
 *
 *************************************************************************/

#include "typedef.h"
#include "macro.h"
#include "proto.h"
#include "struct.h"

#define L_H       22		/* size of truncated impulse response of A(z/g1)/A(z/g2) */
#define  MU       0.80		/* 0.8 Factor for tilt compensation filter        */

/*------------------------------------------------------------*
 *   static vectors                                           *
 *------------------------------------------------------------*/



/*************************************************************************
 *
 *  FUNCTION:   Init_Post_Filter
 *
 *  PURPOSE: Initializes the postfilter parameters.
 *
 *************************************************************************/

void
FGV_MEM::Init_Post_Filter (void)
{
    int16 i;

    for (i = 0; i < ORDER; i++) {
	PF_mem_syn_pst[i] = 0;
	FIRmem[i] = 0;
    }
    if (data_packet.WB_MODE_BIT == 1) {
    }

    return;
}

void
FGV_MEM::preemphasis (float *signal,	/* (i/o) : input signal overwritten by the output */
		      float g,	/* (i)   : preemphasis coefficient                */
		      int16 L	/* (i)   : size of filtering                      */
    )
{
    // static float mem_pre = 0;
    float *p1, *p2, temp;
    int16 i;

    p1 = signal + L - 1;
    p2 = p1 - 1;
    temp = *p1;

    for (i = 0; i <= L - 2; i++) {
	*p1 -= g * *p2--;
	p1--;
    }

    *p1 -= g * mem_pre;

    mem_pre = temp;

    return;
}

/*************************************************************************
 *  FUNCTION:  Post_Filter()
 *
 *  PURPOSE:  postfiltering of synthesis speech.
 *
 *  DESCRIPTION:
 *      The postfiltering process is described as follows:
 *
 *          - inverse filtering of syn[] through A(z/GAMMA3) to get res2[]
 *          - tilt compensation filtering; 1 - MU*k*z^-1
 *          - synthesis filtering through 1/A(z/GAMMA4)
 *          - adaptive gain control
 *
 *************************************************************************/

void
FGV_MEM::Post_Filter (float *syn,	/* in/out: synthesis speech (postfiltered is output)    */
		      float *Lsp, float *Az,	/* input : interpolated LPC parameters in all subframes */
		      float *synpf,
		      float delayi, float agc_fac, float mu, int16 l_subfr)
{
 /*-------------------------------------------------------------------*
  *           Decleration of parameters                               *
  *-------------------------------------------------------------------*/

    //Time-Warping: Increased size of res2 below
    float res2[SubFrameSize * 2];	/* inverse filtered synthesis */

    //Time-Warping: Increased size of syn_pst below
    float syn_pst[SubFrameSize * 2];	/* post filtered synthesis speech   */

    float Ap3[ORDER], Ap4[ORDER];	/* bandwidth expanded LP parameters */
    float Lsp3[ORDER], Lsp4[ORDER];
    float lsp_ref[ORDER];

    float h[L_H];
    float tilt;

    float mem[ORDER];
    int16 i;
    float tmp1, tmp2;

    //static short post_filter_first_time = 1;

    if (post_filter_first_time) {
	Init_Post_Filter ();
	post_filter_first_time = 0;
    }


   /*-----------------------------------------------------*
    * Post filtering                                      *
    *-----------------------------------------------------*/

    tmp1 = syn[0] * syn[0];
    for (i = 1; i < l_subfr; i++)
	tmp1 += syn[i] * syn[i];

    tmp2 = syn[0] * syn[1];
    for (i = 1; i < l_subfr - 1; i++)
	tmp2 += syn[i] * syn[i + 1];

    Ap3[0] = -tmp2 / tmp1;
    for (i = 1; i < ORDER; i++)
	Ap3[i] = 0;


    for (i = 0; i < l_subfr; i++)
	res2[i] = syn[i];

    a2lsp (lsp_ref, Ap3, ORDER);


    /* PF1  */
    for (i = 0; i < 5; i++) {
	tmp1 = 0.30 + 0.04 * i;
	Lsp3[i] = tmp1 * lsp_ref[i] + (1.0 - tmp1) * Lsp[i];
    }
    for (i = 5; i < ORDER; i++) {
	tmp1 = 0.50;
	Lsp3[i] = tmp1 * lsp_ref[i] + (1.0 - tmp1) * Lsp[i];
    }

    for (i = 0; i < ORDER; i++) {
	tmp1 = 0.3;
	Lsp4[i] = tmp1 * lsp_ref[i] + (1.0 - tmp1) * Lsp[i];
    }

    lsp2a (Ap3, Lsp3, ORDER);
    lsp2a (Ap4, Lsp4, ORDER);

    /* filtering of synthesis speech by A(z/GAMMA3) to find res2[] */

    fir (res2, res2, Ap3, FIRmem, ORDER, l_subfr);

    /* long term filtering */
    ltf (res2, delayi, agc_fac, l_subfr);
    if (data_packet.WB_MODE_BIT == 1) {
    }
    /* filtering through  1/A(z/GAMMA4) */

    for (i = 0; i < ORDER; i++)
	mem[i] = PF_mem_syn_pst[i];

    iir (syn_pst, res2, Ap4, mem, ORDER, l_subfr);

    /* scale output to input */

    agc (syn, syn_pst, res2, 0.0, l_subfr);

    iir (syn_pst, res2, Ap4, PF_mem_syn_pst, ORDER, l_subfr);

    /* overwrite synthesis speech by postfiltered synthesis speech */

    for (i = 0; i < l_subfr; i++)
	synpf[i] = syn_pst[i];

    return;
}


void
FGV_MEM::agc (float *sig_in,	/* (i)     : postfilter input signal  */
	      float *sig_out,	/* (i)     : postfilter output signal */
	      float *res_out, float agc_fac,	/* (i)     : AGC factor               */
	      int16 l_trm	/* (i)     : subframe size            */
    )
{
    int16 i;
    float gain_in, gain_out, g0, gain;
    float s;

    //static float past_gain = 1.0; 

    /* calculate gain_out with exponent */

    s = sig_out[0] * sig_out[0];
    for (i = 1; i < l_trm; i++) {
	s += sig_out[i] * sig_out[i];
    }

    if (s == 0) {
	past_gain = 0;
	return;
    }
    gain_out = s;

    /* calculate gain_in with exponent */

    s = sig_in[0] * sig_in[0];
    for (i = 1; i < l_trm; i++) {
	s += sig_in[i] * sig_in[i];
    }

    if (s == 0) {
	g0 = 0;
    }
    else {
	gain_in = s;

   /*---------------------------------------------------*
    *  g0 = (1-agc_fac) * sqrt(gain_in/gain_out);       *
    *---------------------------------------------------*/

	s = gain_out / gain_in;
	s = 1.0 / sqrt (s);

	g0 = s * (1.0 - agc_fac);
    }

    /* compute gain(n) = agc_fac gain(n-1) + (1-agc_fac)gain_in/gain_out */
    /* sig_out(n) = gain(n) sig_out(n)                                   */

    gain = past_gain;
    for (i = 0; i < l_trm; i++) {
	gain *= agc_fac;
	gain += g0;
	res_out[i] *= gain;
    }
    past_gain = gain;

    return;
}


void
FGV_MEM::ltf (float *res, float delayi, float ltgain, short len)
{
    // static float Residual[ACBMemSize + SubFrameSize];
    short i, j, n;
    float sum1, sum2, gamma;
    short best;

    for (i = 0; i < len; i++)
	Residual[ACBMemSize + i] = res[i];

    /* long term filtering */
    /* Find best integer delay around delayi */
    j = (short) (delayi + 0.5);
    sum1 = 0;
    best = j;
    for (i = Max (DMIN, j - 3); i <= Min (DMAX, j + 3); i++) {

	for (n = ACBMemSize, sum2 = 0; n < ACBMemSize + len; n++)
	    sum2 += Residual[n] * Residual[n - i];
	if (sum2 > sum1) {
	    sum1 = sum2;
	    best = i;
	}
    }

    /* Get beta for delayi */
    for (i = ACBMemSize, sum1 = 0; i < ACBMemSize + len; i++)
	sum1 += Residual[i - best] * Residual[i - best];
    for (i = ACBMemSize, sum2 = 0; i < ACBMemSize + len; i++)
	sum2 += Residual[i] * Residual[i - best];
    if (data_packet.WB_MODE_BIT == 1) {
	if (bit_rate != 4)	//not FCELP
	{
	    if (sum2 * sum1 != 0) {

		gamma = sum2 / sum1;
		if (gamma >= 0.5) {

		    if (gamma > 1.0)
			gamma = 1.0;	/* Clip gamma at 1.0 */

		    /* Do actual filtering */
		    for (i = 0; i < len; i++) {
			res[i] = Residual[ACBMemSize + i] +
			    gamma * ltgain * Residual[ACBMemSize + i - best];
		    }
		}
	    }
	}
    }
    else {
	if (sum2 * sum1 != 0) {

	    gamma = sum2 / sum1;
	    if (gamma >= 0.5) {

		if (gamma > 1.0)
		    gamma = 1.0;	/* Clip gamma at 1.0 */

		/* Do actual filtering */
		for (i = 0; i < len; i++) {
		    res[i] = Residual[ACBMemSize + i] +
			gamma * ltgain * Residual[ACBMemSize + i - best];
		}
	    }
	}

    }
    /* Update residual buffer */
    for (i = 0; i < ACBMemSize; i++)
	Residual[i] = Residual[i + len];
}


void
FGV_MEM::ltf_fir (float *Residual1,
		  float delayi, float ltgain, short len, float *out)
{

    short i, j, n;
    float sum1, sum2, gamma, alpha1;
    short best;

    /* long term filtering */
    /* Find best integer delay around delayi */
    j = (short) (delayi + 0.5);
    sum1 = 0;

    best = j;

    for (i = Max (DMIN, j - 3); i <= Min (DMAX, j + 3); i++) {

	for (n = ACBMemSize, sum2 = 0; n < ACBMemSize + len; n++)
	    sum2 += Residual1[n] * Residual1[n - i];
	if (sum2 > sum1) {
	    sum1 = sum2;
	    best = i;
	}
    }


    /* Get beta for delayi */
    for (i = ACBMemSize, sum1 = 0; i < ACBMemSize + len; i++)
	sum1 += Residual1[i - best] * Residual1[i - best];
    for (i = ACBMemSize, sum2 = 0; i < ACBMemSize + len; i++)
	sum2 += Residual1[i] * Residual1[i - best];

    if (sum2 * sum1 != 0) {

	gamma = sum2 / sum1;
	if (gamma >= 0.5) {

	    if (gamma > 1.0)
		gamma = 1.0;	/* Clip gamma at 1.0 */


	    /* Do actual fir filtering */
	    for (i = 0; i < len; i++) {
		out[i] = Residual1[ACBMemSize + i] +
		    gamma * ltgain * Residual1[ACBMemSize + i - best];
	    }


	}
    }


}



void
FGV_MEM::agc_new (float *sig_in,	/* (i)     : postfilter input signal  */
		  float *sig_out,	/* (i)     : postfilter output signal */
		  int16 l_trm	/* (i)     : subframe size            */
    )
{
    int16 i;
    float gain_in, gain_out, g0, gain;
    float s;

    /* calculate gain_out with exponent */

    s = sig_out[0] * sig_out[0];
    for (i = 1; i < l_trm; i++) {
	s += sig_out[i] * sig_out[i];
    }

    if (s == 0) {
	return;
    }
    gain_out = s;

    /* calculate gain_in with exponent */

    s = sig_in[0] * sig_in[0];
    for (i = 1; i < l_trm; i++) {
	s += sig_in[i] * sig_in[i];
    }

    if (s == 0) {
	g0 = 0;
    }
    else {
	gain_in = s;

   /*---------------------------------------------------*
    *  g0 = (1-agc_fac) * sqrt(gain_in/gain_out);       *
    *---------------------------------------------------*/

	s = gain_out / gain_in;
	s = 1.0 / sqrt (s);

	g0 = s;
    }

    /* compute gain(n) = agc_fac gain(n-1) + (1-agc_fac)gain_in/gain_out */
    /* sig_out(n) = gain(n) sig_out(n)                                   */

    for (i = 0; i < l_trm; i++) {
	sig_out[i] = sig_out[i] * g0;
    }
    return;
}
