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
    "$Id: rda.cc,v 1.12.4.3 2007/06/24 06:43:17 apadmana Exp $";

/**********************************************************************/
/* Enhanced Variable Rate Speech Codec -                              */
/*     Option Candidate for North American Wideband CDMA Digital      */
/*     Cellular Telephony.                                            */
/*                                                                    */
/* (C) Copyright 1994-1995, QUALCOMM Incorporated                     */
/* QUALCOMM Incorporated                                              */
/* 6455 Lusk Blvd                                                     */
/* San Diego, CA 92121                                                */
/*                                                                    */
/* Note:  Reproduction and use of this software for the design and    */
/*     development of North American Wideband CDMA Digital            */
/*     Cellular Telephony Standards is authorized by                  */
/*     QUALCOMM Incorporated.  QUALCOMM Incorporated does not         */
/*     authorize the use of this software for any other purpose.      */
/*                                                                    */
/*     The availability of this software does not provide any license */
/*     by implication, estoppel, or otherwise under any patent rights */
/*     of QUALCOMM Incorporated or others covering any use of the     */
/*     contents herein.                                               */
/*                                                                    */
/*     Any copies of this software or derivative works must include   */
/*     this and all other proprietary notices.                        */
/**********************************************************************/
/* ratedec - selects encoding rate to use                             */
/**********************************************************************/
#include <math.h>

#include "struct.h"
#include "rda.h"


#define rint(x)  (int)((x) + 0.5)	/*...define non-ANSI function... */

/**********************************************************************/
/*  Initialize rate.                                                  */
/**********************************************************************/
void
initialize_rate (ENCODER_MEM * e_mem, int modebit)
{
    int i, j, k, freqbands;

    if (modebit == 1)
	freqbands = FREQBANDS;
    else
	freqbands = FREQBANDS_NB;

    e_mem->last_rate = EIGHTH;	/* Added by D. Pian 12-Sep-94 */
    e_mem->last_rate_1st_stage = EIGHTH;	/* Added by D. Pian 19-Jan-95 */
    e_mem->last_rate_2nd_stage = EIGHTH;	/* Added by D. Pian 29-Mar-95 */


    /* rate decision initialization */
    e_mem->frame_num = 0;

    e_mem->num_full_frames = 0;
    e_mem->hangover = 10;
    e_mem->hangover_in_progress = 0;

    e_mem->adaptcount = 0;
    e_mem->pitchrun = 0;
    e_mem->frame_energy_sm[0] = 3200000 * 16;	/*changed by D. Pian 08-Feb-95 */
    e_mem->frame_energy_sm[1] = 320000 * 16;	/*changed by D. Pian 08-Feb-95 */

    for (i = 0; i < freqbands; i++) {
	e_mem->band_power_last[i] = 0.0;
    }				/* Added by D. Pian 12-Sep-94 */

    e_mem->signal_energy[0] = 3200000 * 16;	/* Added by D. Pian 12-Sep-94 */
    e_mem->signal_energy[1] = 320000 * 16;	/* and Changed 5-Apr-95 */

    if (modebit == 1) {
	e_mem->signal_energy[freqbands - 1] = 320000 * 16;
    }
    e_mem->snr_stat_once = 0;

    for (j = 0; j < freqbands; j++)
	e_mem->band_noise_sm[j] = HIGH_THRESH_LIM;

    for (j = 0; j < FREQBANDS_NB; j++) {
	for (i = 0; i < FILTERORDER; i++) {
	    e_mem->r_filt[j][i] = 0;
	    for (k = 0; k < FILTERORDER - i; k++) {
		e_mem->r_filt[j][i] += rate_filt[j][k] * rate_filt[j][k + i];
	    }
	}
    }
    for (j = 0; j < freqbands; j++) {
	e_mem->snr[j] = e_mem->signal_energy[j] / e_mem->band_noise_sm[j];
	e_mem->snr_map[j] = 0;
    }
}

/**********************************************************************/
/*  Band energy.                                                      */
/**********************************************************************/
void
band_energy_fcn (float *R, float *energy, ENCODER_MEM * e_mem)
{
    int i, j;

    /* find the energy in each band   */
    for (j = 0; j < FREQBANDS_NB; j++) {
	energy[j] = e_mem->r_filt[j][0] * R[0];
	for (i = 1; i < FILTERORDER; i++) {
	    energy[j] += 2.0 * e_mem->r_filt[j][i] * R[i];
	}
    }
}

