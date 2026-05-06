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

#include "struct.h"
#include "filterbank.h"

static const float AP1_COURSE[1] = { 0.112108615 };
static const float AP2_COURSE[1] = { 0.540115985 };

static const float AP1[ALLPASSSECTIONS] =
    { 0.06262441299567, 0.49326511845632 };
static const float AP2[ALLPASSSECTIONS] =
    { 0.23754715248027, 0.80890715711734 };

static const float AP1_STEEP[ALLPASSSECTIONS_STEEP] =
    { 0.06056541924291, 0.42943401549235, 0.80873048306552 };
static const float AP2_STEEP[ALLPASSSECTIONS_STEEP] =
    { 0.22063024829630, 0.63593943961708, 0.94151583095682 };


void
FGV_MEM::filterbank_init_ana_lb (STATE_ANA_LB * State)
{
    int k;

    State->state_ana_filt_lb1.order = ANA_FILT_LEN_LB - 1;
    State->state_ana_filt_lb1.ptr = 0;
    State->state_ana_filt_lb1.mem = State->mem_ana_filt_lb1;
    State->state_ana_filt_lb2.order = ANA_FILT_LEN_LB - 1;
    State->state_ana_filt_lb2.ptr = 0;
    State->state_ana_filt_lb2.mem = State->mem_ana_filt_lb2;
    for (k = 0; k < ANA_FILT_LEN_LB - 1; k++) {
	State->mem_ana_filt_lb1[k] = 0.0;
	State->mem_ana_filt_lb2[k] = 0.0;
    }
}

void
FGV_MEM::filterbank_init_ana_hb (STATE_ANA_HB * State)
{
    int k;

    for (k = 0; k < 2 * ALLPASSSECTIONS; k++)
	State->state_ana_filt_hb_1[k] = 0.0;
    for (k = 0; k < LEN_DN_HB - 2; k++)
	State->state_ana_filt_hb_2[k] = 0.0;
    for (k = 0; k < 2 * ALLPASSSECTIONS_STEEP + 1; k++)
	State->state_ana_filt_hb_3[k] = 0.0;
    for (k = 0; k < 2 * ALLPASSSECTIONS_STEEP + 1; k++)
	State->state_ana_filt_hb_4[k] = 0.0;
    State->state_ana_filt_hb_iir.order = 1;
    State->state_ana_filt_hb_iir.numord = 1;
    State->state_ana_filt_hb_iir.denord = 1;
    State->state_ana_filt_hb_iir.ptr = 0;
    State->state_ana_filt_hb_iir.mem = State->mem_ana_filt_hb_iir;
    State->mem_ana_filt_hb_iir[0] = 0.0;
}

void
FGV_MEM::filterbank_init_syn_lb (STATE_SYN_LB * State)
{
    int k;

    for (k = 0; k < LB_SYN_DELAY; k++)
	State->buf_LB_delay[k] = 0;
    for (k = 0; k < SYN_FILT_LEN_LB - 1; k++)
	State->mem_syn_filt_lb[k] = 0.0;
    State->mem_syn_filt_lb_iir[0] = 0.0;
    State->mem_syn_filt_lb_iir[1] = 0.0;
    State->state_syn_filt_lb.order = SYN_FILT_LEN_LB - 1;
    State->state_syn_filt_lb.ptr = 0;
    State->state_syn_filt_lb.mem = State->mem_syn_filt_lb;
    State->state_syn_filt_lb_iir.order = 2;
    State->state_syn_filt_lb_iir.numord = 2;
    State->state_syn_filt_lb_iir.denord = 2;
    State->state_syn_filt_lb_iir.ptr = 0;
    State->state_syn_filt_lb_iir.mem = State->mem_syn_filt_lb_iir;
}

