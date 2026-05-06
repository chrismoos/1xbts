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
#include "filt.h"

void
FGV_MEM::WB_encoder ()
{
    int k;
    MODE m;
    float R[17];

    if (args->operating_point < 3)
	data_packet.WB_MODE_BIT = 0;
    else
	data_packet.WB_MODE_BIT = 1;

    if (args->fullrate_coding_method > 0)	//MDCT or Music_MDCT
    {
	if (args->Fsinp == 16000)
	    data_packet.WB_MODE_BIT = 1;	//WB
	else if (args->Fsinp == 8000)
	    data_packet.WB_MODE_BIT = 0;	//NB
    }

    if ((args->Fsinp == 8000) && (args->operating_point == 3)) {

	printf ("This mode of operation not supported !\n");
	exit (0);
    }


    if ((args->Fsinp == 16000) && (args->operating_point < 3)) {

	printf ("This mode of operation is temporarily not supported !\n");
	exit (0);
    }

    //due to non-zero upper band decoded energy for an input having zero upper band energy

    if (data_packet.WB_MODE_BIT == 1) {

	if (data_packet.WB_MODE_BIT == 1) {
	    MDCT_NPULSES = 23;	/* Used for WB Coding */
	    MDCT_NWORDS = 8;	/* 23 pulses on 144 position = 114 bits => 7*15 bits + 9 bits */
	    MDCT_REMAINDER = 9;
	}
	else {
	    MDCT_NPULSES = 28;	/* Used for NB Coding */
	    MDCT_NWORDS = 9;	/* 28 pulses on 144 position = 131 bits => 8*15 bits + 11 bits */
	    MDCT_REMAINDER = 11;
	}
    }
    else {

	if (args->avg_rate_control) {
	    if (args->avg_rate_target > 7500)
		args->operating_point = 0;
	    else if (args->avg_rate_target > 6600)
		args->operating_point = 1;
	    else if (args->avg_rate_target > 5700)
		args->operating_point = 2;

	}

    }


    if ((WB_encoder_first_time == 1) && (args->Fsinp == 16000)) {

	for (k = 0; k < 17; k++)
	    R[k] = 0.0;
	/* initialize filter banks */
	filterbank_init_ana_lb (&S_ana_lb);
	filterbank_init_ana_hb (&S_ana_hb);

	WB_encoder_first_time = 0;

    }


    if (args->Fsinp == 16000) {

	for (k = 0; k < 2 * ibuf_len; k++)
	    buf_float[k] = (float) buf16[k];

      /*=====================================================================*/
      /*        ..Hi-Level limiter AGC                                       */
      /*---------------------------------------------------------------------*/

#define THR_1 82		// RMS threshold, approx 10*log10(16000.0*16000.0)
#define THR_2 87		// PEAK threshold, approx 10*log10(32767.0*32767.0)
#define MIN_AGC_GAIN 0.707
#define AGC_SM_FAC 0.999f

	{
	    int j;
	    float peak = 0;
	    float rms = 0;
	    float xt, p2rms = 0;

	    peak = buf_float[0] * buf_float[0];
	    rms = peak;
	    for (j = 1; j < 2 * ibuf_len; j++) {
		xt = buf_float[j] * buf_float[j];
		if (xt > peak) {
		    peak = xt;
		}
		rms += xt;
	    }
	    rms *= 1.0 / (2 * ibuf_len);

	    peak = 10.0 * log10 (peak);
	    rms = 10.0 * log10 (rms);
	    p2rms = peak - rms;
	    if (peak > THR_2 && rms > THR_1) {
		wb_agc = MIN_AGC_GAIN;
	    }
	    else {
		wb_agc *= 1.1;
	    }
	    wb_agc = Min (1.0, Max (MIN_AGC_GAIN, wb_agc));


	    static float agc_filt_num_coef[3] = {
		0.000009825916820471736201625390094704926013946533203125,
		0.000019651833640943472403250780189409852027893066406250,
		0.000009825916820471736201625390094704926013946533203125
	    };

	    static float agc_filt_den_coef[3] = {
		1.000000000000000000000000000000000000000000000000000000,
		-1.991114292201653590552723471773788332939147949218750000,
		0.991153595868935477497529973334167152643203735351562500
	    };
	    POLEZERO_FILTER agc_filt = { 2, 2, 2, 0, wb_agc_lpfmem };

	    for (j = 0; j < 2 * ibuf_len; j++) {
	      polezero_filter (&wb_agc, &wb_agc_sm, 1, agc_filt_num_coef,
			       agc_filt_den_coef, agc_filt);
	      buf_float[j] *= wb_agc_sm;
	    }
	}



      /*=====================================================================*/
      /*        ..Hi-pass filter with 50 Hz Cheby II.                        */
      /*---------------------------------------------------------------------*/

	if (args->highpass_filter)
	    hpf80 (buf_float, buf_float, &hpfmem, 2 * ibuf_len);

	/*=====================================================================*/
	/*        ..Noise suppression on 10ms buffer.                          */
	/*---------------------------------------------------------------------*/
	if (args->noise_suppression) {
	    noise_suprs (buf_float, next_ns_snr[0]);
	    noise_suprs (buf_float + ibuf_len, next_ns_snr[1]);
	}

	if (args->noise_suppression) {

	    /* low-band analysis filter */
	    filterbank_ana_lb (buf + LOOKAHEAD_LEN - DELAY / 2, buf_float,
			       2 * ibuf_len, &S_ana_lb);
	    STATE_ANA_LB S_ana_lb_TMP = S_ana_lb;	// Save LB filter state

	    filterbank_ana_lb (buf + LOOKAHEAD_LEN - DELAY / 2 + ibuf_len,
			       buf_float + 2 * ibuf_len, DELAY, &S_ana_lb);
	    S_ana_lb = S_ana_lb_TMP;	// Restore LB filter state


	    /* high-band analysis filter */
	    filterbank_ana_hb (buf_HB + (LOOKAHEAD_LEN - DELAY / 2) * 7 / 8 +
			       hb_ana_delay,
			       buf_WB14 + (LOOKAHEAD_LEN -
					   DELAY / 2) * 7 / 4 +
			       hb_ana_delay * 2, buf_float, 2 * ibuf_len,
			       &S_ana_hb);
	    STATE_ANA_HB S_ana_hb_TMP = S_ana_hb;	// Save HB filter state

	    filterbank_ana_hb (buf_HB + (LOOKAHEAD_LEN - DELAY / 2) * 7 / 8 +
			       hb_ana_delay + ibuf_len * 7 / 8,
			       buf_WB14 + (LOOKAHEAD_LEN -
					   DELAY / 2) * 7 / 4 +
			       hb_ana_delay * 2 + ibuf_len * 7 / 4,
			       buf_float + 2 * ibuf_len, DELAY, &S_ana_hb);
	    S_ana_hb = S_ana_hb_TMP;	// Restore HB filter state

	}
	else {

	    /* low-band analysis filter */
	    filterbank_ana_lb (buf + LOOKAHEAD_LEN, buf_float, 2 * ibuf_len,
			       &S_ana_lb);

	    /* high-band analysis filter */
	    filterbank_ana_hb (buf_HB + LOOKAHEAD_LEN * 7 / 8 + hb_ana_delay,
			       buf_WB14 + LOOKAHEAD_LEN * 7 / 4 +
			       hb_ana_delay * 2, buf_float, 2 * ibuf_len,
			       &S_ana_hb);
	}

	for (k = 0; k < SPEECH_BUFFER_LEN + LOOKAHEAD_LEN; k++) {
	  buf_backup[k] = buf[k];
	}

    }				// above code applicable only to wideband
    else {

      /*=====================================================================*/
      /*        ..Hi-pass filter with 80 Hz Cheby II.                        */
      /*---------------------------------------------------------------------*/

	if (args->highpass_filter) {
	    hpf80_8k (buf + LOOKAHEAD_LEN, buf + LOOKAHEAD_LEN, &hpfmem8k,
		      SPEECH_BUFFER_LEN);
	}

	/*=====================================================================*/
	/*        ..Noise suppression on 10ms buffer.                          */
	/*---------------------------------------------------------------------*/
	if (args->noise_suppression) {
	    noise_suprs_8k (buf + LOOKAHEAD_LEN, next_ns_snr[0]);
	    noise_suprs_8k (buf + LOOKAHEAD_LEN + ibuf_len / 2,
			    next_ns_snr[1]);
	}

    }

    pre_encode (buf, R);


    if (data_packet.WB_MODE_BIT == 1) {
	/* Use either vad decision from nsvad() or EVRC rate selection: */
	/* WB lpc analysis; to be shared with 8th rate encoder */

	float HBnrg;

	HBnrg = 0.0;
	for (k = 0; k < 140; k++)
	    HBnrg +=
		buf_HB[LOOKAHEAD_LEN * 7 / 8 + HB_ANA_DELAY_NS_LB +
		       k] * buf_HB[LOOKAHEAD_LEN * 7 / 8 +
				   HB_ANA_DELAY_NS_LB + k];
	HBnrg /= 16.0;		// scaling

	select_rate (R, args->max_rate, args->min_rate, beta, HBnrg);

    }
    else {			//narrowband case

	select_rate (R, args->max_rate, args->min_rate, beta, 0);


    }

    m = new_mode_decision (buf);

    clearBitTotal ();


    if (!args->olr_calibration)
	encode (rate, buf16);


    if (data_packet.WB_MODE_BIT == 1) {


	if (celp_mdct_dec == 0) {	//CELP
	    UB_enc ();
	}
	else {

	    if (data_packet.WB_MODE_BIT == 1) {
		UB_enc ();
	    }


	}
    }

    lastrateE = bit_rate;
    printBitTotal ();

    if (data_packet.WB_MODE_BIT == 1) {
	if (rate == 2)
	    rate = 3;
	if (data_packet.PACKET_RATE == 2)
	    data_packet.PACKET_RATE = 3;
    }

    update_average_rate (rate);



}