/**********************************************************************/
/*  Update background.                                                */
/**********************************************************************/
void
FGV_MEM::update_background (short rate, ENCODER_MEM * e_mem, float beta)
{
    int i;
    int freqbands;

    if (data_packet.WB_MODE_BIT == 1)
	freqbands = FREQBANDS;
    else
	freqbands = FREQBANDS_NB;
    //static short  first = 0;

    /* Updating Smooth Energy estimate Esmb(i)(k), Eq. (2.4.4.2.1-1) */
    if (update_background_first == 0) {
	update_background_first = 1;
	for (i = 0; i < freqbands; i++) {
	    e_mem->frame_energy_sm[i] = e_mem->band_power[i];
	}
    }
    else {
	for (i = 0; i < freqbands; i++) {
	    e_mem->frame_energy_sm[i] =
		SMSNR * e_mem->frame_energy_sm[i] + (1.0 -
						     SMSNR) *
		e_mem->band_power[i];
	}
    }
	/*************************************************************************/
    /* Section 2.4.4.2.2 Updating background noise                           */

    /* Now do the threshold adaptation                                       */
    /* don't adapt thresholds unless NACF < NACF_ADAP_BGN_THR over ADP frames */
    /* we're assuming the background noise here doesn't have periodicity in  */
    /* the range of typical speech                                           */

    if (beta < NACF_ADAP_BGN_THR) {
	e_mem->adaptcount++;
    }
    else {
	e_mem->adaptcount = 0;
    }

    if (e_mem->adaptcount > ADP) {
	/* change the predictor filter for the noise whitening */
	/* noise                                            */
	/* update our background noise estimate */
	for (i = 0; i < freqbands; i++) {
	    /* just increase background noise until it matches */
	    /* lowest stationary period background noise       */
	    if (e_mem->band_noise_sm[i] + 1 <
		e_mem->band_noise_sm[i] * INC_FACTOR) {
		e_mem->band_noise_sm[i] *= INC_FACTOR;
	    }
	    else {
		e_mem->band_noise_sm[i] += 1.0;
	    }
	}
	e_mem->adaptcount = ADP + 1;
    }
    else {

	/*if SNR map est. is valid & above SNR_MAP_THRESHOLD let background noise */
	/* estimate slowly inch up like it does in IS-96.  This will allow */
	/* the rate decision to perform as well as IS-96 in babble noise and */
	/* other non-stationary noises    */

	if (e_mem->snr_stat_once == 1) {	/* snr is valid  */
	    for (i = 0; i < freqbands; i++) {
		if ((e_mem->snr_map[i] > SNR_MAP_THRESHOLD) &&
		    (e_mem->band_noise_sm[i] < e_mem->frame_energy_sm[i])) {
		    if (e_mem->band_noise_sm[i] + 1.0 <
			e_mem->band_noise_sm[i] * IS96_INC) {
			e_mem->band_noise_sm[i] *= IS96_INC;
		    }
		    else {
			e_mem->band_noise_sm[i] += 1.0;
		    }
		}
	    }
	}
    }

    /* if input energy is lower than noise estimate, */
    /* reduce background noise estimate immediately  */

    for (i = 0; i < freqbands; i++) {
	/* Update Background Noise Estimate */
	if (e_mem->frame_energy_sm[i] < e_mem->band_noise_sm[i]) {
	    e_mem->band_noise_sm[i] = e_mem->frame_energy_sm[i];
	}
    }

    for (i = 0; i < freqbands; i++) {
	if (e_mem->band_noise_sm[i] > HIGH_THRESH_LIM) {
	    e_mem->band_noise_sm[i] = HIGH_THRESH_LIM;
	}
	if (e_mem->band_noise_sm[i] < LOWEST_LEVEL[i]) {
	    e_mem->band_noise_sm[i] = LOWEST_LEVEL[i];
	}
    }


    if (encode_fcnt < 8 && data_packet.WB_MODE_BIT == 1) {
	for (i = 0; i < freqbands; i++) {
	    /* Update Background Noise Estimate */
	    e_mem->band_noise_sm[i] = e_mem->band_power[i];
	}
    }

    /* end of updating backgound noise energy */
	/***************************************************************/

	/***************************************************************/
    /* Section 2.4.4.2.2 Updating Signal Energy Estimate */
    /* pitchrun keeps track of the number of frames in a row with periodicity */

    if (beta > NACF_SOLID_VOICED) {
	e_mem->pitchrun += 1;
    }
    else {
	e_mem->pitchrun = 0;
    }

    if (e_mem->pitchrun > STATVOICED) {	/* decrease the signal energy   */
	/* if we're in a voiced segment */
	e_mem->snr_stat_once = 1;	/* confidence that we have seen speech */

	/* indicates a stationary voiced segment       */
	for (i = 0; i < freqbands; i++) {
	    e_mem->signal_energy[i] *= SCALE_DOWN_ENERGY;	/* *= 0.97 */
	}
    }

    /* assume that the highest energy sounds are speech signals and */
    /* thus the highest energy frames contain speech and can be used */
    /* to define the SNR */

    for (i = 0; i < freqbands; i++) {
	/* Update Signal Energy Estimate */
	if (e_mem->frame_energy_sm[i] > e_mem->signal_energy[i]) {
	    e_mem->signal_energy[i] = e_mem->frame_energy_sm[i];
	}
    }

    /* end updating signal energy estimate */
	/************************************************************************/

    /* This is now done near the top of select_mode1() */
    /* update SNR estimates */

    for (i = 0; i < freqbands; i++) {
	if (e_mem->band_noise_sm[i] > 0.0) {
	    e_mem->snr[i] = e_mem->signal_energy[i] / e_mem->band_noise_sm[i];
	}
	else {
	    e_mem->snr[i] = 100000000.0;	/* a very high snr if noise =0.0 */
	}
    }

    if (e_mem->snr_stat_once != 0) {
	/* we have seen some speech and are confident in our SNR measure */
	e_mem->snr_stat_once = 1;

	/* UPDATE THIS FOR GENERAL SNR MAPS! */
	for (i = 0; i < freqbands; i++) {

	    if (e_mem->snr[i] > 0.0) {
		e_mem->snr_map[i] =
		    (int) rint ((10 * log10 (e_mem->snr[i]) - 20.0) / 5.0);
	    }
	    else {
		e_mem->snr_map[i] = 0;
	    }

	    if (e_mem->snr_map[i] < 0)
		e_mem->snr_map[i] = 0;
	    if (e_mem->snr_map[i] > 7)
		e_mem->snr_map[i] = 7;
	}
    }
    else {
	/* we haven't seen speech, aren't confident in our estimate of the */
	/* signal energy and will use a nominal energy of */
	/* VOICE_INITIAL dB (-18dbm0) for our signal energy */

	for (i = 0; i < freqbands; i++) {

	    if (e_mem->band_noise_sm[i] > 0.0) {
		if (i == 0) {
		    e_mem->snr_map[i] =
			(int)
			rint ((VOICE_INITIAL -
			       10 * log10 (e_mem->band_noise_sm[i]) -
			       20.0) / 5.0);
		}
		else {
		    e_mem->snr_map[i] =
			(int)
			rint ((VOICE_INITIAL_HI -
			       10 * log10 (e_mem->band_noise_sm[i]) -
			       20.0) / 5.0);
		}
	    }
	    else {
		e_mem->snr_map[i] = 7;
	    }
	    if (e_mem->snr_map[i] < 0)
		e_mem->snr_map[i] = 0;
	    if (e_mem->snr_map[i] > 7)
		e_mem->snr_map[i] = 7;
	}
    }

    e_mem->last_rate = rate;
    e_mem->last_rate_2nd_stage = rate;

}