void
FGV_MEM::filterbank_init_syn_hb (STATE_SYN_HB * State)
{
    int k;

    State->mem_syn_filt_hb_iir1[0] = 0.0;
    State->mem_syn_filt_hb_iir1[1] = 0.0;
    State->mem_syn_filt_hb_iir2[0] = 0.0;
    State->mem_syn_filt_hb_iir2[1] = 0.0;
    for (k = 0; k < 2 * ALLPASSSECTIONS_STEEP; k++)
	State->state_syn_filt_hb_0[k] = 0.0;
    for (k = 0; k < 2 * ALLPASSSECTIONS_STEEP; k++)
	State->state_syn_filt_hb_1[k] = 0.0;
    for (k = 0; k < LEN_UP_HB; k++)
	State->state_syn_filt_hb_2[k] = 0.0;
    for (k = 0; k < 2 * ALLPASSSECTIONS + 1; k++)
	State->state_syn_filt_hb_3[k] = 0.0;
    State->state_syn_filt_hb_iir1.order = 2;
    State->state_syn_filt_hb_iir1.numord = 2;
    State->state_syn_filt_hb_iir1.denord = 2;
    State->state_syn_filt_hb_iir1.ptr = 0;
    State->state_syn_filt_hb_iir1.mem = State->mem_syn_filt_hb_iir1;
    State->state_syn_filt_hb_iir2.order = 2;
    State->state_syn_filt_hb_iir2.numord = 2;
    State->state_syn_filt_hb_iir2.denord = 2;
    State->state_syn_filt_hb_iir2.ptr = 0;
    State->state_syn_filt_hb_iir2.mem = State->mem_syn_filt_hb_iir2;
}


void
FGV_MEM::filterbank_ana_lb (float *out, const float *in, int16 len,	/* length of input array */
			    STATE_ANA_LB * State)
{
    int k;
    float buf_tmp[2 * SPEECH_BUFFER_LEN];

    /* analysis filter bank (polyphase implementation) */
    for (k = 0; k < len / 2; k++)
	buf_tmp[k] = in[2 * k + 1];
    zero_filter (buf_tmp, out, len / 2, coefs_ana_filt_lb[0],
		 State->state_ana_filt_lb1);
    for (k = 0; k < len / 2; k++)
	buf_tmp[k] = in[2 * k];
    zero_filter (buf_tmp, buf_tmp, len / 2, coefs_ana_filt_lb[1],
		 State->state_ana_filt_lb2);
    for (k = 0; k < len / 2; k++)
	out[k] += buf_tmp[k];

}

void
FGV_MEM::filterbank_ana_hb (float *out_7k,	/* high-band signal, 7 kHz sampling */
			    float *out_14k,	/* wideband signal, 14 kHz sampling */
			    const float *in, int16 len,	/* length of input array */
			    STATE_ANA_HB * State)
{
    int k;
    float buf_tmp[4 * SPEECH_BUFFER_LEN + 20];
    float buf_tmp2[4 * SPEECH_BUFFER_LEN];

    /* upsample to 32 kHz */
    Interpolate_allpass (in, State->state_ana_filt_hb_1, len,
			 buf_tmp + LEN_DN_HB - 2);
    /* resample to 28 kHz */
    for (k = 0; k < LEN_DN_HB - 2; k++) {
	buf_tmp[k] = State->state_ana_filt_hb_2[k];
	State->state_ana_filt_hb_2[k] = buf_tmp[k + 2 * len];
    }
    resample_down_HB (buf_tmp, buf_tmp2, 2 * len);
    /* downsample to 14 kHz */
    Decimate_allpass_steep (buf_tmp2, State->state_ana_filt_hb_3, 7 * len / 4,
			    buf_tmp);

    /* Eighth rate (full band 14 kHz) exctracted here */
    /* shift buffer */
    for (k = 0; k < 7 * len / 8; k++)
	out_14k[k] = buf_tmp[k];

    /* flip spectrum */
    for (k = 0; k < 7 * len / 8; k += 2)
	buf_tmp[k] = -buf_tmp[k];
    /* downsample to 7 kHz */
    Decimate_allpass_steep (buf_tmp, State->state_ana_filt_hb_4, 7 * len / 8,
			    buf_tmp2);

    /* shaping filter */
    polezero_filter (buf_tmp2, out_7k, 7 * len / 16, coefs_ana_filt_hb_b,
		     coefs_ana_filt_hb_a, State->state_ana_filt_hb_iir);

}

