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
static char rcsid[] =
    "$Id: new_mode.cc,v 1.25.6.4 2007/06/24 06:43:15 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <stdio.h>
#include "defines.h"
#include "struct.h"
#include "filt.h"
#include "io.h"
#include "macro.h"


#define SFNUM 10

#define CURR_SNR_OUT 0
#define CURR_NS_SNR_OUT 0
#define FRAME_SNR_OUT 0
#define ZCR_OUT 0
#define BER_OUT  0
#define VER_OUT  0
#define PEAKY_OUT 0

#define SILENCE 1
#define DOWN_TRANSIENT 5
#define UP_TRANSIENT 6

#define SNR_THLD 25		// Threshold to distinguish between clean and noisy speech

static float lpf_num_coef[13] =
    { 0.02581939589060, 0.15507757732855, 0.51401728885032, 1.18090000045387,
2.05669859465992, 2.83147579652417, 3.14362122814036, 2.83147579652417, 2.05669859465991,
1.18090000045386, 0.51401728885032, 0.15507757732855, 0.02581939589060 };
static float lpf_den_coef[13] =
    { 1.0, 1.18516766163740, 2.85846612055532, 2.78134952904007,
3.19933397924298, 2.39702624599082, 1.70571473131276, 0.92260744442122, 0.42394697577451,
0.14991606832858, 0.04018979420882, 0.00721315391677, 0.00066683112593 };
static float hpf_num_coef[13] =
    { 0.02581939589060, -0.15507757732854, 0.51401728885032,
-1.18090000045385, 2.05669859465990, -2.83147579652415, 3.14362122814034, -2.83147579652416,
2.05669859465991, -1.18090000045386, 0.51401728885032, -0.15507757732855, 0.02581939589060 };
static float hpf_den_coef[13] =
    { 1.0, -1.18516766163740, 2.85846612055531, -2.78134952904007,
3.19933397924297, -2.39702624599081, 1.70571473131276, -0.92260744442122, 0.42394697577451,
-0.14991606832858, 0.04018979420882, -0.00721315391677, 0.00066683112593 };


#include "hpf80.h"

static void
hpf100 (float *inptr, float *outptr, HPFMem * z, int nsamples)
{

    //this is actually 80Hz
    static float b[5] = {
	0.93079640846211,
	-3.72226699350450,
	5.58294128342915,
	-3.72226699350450,
	0.93079640846211
    };

    static float a[5] = {
	1.00000000000000,
	-3.85563847199150,
	5.57815775689283,
	-3.58888990447158,
	0.86638195400646
    };

    int i, j;

    for (i = 0; i < nsamples; i++) {
	z->b[0] = *inptr++;
	z->a[0] = b[0] * z->b[0];
	for (j = 4; j >= 1; j--) {
	    z->a[0] += b[j] * z->b[j] - a[j] * z->a[j];
	    z->a[j] = z->a[j - 1];
	    z->b[j] = z->b[j - 1];
	}
	*outptr++ = z->a[0];
    }

}				/* end of hpf100() */