/**********************************************************************/
/*  Select rate.                                                      */
/**********************************************************************/
void
FGV_MEM::select_rate (float *R_interp,
		      short max_rate, short min_rate, float beta, float HBnrg)
{
    //static short  first = 0;
    //static float (*THRESH_SNR)[TLEVELS][2];
    //static int *hangover;
    //short  evrc_rate;
    int i;
    int k;
    int freqbands;

    //      e_mem = &rate_mem;  /* done for rate integration with AT&T simulation*/

    if (data_packet.WB_MODE_BIT == 1) {
	freqbands = FREQBANDS;
    }
    else {
	freqbands = FREQBANDS_NB;
    }

    if (select_rate_first == 0) {
	/* do initialization of rate_mem */
	initialize_rate (e_mem, (int) data_packet.WB_MODE_BIT);
	if (data_packet.WB_MODE_BIT == 1) {


	    hangover = hangover0FB;
	    THRESH_SNR = THRESH_SNR0_FB;
	}
	else {


	    if (args->operating_point == 0) {

		hangover = hangover0;
		THRESH_SNR = THRESH_SNR0;
	    }
	    else if (args->operating_point == 1) {
		hangover = hangover1;
		THRESH_SNR = THRESH_SNR1;
	    }
	    else {
		hangover = hangover2;
		THRESH_SNR = THRESH_SNR2;
	    }

	}
	select_rate_first = 1;
    }


    /* this is the acf of the noise reducing prediction filter */

    /* now filter the input speech by bandpass filters and     */
    /* derive the necessary band energies                      */

    band_energy_fcn (R_interp, e_mem->band_power, e_mem);
    if (data_packet.WB_MODE_BIT == 1) {
	e_mem->band_power[freqbands - 1] = HBnrg;
    }

    /* now our threshold comparison and background noise estimation */
    /* is done for all frequency bands                              */
    /* rate thresholds are calculated below, but not saved */

    for (k = 0; k < freqbands; k++) {
	e_mem->band_rate[k] = EIGHTH;
	for (i = 0; i < 2; i++) {
	    /* rate_thresh[k][i] = THRESH_SNR[k][e_mem->snr_map[k]][i]* e_mem->band_noise_sm[k]; */
	    /* since we changed SMSNR to 0.6 from 0.8 */


	    if (e_mem->band_power[k] >
		THRESH_SNR[k][e_mem->snr_map[k]][i] *
		e_mem->band_noise_sm[k]) {
		if (e_mem->band_rate[k] == EIGHTH) {
		    e_mem->band_rate[k] = HALFRATE_VOICED;
		}
		else if (e_mem->band_rate[k] == HALFRATE_VOICED) {
		    e_mem->band_rate[k] = FULLRATE_VOICED;
		}
	    }
	}
    }				/* do threshold comparisons for full and half rates  */

    /* use 10log10(e_mem->band_power[k]/band_noise_sm[k]) */

    /* the maximum rate for all bands is chosen as the rate */
    evrc_rate = e_mem->band_rate[0];
    for (k = 1; k < freqbands; k++) {
	if (e_mem->band_rate[k] > evrc_rate)
	    evrc_rate = e_mem->band_rate[k];
    }

    /* Section 2.4.4.1.3 Calculating Hangover Frames as Function of SNR */
    if ((e_mem->num_full_frames > FULL_THRESH)
	|| e_mem->hangover_in_progress == 1) {
	/* only use hangover after so many full rate frames in a row */
	if (evrc_rate != FULLRATE_VOICED) {
	    e_mem->hangover += 1;
	    if (e_mem->hangover <= hangover[e_mem->snr_map[0]]) {
		evrc_rate = FULLRATE_VOICED;
		e_mem->hangover_in_progress = 1;
	    }
	    else {
		e_mem->hangover_in_progress = 0;
	    }
	}
	else {
	    e_mem->hangover_in_progress = 0;
	}
    }

    if ((evrc_rate == FULLRATE_VOICED) && (e_mem->hangover_in_progress == 0)) {
	e_mem->hangover = 0;
	e_mem->num_full_frames += 1;
    }
    else {
	e_mem->num_full_frames = 0;
    }
    /* end of hangover algorithm     */


    /* assuming only 1/8,1/2,and full rates pluse some 1/2 and full rates modes */
    /* if last rate is full force 1/2 rate before going to 1/8                  */

    /* Section 2.4.4.1.4 Constraints on Rate Selection */
    if ((e_mem->last_rate_1st_stage == FULLRATE_VOICED)
	&& evrc_rate == EIGHTH) {
	evrc_rate = HALFRATE_VOICED;
    }

    /* allows fast convergence for EIGHTH rate */
    if (e_mem->frame_num == 0) {
	evrc_rate = FULLRATE_VOICED;
    }

    if (evrc_rate > max_rate) {
	evrc_rate = max_rate;
    }
    if (evrc_rate < min_rate) {
	evrc_rate = min_rate;
    }

    e_mem->last_rate_1st_stage = evrc_rate;
    e_mem->last_rate = evrc_rate;

    /* update background noise and signal energy estimates */
    update_background (evrc_rate, e_mem, beta);

    e_mem->frame_num += 1;

    return;
}