void
FGV_MEM::filterbank_syn_lb (float *out, const float *in, int16 len,	/* length of input array */
			    STATE_SYN_LB * State)
{
    int k;
    float buf_tmp[4 * SPEECH_BUFFER_LEN + 20];
    float buf_tmp2[4 * SPEECH_BUFFER_LEN];


    /* overlap-add delay for low-band */
    for (k = 0; k < LB_SYN_DELAY; k++)
	buf_tmp2[k] = State->buf_LB_delay[k];
    for (k = LB_SYN_DELAY; k < len; k++)
	buf_tmp2[k] = in[k - LB_SYN_DELAY];
    for (k = 0; k < LB_SYN_DELAY; k++)
	State->buf_LB_delay[k] = in[len - LB_SYN_DELAY + k];

    /* synthesis filter bank (polyphase implementation) */
    for (k = 0; k < SYN_FILT_LEN_LB - 1; k++)
	State->mem_syn_filt_lb_tmp[k] = State->mem_syn_filt_lb[k];	/* store filter memory */
    zero_filter (buf_tmp2, buf_tmp, len, coefs_syn_filt_lb[1],
		 State->state_syn_filt_lb);
    for (k = 0; k < len; k++)
	out[2 * k] = buf_tmp[k];
    for (k = 0; k < SYN_FILT_LEN_LB - 1; k++)
	State->mem_syn_filt_lb[k] = State->mem_syn_filt_lb_tmp[k];	/* restore filter memory */
    zero_filter (buf_tmp2, buf_tmp, len, coefs_syn_filt_lb[0],
		 State->state_syn_filt_lb);
    for (k = 0; k < len; k++)
	out[2 * k + 1] = buf_tmp[k];
    polezero_filter (out, out, 2 * len, coefs_syn_filt_lb_b,
		     coefs_syn_filt_lb_a, State->state_syn_filt_lb_iir);
}

void
FGV_MEM::filterbank_syn_hb (float *out, const float *in_7k,	/* high-band input signal, 7 kHz sampling */
			    const float *in_14k,	/* wideband input signal, 14 kHz sampling */
			    int16 len,	/* length of input array */
			    STATE_SYN_HB * State)
{
    int k;
    float buf_tmp[4 * SPEECH_BUFFER_LEN + 20];
    float buf_tmp2[4 * SPEECH_BUFFER_LEN];


    /* upsample to 14 kHz */
    Interpolate_allpass_steep (in_7k, State->state_syn_filt_hb_0, len,
			       buf_tmp2);
    /* flip spectrum */
    for (k = 1; k < 2 * len; k += 2)
	buf_tmp2[k] = -buf_tmp2[k];

    /* Eighth rate (full band 14 kHz) inserted here */
    for (k = 0; k < 2 * len; k++)
	buf_tmp2[k] += in_14k[k];

    /* upsample to 28 kHz */
    Interpolate_allpass_steep (buf_tmp2, State->state_syn_filt_hb_1, 2 * len,
			       buf_tmp + LEN_UP_HB);
    /* resample to 16 kHz */
    for (k = 0; k < LEN_UP_HB; k++) {
	buf_tmp[k] = State->state_syn_filt_hb_2[k];
	State->state_syn_filt_hb_2[k] = buf_tmp[k + 4 * len];
    }
    resample_up_HB (buf_tmp, out, 4 * len);

    /* notch at 7100 Hz */
    polezero_filter (out, out, (16 * len) / 7, coefs_syn_filt_hb_b1,
		     coefs_syn_filt_hb_a1, State->state_syn_filt_hb_iir1);
    polezero_filter (out, out, (16 * len) / 7, coefs_syn_filt_hb_b2,
		     coefs_syn_filt_hb_a2, State->state_syn_filt_hb_iir2);
}


