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


#include <math.h>
#include <stdio.h>
#include "proto_new.h"
#include "gen_shaped_noise.h"
#include "filt.h"
#include "struct.h"
#define TMP_ORD 4
#include "proto.h"
static float rhpf_num_coef[2] = { 1, -0.97 };
static float rhpf_den_coef[3] = { 1.0000, -0.5500, -0.1500 };


#define MOT_PPF_DEBUG 0


#define SCALE_MEMSIZE 2

void
generate_pulses (float *whtnd_la, float *exc, struct GEN_PULSES *f)
{


    register float *t;
    float out[SCALE_MEMSIZE * LB_FRAMESIZE];
    float out1[SCALE_MEMSIZE * LB_FRAMESIZE];


    t = exc;
    int i;

    POLEZERO_FILTER rhpf_filter = { 2, 1, 2, 0, f->rhpf_filt_mem };

    Interpolate_allpass_steep (whtnd_la, f->mem1, LB_FRAMESIZE, out);	/* array of size 2*LB_FRAMESIZE */

    /* non-linearity */
    for (i = 0; i < SCALE_MEMSIZE * LB_FRAMESIZE; i++)
	out1[i] = fabs (out[i]);

    polezero_filter (out1, out, 2 * LB_FRAMESIZE, rhpf_num_coef,
		     rhpf_den_coef, rhpf_filter);

    // inp , op
    for (i = 0; i < 2 * LB_FRAMESIZE; i++)
	*t++ = out[i];

}




void
FGV_MEM::gen_shaped_noise (float *in, short voiced, float noise_gain,
			   float *lpc, struct GEN_SHP_NOISE *f, float *noise)
{
    int i, j, k, lpc_win_len;
    float out[2 * LB_FRAMESIZE];
    float out1[2 * LB_FRAMESIZE];
    float pc[4], rc[4], R[11], win[UB_FRAMESIZE + 4],
	gen_op[2 * LB_FRAMESIZE];
    POLEZERO_FILTER rapf_filter = { 18, 18, 18, 0, f->rapf_filt_mem };
    double zr[4] = { 0, 0, 0, 0 };	//,zr1[4]={0,0,0,0};
    float *buf_tmp = new float[4 * LB_FRAMESIZE + 24];

    float coef[TMP_ORD], alfa;

    float out2[2 * LB_FRAMESIZE];
    float ZRmem[TMP_ORD] = { 0, 0, 0, 0 };
    float ZRmem1[6] = { 0, 0, 0, 0, 0, 0 };

    POLEZERO_FILTER rlpf_filter = { 1, 1, 1, 0, &zr[0] };
    POLEZERO_FILTER rlpf6_filter = { 4, 4, 4, 0, &zr[0] };
    register float *t;

    t = noise;

    polezero_filter (in, out, LB_FRAMESIZE, AP_NR, AP_DR, rapf_filter);
    // inp , op

    if (voiced == 1)
	generate_pulses (in, gen_op, f->gen_pulses_dec);	//passing in the addr of a sub-structure
    else
	generate_pulses (out, gen_op, f->gen_pulses_dec);	//passing in the addr of a sub-structure

    /* upsample to 32 kHz */
    Interpolate_allpass (gen_op, f->res_down_dec->memup, 2 * LB_FRAMESIZE,
			 buf_tmp + LEN_DN_HB - 2);
    /* resample to 28 kHz */
    for (k = 0; k < LEN_DN_HB - 2; k++) {
	buf_tmp[k] = f->res_down_dec->state[k];
	f->res_down_dec->state[k] = buf_tmp[k + 4 * LB_FRAMESIZE];
    }
    resample_down_HB (buf_tmp, buf_tmp, 4 * LB_FRAMESIZE);
    /* downsample to 14 kHz */
    Decimate_allpass_steep (buf_tmp, f->res_down_dec->memdown,
			    7 * LB_FRAMESIZE / 2, out);
    /* flip spectrum */
    for (k = 1; k < 7 * LB_FRAMESIZE / 4; k += 2)
	out[k] = -out[k];
    /* downsample to 7 kHz */
    Decimate_allpass_steep (out, f->memdown1, 7 * LB_FRAMESIZE / 4, out1);

    /* now a 7K signal */
    /* low -pass filter */
    polezero_filter (out1, out, LB_FRAMESIZE * 7 / 8, LP_NR, LP_DR,
		     rlpf_filter);
    //inp,op
    /* initialize filter mem */
    for (i = 0; i < 4; i++)
	rlpf6_filter.mem[i] = 0;

    for (i = 0; i < 4; i++)
	out1[i] = f->stateExc[i];
    for (i = 0; i < LB_FRAMESIZE * 7 / 8; i++)
	out1[i + 4] = out[i];

    lpc_win_len = UB_FRAMESIZE + 4;
    for (i = 0; i < lpc_win_len; i++)
	win[i] = 1.0;

    // lpcanalys_ExtWin (pc, rc, out1, TMP_ORD, UB_FRAMESIZE + 4, R, win);

    lpcanalys (pc, rc, out1, TMP_ORD, UB_FRAMESIZE + 4, R, win);

    for (i = 0; i < 4; i++)
	f->stateExc[i] = out1[LB_FRAMESIZE * 7 / 8 + i];

    for (i = 0; i < TMP_ORD; i++)
	coef[i] = pc[i] * pow (0.6, i + 1);

    fir (out, out1, coef, ZRmem, TMP_ORD, UB_FRAMESIZE + 4);
    // o/p , inp, out is excw


    /* out+4 = input to additive_noise and out1 is the output */
    /* additive noise code */
    for (i = 0; i < UB_FRAMESIZE; i++)
	out2[i] = out[i + 4] * out[i + 4] * 0.1;

    iir (out1, out2, NF, f->noise_iir, 1, UB_FRAMESIZE);
    //o/p, inp

    alfa = 0.08 + noise_gain * 0.2;

    for (i = 0; i < UB_FRAMESIZE; i++) {
	out1[i] = sqrt (out1[i]) * ran_g (f->Seed);	// read from file and passed into fn
	//printf("%f\n",tt);

    }
    for (i = 0; i < UB_FRAMESIZE; i++)
	out[i] = (sqrt (1 - (alfa * alfa)) * out[i + 4]) + (alfa * out1[i]);	//excw


    for (i = 0; i < 54; i++)
	out1[i] = f->stateExcW[i];
    for (i = 0; i < UB_FRAMESIZE; i++)
	out1[i + 54] = out[i];	//excw_buf

    for (i = 0; i < 54; i++)
	f->stateExcW[i] = out1[LB_FRAMESIZE * 7 / 8 + i];

    polezero_filter (out1, out, UB_FRAMESIZE + 54, LP_NR6, LP_DR6, rlpf6_filter);	//test
    //inp,op

    SynthesisFilter (out2, out, lpc, ZRmem1, LPC_ORD_WB, UB_FRAMESIZE + 54);
    //op,inp
    for (i = 40; i < UB_FRAMESIZE + 54; i++)
	*t++ = out2[i];

    delete[]buf_tmp;
}