MODE
FGV_MEM::new_mode_decision (float *in)
{				// Current Mode Decision
    MODE m;
    int va;
    POLEZERO_FILTER lp_filter = { 12, 12, 12, 0, lpf_filt_mem };
    POLEZERO_FILTER hp_filter = { 12, 12, 12, 0, hpf_filt_mem };

    float vER, vER2;

    float Enext = 0;
    int i, j, zcr = 0;
    float E = 0.001, EL = 0.001, EH = 0.001, out[FSIZE];
    float out_hpf[FSIZE + LOOKAHEAD_LEN];
    float bER;

    va = (evrc_rate == 1) ? 0 : 1;

    for (i = 0; i < 2; i++) {
	curr_snr[i] =
	    10 * log10 (rate_mem.band_power[i] / rate_mem.band_noise_sm[i]);
    }

    float curr_snr_diff = curr_snr[0] - curr_snr[1];

    float sfEng[SFNUM];
    float mean_sfe, max_sfe, peaky_l, peaky_r, peaky;
    int maxsfe_idx;

    if (data_packet.WB_MODE_BIT) {
	hpf100 (in, out_hpf, &z, FSIZE);
	HPFMem ztmp = z;	// copy state to temp state

	hpf100 (in + FSIZE, out_hpf + FSIZE, &z, LOOKAHEAD_LEN);
	z = ztmp;		// restore state for next FSIZE frmae
	in = &out_hpf[0];	// reset input pointer definition
    }
    if (mode_decision_FirstTime) {
	mode_decision_FirstTime = 0;
	AV_TH = args->avg_rate_target;
	if (data_packet.WB_MODE_BIT == 0) {
	    if (args->operating_point == 0)
		pattern_m = 1000;
	    else
		pattern_m = 0;
	}
    }


    // Determine NACF thresholds based on NS SNR
    VOICEDTH = 0.75;
    LOWVOICEDTH = 0.55;
    UNVOICEDTH = 0.35;

    float tmpxxx = SNR_THLD;

    if (curr_ns_snr[0] < SNR_THLD) {
	VOICEDTH = 0.65;
	LOWVOICEDTH = 0.5;
	UNVOICEDTH = 0.35;
    }
    if (data_packet.WB_MODE_BIT) {
	UNVOICEDTH = 0.30;
    }

    for (i = 0; i < FSIZE - 1; i++)
	if (in[i] * in[i + 1] < 0)
	    zcr++;
    for (i = 0; i < FSIZE; i++)
	E += SQR (in[i]);
    for (i = 0; i < FSIZE; i++)
	Enext += SQR (in[i + LOOKAHEAD_LEN]);

    polezero_filter (in, out, FSIZE, lpf_num_coef, lpf_den_coef, lp_filter);
    for (i = 0; i < FSIZE; i++)
	EL += SQR (out[i]);
    polezero_filter (in, out, FSIZE, hpf_num_coef, hpf_den_coef, hp_filter);
    for (i = 0; i < FSIZE; i++)
	EH += SQR (out[i]);

    bER = log2 (EL / EH);
    vER2 = MIN (20, 10 * log10 (E / vEav));
    vER = 10 * log10 (E / vEprev);

    if (VER_OUT)
	write_samples (1, &vER, 1);

    if (BER_OUT) {
	float temp = 10 * bER;

	write_samples (1, &temp, 1);
    }
    if (ZCR_OUT)
	write_ints (1, &zcr, 1);
    if (CURR_SNR_OUT) {
	float temp_snr = curr_snr[0] - curr_snr[1];

	write_samples (1, &temp_snr, 1);
    }

    if (CURR_NS_SNR_OUT) {
	float temp_snr = curr_ns_snr[0] - curr_ns_snr[1];

	write_samples (1, &temp_snr, 1);
    }

    if (FRAME_SNR_OUT) {
	float temp_snr[2];

	temp_snr[0] = curr_snr[0] - prev_snr[0];
	temp_snr[1] = curr_ns_snr[0] - prev_ns_snr[0];
	write_samples (1, temp_snr, 2);
    }

    for (i = 0; i < SFNUM; i++) {
	sfEng[i] = 0;
	for (j = 0; j < FSIZE / SFNUM; j++)
	    sfEng[i] += SQR (in[j + i * FSIZE / SFNUM]);
	sfEng[i] = sqrt (sfEng[i] / FSIZE * SFNUM);
    }
    for (i = 0, mean_sfe = 0; i < SFNUM; i++)
	mean_sfe += sfEng[i];
    mean_sfe /= SFNUM;
    // Search the upper half frame for peakiness
    max_sfe = sfEng[4];
    maxsfe_idx = 5;
    for (i = 5; i < SFNUM; i++) {
	if (sfEng[i] > max_sfe) {
	    max_sfe = sfEng[i];
	    maxsfe_idx = i + 1;
	}
    }
    peaky = max_sfe / mean_sfe;
    peaky_l = MAX (sfEng[0], sfEng[1]) / mean_sfe;
    peaky_r = MAX (sfEng[SFNUM - 1], sfEng[SFNUM - 2]) / mean_sfe;

    if (PEAKY_OUT) {
	float temp = 10 * peaky_l;

	write_samples (1, &temp, 1);
	temp = 10 * peaky;
	write_samples (1, &temp, 1);
	temp = 10 * peaky_r;
	write_samples (1, &temp, 1);
    }

    if (va == ACTIVE) {

	if (nacf_ap[2] >= VOICEDTH) {	// 2nd subframe NACF high
	    switch (prev_mode) {

	    case SILENCE:
		m.speech_type = UP_TRANSIENT;
		if (nacf_ap[3] < UNVOICEDTH && zcr > 70 && bER < 0)
		    m.speech_type = UNVOICED;
		if (nacf_ap[3] < UNVOICEDTH && zcr > 100 && vER < -20)
		    m.speech_type = UNVOICED;
		if (vER < -25)
		    m.speech_type = UNVOICED;

		break;

	    case UNVOICED:
		m.speech_type = UP_TRANSIENT;
		if (data_packet.WB_MODE_BIT) {
		    if (nacf_ap[3] < UNVOICEDTH && nacf_ap[4] < UNVOICEDTH
			&& nacf < UNVOICEDTH && zcr > 70 && bER < 0)
			m.speech_type = UNVOICED;
		}
		else {
		    if (nacf_ap[3] < UNVOICEDTH && nacf_ap[4] < UNVOICEDTH
			&& zcr > 70 && bER < 0)
			m.speech_type = UNVOICED;
		}
		if (nacf_ap[3] < UNVOICEDTH && zcr > 100 && vER < -20
		    && E < Eprev)
		    m.speech_type = UNVOICED;
		if (vER < -25)
		    m.speech_type = UNVOICED;

		break;

	    case VOICED:
		m.speech_type = VOICED;
		if ((nacf_ap[2] - nacf_ap[1] > 0.3)
		    && nacf_ap[1] < LOWVOICEDTH && E > 0.5 * Eprev)
		    m.speech_type = TRANSIENT;
		if (vER < -18 && nacf_ap[3] < VOICEDTH)
		    m.speech_type = DOWN_TRANSIENT;
		if (vER < -30 && E < Eprev)
		    m.speech_type = UNVOICED;
		break;

	    case DOWN_TRANSIENT:
		m.speech_type = DOWN_TRANSIENT;
		if (vER < -30)
		    m.speech_type = UNVOICED;
		if (E > Eprev)
		    m.speech_type = TRANSIENT;
		break;

	    default:		// previous is transient
		m.speech_type = VOICED;
		if ((nacf_ap[2] - nacf_ap[1] > 0.45)
		    || nacf_ap[1] < UNVOICEDTH)
		    m.speech_type = TRANSIENT;
		if (nacf_ap[1] - nacf_ap[0] > 0.3 && nacf_ap[1] < VOICEDTH
		    && prev_mode != TRANSIENT)
		    m.speech_type = TRANSIENT;

		if (((nacf_ap[1] - nacf_ap[0] > 0.3)
		     || (nacf_ap[2] - nacf_ap[1] > 0.3))
		    && nacf_ap[1] < LOWVOICEDTH && nacf_ap[0] < UNVOICEDTH)
		    m.speech_type = TRANSIENT;

		if (E < 0.05 * vEav && nacf_ap[3] < VOICEDTH)
		    m.speech_type = DOWN_TRANSIENT;
		if (vER < -30 && E < Eprev)
		    m.speech_type = UNVOICED;
		break;
	    }
	}

	else {			// 2nd subframe NACF not very high
	    if (nacf_ap[2] <= UNVOICEDTH) {
		switch (prev_mode) {

		case SILENCE:
		    m.speech_type = UNVOICED;
		    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			&& nacf_ap[4] > LOWVOICEDTH && zcr < 110 && vER > -21
			&& (nacf_ap[3] > UNVOICEDTH || bER > -2))
			m.speech_type = UP_TRANSIENT;
		    if (zcr < 60 && bER > 6 && vER > -13
			&& curr_snr_diff > 13.5463)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[3] > LOWVOICEDTH || nacf_ap[4] > LOWVOICEDTH)
			&& bER > 3 && zcr < 60 && vER > 0)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > LOWVOICEDTH && nacf_ap[4] > LOWVOICEDTH
			&& bER > 3 && zcr < 40 && vER > -15)
			m.speech_type = UP_TRANSIENT;
		    break;

		case UNVOICED:
		    m.speech_type = UNVOICED;
		    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			&& nacf_ap[4] > LOWVOICEDTH && zcr < 110 && vER > -21
			&& (nacf_ap[3] > UNVOICEDTH || bER > -2))
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			&& E > 5 * Eprev && bER > 3 && vER > -16)
			m.speech_type = UP_TRANSIENT;
		    if (data_packet.WB_MODE_BIT) {
			if (nacf > UNVOICEDTH && nacf_ap[4] > VOICEDTH
			    && bER > 3 && zcr < 80 && E > 2 * Eprev
			    && vER > -20)
			    m.speech_type = UP_TRANSIENT;
			if (nacf > UNVOICEDTH && nacf_ap[3] > VOICEDTH
			    && bER > 0 && zcr < 80 && E > 2 * Eprev
			    && vER > -16)
			    m.speech_type = UP_TRANSIENT;
		    }
		    else {
			if (nacf_ap[4] > VOICEDTH && bER > 3 && zcr < 80
			    && E > 2 * Eprev && vER > -20)
			    m.speech_type = UP_TRANSIENT;
			if (nacf_ap[3] > VOICEDTH && bER > 0 && zcr < 80
			    && E > 2 * Eprev && vER > -16)
			    m.speech_type = UP_TRANSIENT;
		    }
		    if ((nacf_ap[3] > LOWVOICEDTH || nacf_ap[4] > LOWVOICEDTH)
			&& bER > 0 && zcr < 80 && maxsfe_idx == SFNUM
			&& vER > -16)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > UNVOICEDTH && nacf_ap[4] > UNVOICEDTH && zcr < 80 && bER > 4 && vER > -20 && curr_snr_diff > 0 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
			m.speech_type = UP_TRANSIENT;

		    if (nacf_ap[3] > LOWVOICEDTH && nacf_ap[4] > LOWVOICEDTH
			&& zcr < 80 && E > Eprev && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > VOICEDTH && nacf_ap[4] > VOICEDTH
			&& vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (bER > 0 && zcr < 80 && E > 10 * Eprev && vER > -10)
			m.speech_type = UP_TRANSIENT;

		    if (((bER > 5 && vER > -10) || (bER > 3 && vER > -6) || (bER > 0 && vER > -3)) && zcr < 80 && curr_snr_diff > 0 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
			m.speech_type = UP_TRANSIENT;

		    if (bER > -3 && vER > -14 && zcr < 80 && curr_ns_snr[0] < SNR_THLD)	//Noisy condition
			m.speech_type = UP_TRANSIENT;
		    if (data_packet.WB_MODE_BIT) {
			if (nacf > UNVOICEDTH && zcr < 60 && bER > 2 && vER > -5 && curr_ns_snr[0] < SNR_THLD)	//Noisy condition
			    m.speech_type = UP_TRANSIENT;
		    }
		    else {
			if (zcr < 60 && bER > 2 && vER > -5 && curr_ns_snr[0] < SNR_THLD)	//Noisy condition
			    m.speech_type = UP_TRANSIENT;
		    }
		    if (bER > -5 && vER > -4 && nacf_ap[3] > UNVOICEDTH && nacf_ap[4] > nacf_ap[3] && zcr < 100 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
			m.speech_type = UP_TRANSIENT;

		    break;

		case DOWN_TRANSIENT:	// following down_transient
		    m.speech_type = UNVOICED;
		    if (bER > 0) {
			if (vER > -20 && zcr < 70)
			    m.speech_type = DOWN_TRANSIENT;
			if ((nacf_ap[3] > LOWVOICEDTH
			     || nacf_ap[4] > LOWVOICEDTH) && E >= 2 * Eprev
			    && vER > -15)
			    m.speech_type = TRANSIENT;
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			    && nacf_ap[4] > VOICEDTH && vER > -15
			    && E > Eprev)
			    m.speech_type = TRANSIENT;
		    }
		    else {
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			    && nacf_ap[4] > VOICEDTH && vER > -15
			    && E > 2 * Eprev)
			    m.speech_type = TRANSIENT;
		    }
		    break;
		case VOICED:
		default:
		    if (bER > 0) {
			m.speech_type = TRANSIENT;
			if (vER2 < -15 && zcr < 90 && E < Eprev
			    && nacf_ap[3] < VOICEDTH)
			    m.speech_type = DOWN_TRANSIENT;
			if (vER < -25 && E < Eprev)
			    m.speech_type = UNVOICED;
		    }
		    else {
			m.speech_type = UNVOICED;
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3] && nacf_ap[4] > UNVOICEDTH && zcr < 100 && vER > -20 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
			    m.speech_type = TRANSIENT;
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3] && nacf_ap[4] > LOWVOICEDTH && zcr < 100 && vER > -20 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
			    m.speech_type = TRANSIENT;

			if (((bER > -1 && vER > -12 && zcr < 80) || (bER > -3 && vER > -10 && zcr < 90) || (bER > -7 && vER > -3 && zcr < 105)) && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
			    m.speech_type = TRANSIENT;
			if (data_packet.WB_MODE_BIT) {
			    if (bER > -3 && vER > -10 && zcr < 90 && (nacf > UNVOICEDTH || nacf_ap[3] > UNVOICEDTH) && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
				m.speech_type = TRANSIENT;
			}
			else {
			    if (bER > -3 && vER > -10 && zcr < 90 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
				m.speech_type = TRANSIENT;
			}
		    }
		    break;

		}
	    }

	    else {		// UV<nacf_ap[2]<V, major gray area
		switch (prev_mode) {

		case SILENCE:
		    m.speech_type = UNVOICED;
		    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			&& nacf_ap[4] > LOWVOICEDTH && zcr < 110 && vER > -22)
			m.speech_type = UP_TRANSIENT;

		    if (nacf_ap[2] > LOWVOICEDTH && curr_snr_diff > 0
			&& E > 2 * Eprev && zcr < 70 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[2] > LOWVOICEDTH && nacf_ap[3] > LOWVOICEDTH
			&& zcr < 70 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[4] > VOICEDTH || nacf_ap[3] > VOICEDTH)
			&& nacf_ap[3] > UNVOICEDTH && zcr < 70 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[3] - nacf_ap[2] > 0.3) && E > 2 * Eprev
			&& zcr < 70 && bER > 0 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[4] > VOICEDTH
			&& (nacf_ap[2] > LOWVOICEDTH
			    || nacf_ap[3] > LOWVOICEDTH) && E > 2 * Eprev
			&& curr_snr_diff > 0 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > LOWVOICEDTH && nacf_ap[4] > LOWVOICEDTH
			&& bER > 3 && zcr < 40 && vER > -15)
			m.speech_type = UP_TRANSIENT;
		    break;

		case UNVOICED:
		    m.speech_type = UNVOICED;
		    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			&& nacf_ap[4] > LOWVOICEDTH && zcr < 110 && vER > -22)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[4] > LOWVOICEDTH || nacf_ap[3] > LOWVOICEDTH)
			&& zcr < 100 && E > Eprev && bER > 0 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (data_packet.WB_MODE_BIT) {
			if (nacf_ap[4] > VOICEDTH && nacf > UNVOICEDTH
			    && zcr < 100 && E > Eprev && bER > 1 && vER > -16)
			    m.speech_type = UP_TRANSIENT;
		    }
		    else {
			if (nacf_ap[4] > VOICEDTH && zcr < 100 && E > Eprev
			    && bER > 1 && vER > -16)
			    m.speech_type = UP_TRANSIENT;
		    }
		    if (zcr < 60 && bER > 3 && E > 5 * Eprev
			&& curr_snr_diff > 9.0309 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[3] > UNVOICEDTH || nacf_ap[4] > UNVOICEDTH)
			&& bER > 1 && vER > -16 && zcr < 80
			&& curr_snr_diff > -6.0206)
			m.speech_type = UP_TRANSIENT;

		    if (nacf_ap[2] > LOWVOICEDTH && curr_snr_diff > 0
			&& E > 2 * Eprev && zcr < 70 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[2] > LOWVOICEDTH && nacf_ap[3] > LOWVOICEDTH
			&& zcr < 100 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[4] > VOICEDTH || nacf_ap[3] > VOICEDTH)
			&& nacf_ap[3] > UNVOICEDTH && zcr < 80 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if ((nacf_ap[3] > LOWVOICEDTH || nacf_ap[4] > LOWVOICEDTH)
			&& bER > 0 && zcr < 100 && maxsfe_idx == SFNUM
			&& vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[3] > UNVOICEDTH && nacf_ap[4] > UNVOICEDTH && zcr < 100 && vER > -3 && bER > -5 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
			m.speech_type = UP_TRANSIENT;

		    if (bER > 4 && zcr < 100 && maxsfe_idx == SFNUM
			&& vER > -20)
			m.speech_type = UP_TRANSIENT;

		    if ((nacf_ap[3] - nacf_ap[2] > 0.3) && E > 2 * Eprev
			&& zcr < 70 && bER > 0 && vER > -20)
			m.speech_type = UP_TRANSIENT;
		    if (nacf_ap[4] > VOICEDTH
			&& (nacf_ap[2] > LOWVOICEDTH
			    || nacf_ap[3] > LOWVOICEDTH) && E > 2 * Eprev
			&& curr_snr_diff > 0 && vER > -20)
			m.speech_type = UP_TRANSIENT;

		    if (((bER > 5 && vER > -10) || (bER > 3 && vER > -6) || (bER > 0 && vER > -3)) && zcr < 80 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition
			m.speech_type = UP_TRANSIENT;

		    if (bER > -3 && vER > -14 && zcr < 80 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
			m.speech_type = UP_TRANSIENT;
		    break;

		case DOWN_TRANSIENT:
		    m.speech_type = UNVOICED;
		    if (bER > 0) {
			if (vER > -20 && zcr < 70)
			    m.speech_type = DOWN_TRANSIENT;
			if ((nacf_ap[3] > LOWVOICEDTH
			     || nacf_ap[4] > LOWVOICEDTH) && E >= 2 * Eprev
			    && vER > -15)
			    m.speech_type = TRANSIENT;
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			    && nacf_ap[4] > VOICEDTH && vER > -15)
			    m.speech_type = TRANSIENT;

		    }
		    else {
			if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3]
			    && nacf_ap[4] > VOICEDTH && vER > -15
			    && E > 2 * Eprev)
			    m.speech_type = TRANSIENT;
		    }
		    break;
		case VOICED:
		default:
		    if (nacf_ap[2] >= LOWVOICEDTH) {
			m.speech_type = VOICED;
			if (nacf_ap[1] < LOWVOICEDTH
			    || (nacf_ap[1] < VOICEDTH
				&& nacf_ap[1] - nacf_ap[0] > 0.3) || bER < 0)
			    m.speech_type = TRANSIENT;
			if (vER < -15 && nacf_ap[3] < LOWVOICEDTH
			    && E < Eprev)
			    m.speech_type = DOWN_TRANSIENT;
			if (bER < 0 && vER < -20 && Enext < E
			    && nacf_ap[3] < UNVOICEDTH
			    && nacf_ap[4] < UNVOICEDTH)
			    m.speech_type = UNVOICED;
			if (bER < -30 && E < Eprev)
			    m.speech_type = UNVOICED;
			if ((nacf_ap[2] - nacf_ap[3] > 0.3)
			    && nacf_ap[3] < 0.3 && nacf_ap[4] < 0.25)
			    m.speech_type = TRANSIENT;
		    }
		    else {
			if (bER > 0) {
			    m.speech_type = TRANSIENT;
			    if (vER2 < -15 && zcr < 90 && E < Eprev
				&& nacf_ap[3] < VOICEDTH)
				m.speech_type = DOWN_TRANSIENT;
			    if (vER < -25 && E < Eprev)
				m.speech_type = UNVOICED;
			}
			else {
			    m.speech_type = UNVOICED;
			    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3] && zcr < 100 && vER > -20 && curr_ns_snr[0] >= SNR_THLD)	// Clean condition          
				m.speech_type = TRANSIENT;
			    if (nacf_ap[3] > nacf_ap[2] && nacf_ap[4] > nacf_ap[3] && zcr < 110 && vER > -15 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
				m.speech_type = TRANSIENT;
			    if (bER > -1 && vER > -12 && zcr < 80)
				m.speech_type = TRANSIENT;
			    if (data_packet.WB_MODE_BIT) {
				if (bER > -3 && vER > -10 && nacf > UNVOICEDTH
				    && zcr < 90)
				    m.speech_type = TRANSIENT;
			    }
			    else {
				if (bER > -3 && vER > -10 && zcr < 90)
				    m.speech_type = TRANSIENT;
			    }
			    if (bER > -7 && vER > -3 && zcr < 105 && curr_ns_snr[0] < SNR_THLD)	// Noisy condition
				m.speech_type = TRANSIENT;
			}

		    }

		    break;
		}
	    }
	}
	m.curr_voiced = ((m.speech_type == VOICED)
			 || (m.speech_type == TRANSIENT && prev_voiced));
    }

    else {
	m.speech_type = SILENCE;
	m.curr_voiced = 0;
    }

    if (m.speech_type == UNVOICED) {
	m.curr_voiced = 0;
    }


    if (m.curr_voiced) {
	vE[vcount % 3] = E;
	vcount++;
	vEav = (vE[0] + vE[1] + vE[2]) / 3.0;
    }
    else if (m.speech_type <= UNVOICED) {
	vE[0] = vE[1] = vE[2] = 0;
	vcount = 0;
	vEav = 0.1;
    }

    if (m.speech_type <= UNVOICED)
	FirstHVframe = 1;
    Eprev = E;
    if (m.speech_type == VOICED)
	Eavg = 0.8 * Eavg + 0.2 * E;
    if (m.curr_voiced)
	vEprev = vEav;
    prev_snr_diff = curr_snr_diff;
    for (i = 0; i < 2; i++) {
	prev_snr[i] = curr_snr[i];	// Back up band SNR from VAD
	prev_ns_snr[i] = curr_ns_snr[i];	// Back up band SNR from NS
    }

    prev_snr[0] = curr_snr[0];
    prev_snr[1] = curr_snr[1];

    //refine speech_type
    if (evrc_rate == 3)
	if (m.speech_type == 4 || m.speech_type == 3) {
	    m.speech_type = 5;
	}
    m.mode = m.speech_type;

    if (data_packet.WB_MODE_BIT) {
	if (m.mode == 3)
	    m.mode = 4;
    }
    //refine mode
    if (m.mode == 3) {
	if (MAX (delay, pdelay) / MIN (delay, pdelay) > 1.2)
	    m.mode = 4;
	if (fabs (delay - pdelay) > 15)
	    m.mode = 4;		// Messes up 'Q' otherwise in PPP
    }

    // Back up variables
    prev_mode = m.mode;
    prev_voiced = m.curr_voiced;

    rate = m.mode;
    // End new stuff from G*

    if (rate == 3) {
	PPP_MODE_E = 'H';
	rcelp_half_rateE = 0;
    }
    if (rate == 5) {
	rate = 3;
	rcelp_half_rateE = 1;
    }
    if (rate == 6)
	rate = 4;

    if (data_packet.WB_MODE_BIT == 1) {
	if (rate == 3)
	    rate = 4;
    }

    if (args->operating_point == 0) {
	rate = evrc_rate;
	rcelp_half_rateE = 1;	//No PPP for OP-0 at 1/2 rate (really at all rates)
	if (args->avg_rate_control && rate != 3 && m.mode == 2)
	    rate = 2;
    }
    if (args->avg_rate_control && data_packet.WB_MODE_BIT == 0) {
	if (args->operating_point == 0) {
	    if (rate == 2) {
		patterncount += pattern_m;
		if (patterncount >= 1000) {
		    patterncount -= 1000;
		    rate = 4;
		}
	    }
	}
    }

    // Pattern QPPP_FCELP-FCELP for OP-1
    if (args->operating_point == 1) {
	if (rate != 3)
	    pppcountE = 0;
	else if (rcelp_half_rateE != 1) {
	    if (curr_ns_snr[0] < 25) {
		pppcountE++;
		if (pppcountE == 1)
		    rate = 3;
		else if (pppcountE == 2) {
		    if (lastrateE == 3)
			rate = 4;
		    else
			rate = 3;
		}
		else {
		    rate = 4;
		    pppcountE = 0;
		}
	    }
	    else {
		pppcountE++;
		if (pppcountE == 1)
		    rate = 4;
		else if (pppcountE == 2) {
		    if (lastrateE == 3)
			rate = 4;
		    else
			rate = 3;
		}
		else {
		    rate = 4;
		    pppcountE = 0;
		}
	    }
	}
    }

    NS_SNR = curr_ns_snr[0];
    PPP_BUMPUP = 0;		// Reset this flag before every encode to avoid spurious
    // percentages of rate=4 bumpups
    //
    // Signalling Dim and Burst
    if (args->signalling_file != NULL) {
	char dim;
	fread (&dim, sizeof (char), 1, args->signalling_file);
	if (dim == 1) {
	    if (rate == 4)
		rate = 3;
	    dim_and_burstE = 1;
	    if (args->operating_point == 1)
		pppcountE = 0;	//Reset the pattern FQF in OP1
	}
	else
	    dim_and_burstE = 0;
    }

    if (rate > args->max_rate)
	rate = args->max_rate;
    if (rate < args->min_rate)
	rate = args->min_rate;
    PREV_LSP_FLAG = 0;
    if (args->max_rate == 3)
	rcelp_half_rateE = 1;
    if (!data_packet.WB_MODE_BIT) {
	SPL_HNELP = 0;
	if ((m.mode == 2) && (args->operating_point == 0)
	    && (args->max_rate < 4)) {
	    rate = 2;
	    SPL_HNELP = 1;
	}

	if ((rate_mem.last_rate == 3)
	    && ((rate == 4) || ((rate == 3) && (rcelp_half_rateE == 0))))
	    rate_mem.last_rate_1st_stage = 4;
    }



    return m;
}