/* high band analysis filter */
void
resample_down_HB (const float *in,	/* size: len + LEN_DN_HB - 2 */
		  float *out,	/* size: len * 7/8 */
		  int len)
{
    int k, i;
    float sum;
    const float *in_ptr, *cf;

    len = len / 8;

    for (k = 0; k < len; k++) {
	sum = 0.0;
	cf = &coefs_down_HB[0][0];
	in_ptr = &in[0];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[1][0];
	in_ptr = &in[1];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[2][0];
	in_ptr = &in[2];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[3][0];
	in_ptr = &in[3];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[4][0];
	in_ptr = &in[4];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[5][0];
	in_ptr = &in[5];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_down_HB[6][0];
	in_ptr = &in[6];
	for (i = 0; i < LEN_DN_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	in += 8;
    }
}

/* high band synthsis filter */
void
resample_up_HB (const float *in,	/* size: len + LEN_UP_HB */
		float *out,	/* size: len * 4/7 */
		int len)
{
    int k, i;
    float sum;
    const float *in_ptr, *cf;

    len = len / 7;

    for (k = 0; k < len; k++) {
	sum = 0.0;
	cf = &coefs_up_HB[0][0];
	in_ptr = &in[1];
	for (i = 0; i < LEN_UP_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_up_HB[1][0];
	in_ptr = &in[3];
	for (i = 0; i < LEN_UP_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_up_HB[2][0];
	in_ptr = &in[5];
	for (i = 0; i < LEN_UP_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	sum = 0.0;
	cf = &coefs_up_HB[3][0];
	in_ptr = &in[7];
	for (i = 0; i < LEN_UP_HB; i++)
	    sum += cf[i] * in_ptr[i];
	*out++ = sum;

	in += 7;
    }
}


/* 
* interpolation by a factor 2
*/
void
Interpolate_allpass (const float *in, float *state,	/* array of size: 2*ALLPASSSECTIONS */
		     const int N,	/* number of input samples */
		     float *out)
{				/* array of size 2*N */
    int n, k;
    float temp[ALLPASSSECTIONS - 1];


    /* 
     * upper allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	temp[0] = state[0] + AP2[0] * in[k];
	state[0] = in[k] - AP2[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS - 1; n++) {
	    temp[n] = state[n] + AP2[n] * temp[n - 1];
	    state[n] = temp[n - 1] - AP2[n] * temp[n];
	}

	out[2 * k + 1] =
	    state[ALLPASSSECTIONS - 1] + AP2[ALLPASSSECTIONS -
					     1] * temp[ALLPASSSECTIONS - 2];
	state[ALLPASSSECTIONS - 1] =
	    temp[ALLPASSSECTIONS - 2] - AP2[ALLPASSSECTIONS - 1] * out[2 * k +
								       1];
    }

    /* 
     * lower allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	temp[0] = state[ALLPASSSECTIONS] + AP1[0] * in[k];
	state[ALLPASSSECTIONS] = in[k] - AP1[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS - 1; n++) {
	    temp[n] = state[ALLPASSSECTIONS + n] + AP1[n] * temp[n - 1];
	    state[ALLPASSSECTIONS + n] = temp[n - 1] - AP1[n] * temp[n];
	}

	out[2 * k] =
	    state[2 * ALLPASSSECTIONS - 1] + AP1[ALLPASSSECTIONS -
						 1] * temp[ALLPASSSECTIONS -
							   2];
	state[2 * ALLPASSSECTIONS - 1] =
	    temp[ALLPASSSECTIONS - 2] - AP1[ALLPASSSECTIONS - 1] * out[2 * k];
    }
}

/* 
* interpolation by a factor 2
*/
void
Interpolate_allpass_steep (const float *in, float *state,	/* array of size: 2*ALLPASSSECTIONS_STEEP */
			   const int N,	/* number of input samples */
			   float *out)
{				/* array of size 2*N */
    int n, k;
    float temp[ALLPASSSECTIONS_STEEP - 1];


    /* 
     * upper allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	temp[0] = state[0] + AP2_STEEP[0] * in[k];
	state[0] = in[k] - AP2_STEEP[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS_STEEP - 1; n++) {
	    temp[n] = state[n] + AP2_STEEP[n] * temp[n - 1];
	    state[n] = temp[n - 1] - AP2_STEEP[n] * temp[n];
	}

	out[2 * k + 1] =
	    state[ALLPASSSECTIONS_STEEP - 1] +
	    AP2_STEEP[ALLPASSSECTIONS_STEEP -
		      1] * temp[ALLPASSSECTIONS_STEEP - 2];
	state[ALLPASSSECTIONS_STEEP - 1] =
	    temp[ALLPASSSECTIONS_STEEP - 2] -
	    AP2_STEEP[ALLPASSSECTIONS_STEEP - 1] * out[2 * k + 1];
    }

    /* 
     * lower allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	temp[0] = state[ALLPASSSECTIONS_STEEP] + AP1_STEEP[0] * in[k];
	state[ALLPASSSECTIONS_STEEP] = in[k] - AP1_STEEP[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS_STEEP - 1; n++) {
	    temp[n] =
		state[ALLPASSSECTIONS_STEEP + n] + AP1_STEEP[n] * temp[n - 1];
	    state[ALLPASSSECTIONS_STEEP + n] =
		temp[n - 1] - AP1_STEEP[n] * temp[n];
	}

	out[2 * k] =
	    state[2 * ALLPASSSECTIONS_STEEP - 1] +
	    AP1_STEEP[ALLPASSSECTIONS_STEEP -
		      1] * temp[ALLPASSSECTIONS_STEEP - 2];
	state[2 * ALLPASSSECTIONS_STEEP - 1] =
	    temp[ALLPASSSECTIONS_STEEP - 2] -
	    AP1_STEEP[ALLPASSSECTIONS_STEEP - 1] * out[2 * k];
    }
}



/* 
* interpolation by a factor 2
*/
void
Interpolate_allpass_course (float *in, float *state,	/* array of size: 2*ALLPASSSECTIONS */
			    short N,	/* number of input samples */
			    float *out	/* array of size 2*N */
    )
{
    int n, k;


    /* 
     * upper allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	out[2 * k + 1] = state[0] + AP2_COURSE[0] * in[k];
	state[0] = in[k] - AP2_COURSE[0] * out[2 * k + 1];
    }

    /* 
     * lower allpass filter chain 
     */
    for (k = 0; k < N; k++) {
	out[2 * k] = state[1] + AP1_COURSE[0] * in[k];
	state[1] = in[k] - AP1_COURSE[0] * out[2 * k];
    }
}



/* 
* decimation by a factor 2
*/
void
Decimate_allpass (const float *in, float *state,	/* array of size: 2*ALLPASSSECTIONS+1 */
		  const int N,	/* number of input samples */
		  float *out)
{				/* array of size N/2 */
    int n, k;
    float temp[ALLPASSSECTIONS];


    /* 
     * upper allpass filter chain 
     */
    for (k = 0; k < N / 2; k++) {
	temp[0] = state[0] + AP1[0] * in[2 * k];
	state[0] = in[2 * k] - AP1[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS - 1; n++) {
	    temp[n] = state[n] + AP1[n] * temp[n - 1];
	    state[n] = temp[n - 1] - AP1[n] * temp[n];
	}

	out[k] =
	    state[ALLPASSSECTIONS - 1] + AP1[ALLPASSSECTIONS -
					     1] * temp[ALLPASSSECTIONS - 2];
	state[ALLPASSSECTIONS - 1] =
	    temp[ALLPASSSECTIONS - 2] - AP1[ALLPASSSECTIONS - 1] * out[k];
    }

    /* 
     * lower allpass filter chain 
     */
    temp[0] = state[ALLPASSSECTIONS] + AP2[0] * state[2 * ALLPASSSECTIONS];
    state[ALLPASSSECTIONS] = state[2 * ALLPASSSECTIONS] - AP2[0] * temp[0];

    /* for better performance, unroll this loop */
    for (n = 1; n < ALLPASSSECTIONS - 1; n++) {
	temp[n] = state[ALLPASSSECTIONS + n] + AP2[n] * temp[n - 1];
	state[ALLPASSSECTIONS + n] = temp[n - 1] - AP2[n] * temp[n];
    }

    temp[ALLPASSSECTIONS - 1] =
	state[2 * ALLPASSSECTIONS - 1] + AP2[ALLPASSSECTIONS -
					     1] * temp[ALLPASSSECTIONS - 2];
    state[2 * ALLPASSSECTIONS - 1] =
	temp[ALLPASSSECTIONS - 2] - AP2[ALLPASSSECTIONS -
					1] * temp[ALLPASSSECTIONS - 1];
    out[0] = (out[0] + temp[ALLPASSSECTIONS - 1]) * 0.5;

    for (k = 1; k < N / 2; k++) {
	temp[0] = state[ALLPASSSECTIONS] + AP2[0] * in[2 * k - 1];
	state[ALLPASSSECTIONS] = in[2 * k - 1] - AP2[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS - 1; n++) {
	    temp[n] = state[ALLPASSSECTIONS + n] + AP2[n] * temp[n - 1];
	    state[ALLPASSSECTIONS + n] = temp[n - 1] - AP2[n] * temp[n];
	}

	temp[ALLPASSSECTIONS - 1] =
	    state[2 * ALLPASSSECTIONS - 1] + AP2[ALLPASSSECTIONS -
						 1] * temp[ALLPASSSECTIONS -
							   2];
	state[2 * ALLPASSSECTIONS - 1] =
	    temp[ALLPASSSECTIONS - 2] - AP2[ALLPASSSECTIONS -
					    1] * temp[ALLPASSSECTIONS - 1];
	out[k] = (out[k] + temp[ALLPASSSECTIONS - 1]) * 0.5;
    }

    /* z^(-1) */
    state[2 * ALLPASSSECTIONS] = in[N - 1];
}

/* 
* decimation by a factor 2
*/
void
Decimate_allpass_steep (const float *in, float *state,	/* array of size: 2*ALLPASSSECTIONS_STEEP+1 */
			const int N,	/* number of input samples */
			float *out)
{				/* array of size N/2 */
    int n, k;
    float temp[ALLPASSSECTIONS_STEEP];


    /* 
     * upper allpass filter chain 
     */
    for (k = 0; k < N / 2; k++) {
	temp[0] = state[0] + AP1_STEEP[0] * in[2 * k];
	state[0] = in[2 * k] - AP1_STEEP[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS_STEEP - 1; n++) {
	    temp[n] = state[n] + AP1_STEEP[n] * temp[n - 1];
	    state[n] = temp[n - 1] - AP1_STEEP[n] * temp[n];
	}

	out[k] =
	    state[ALLPASSSECTIONS_STEEP - 1] +
	    AP1_STEEP[ALLPASSSECTIONS_STEEP -
		      1] * temp[ALLPASSSECTIONS_STEEP - 2];
	state[ALLPASSSECTIONS_STEEP - 1] =
	    temp[ALLPASSSECTIONS_STEEP - 2] -
	    AP1_STEEP[ALLPASSSECTIONS_STEEP - 1] * out[k];
    }

    /* 
     * lower allpass filter chain 
     */
    temp[0] =
	state[ALLPASSSECTIONS_STEEP] +
	AP2_STEEP[0] * state[2 * ALLPASSSECTIONS_STEEP];
    state[ALLPASSSECTIONS_STEEP] =
	state[2 * ALLPASSSECTIONS_STEEP] - AP2_STEEP[0] * temp[0];

    /* for better performance, unroll this loop */
    for (n = 1; n < ALLPASSSECTIONS_STEEP - 1; n++) {
	temp[n] =
	    state[ALLPASSSECTIONS_STEEP + n] + AP2_STEEP[n] * temp[n - 1];
	state[ALLPASSSECTIONS_STEEP + n] =
	    temp[n - 1] - AP2_STEEP[n] * temp[n];
    }

    temp[ALLPASSSECTIONS_STEEP - 1] =
	state[2 * ALLPASSSECTIONS_STEEP - 1] +
	AP2_STEEP[ALLPASSSECTIONS_STEEP - 1] * temp[ALLPASSSECTIONS_STEEP -
						    2];
    state[2 * ALLPASSSECTIONS_STEEP - 1] =
	temp[ALLPASSSECTIONS_STEEP - 2] - AP2_STEEP[ALLPASSSECTIONS_STEEP -
						    1] *
	temp[ALLPASSSECTIONS_STEEP - 1];
    out[0] = (out[0] + temp[ALLPASSSECTIONS_STEEP - 1]) * 0.5;

    for (k = 1; k < N / 2; k++) {
	temp[0] = state[ALLPASSSECTIONS_STEEP] + AP2_STEEP[0] * in[2 * k - 1];
	state[ALLPASSSECTIONS_STEEP] = in[2 * k - 1] - AP2_STEEP[0] * temp[0];

	/* for better performance, unroll this loop */
	for (n = 1; n < ALLPASSSECTIONS_STEEP - 1; n++) {
	    temp[n] =
		state[ALLPASSSECTIONS_STEEP + n] + AP2_STEEP[n] * temp[n - 1];
	    state[ALLPASSSECTIONS_STEEP + n] =
		temp[n - 1] - AP2_STEEP[n] * temp[n];
	}

	temp[ALLPASSSECTIONS_STEEP - 1] =
	    state[2 * ALLPASSSECTIONS_STEEP - 1] +
	    AP2_STEEP[ALLPASSSECTIONS_STEEP -
		      1] * temp[ALLPASSSECTIONS_STEEP - 2];
	state[2 * ALLPASSSECTIONS_STEEP - 1] =
	    temp[ALLPASSSECTIONS_STEEP - 2] -
	    AP2_STEEP[ALLPASSSECTIONS_STEEP -
		      1] * temp[ALLPASSSECTIONS_STEEP - 1];
	out[k] = (out[k] + temp[ALLPASSSECTIONS_STEEP - 1]) * 0.5;
    }

    /* z^(-1) */
    state[2 * ALLPASSSECTIONS_STEEP] = in[N - 1];
}
