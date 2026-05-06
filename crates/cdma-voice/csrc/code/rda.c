/**********************************************************************
Each of the companies; Lucent, Motorola, Nokia, and Qualcomm (hereinafter 
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
all Normative portions of the Enhanced Variable Rate Codec (EVRC) as 
it exists on the date of submittal of this form, should the EVRC be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC or the field of use of practice of the 
EVRC Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
 
/*======================================================================*/
/*     Enhanced Variable Rate Codec - Bit-Exact C Specification         */
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
/* ratedec - selects encoding rate to use                             */
/**********************************************************************/
/* E. Chen: not needed; #include <math.h> */

/* E. Chen: DSP math libraries */
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"

#include "coder_ra.h"
#include "qcoder.h"
#include "rda.h"
#include "evrc_state.h"

/* E. Chen: not needed; #define rint(x)  (int)((x) + 0.5)     ...define non-ANSI function... */

/* E. Chen: added the following arrays to aid in calculation of e_mem->band_power */
/* E. Chen: may not need "r_filt" array if not initializing e_mem->r_filt */
static INT16 r_filt[FREQBANDS][FILTERORDER] =
{
	{13527, 8672, -548, -5071, -2981, 48, -456, -2740, -3003,
	 -959, 765, 780, 113, -36, 208, 262, 101},
	{15566, -10421, 877, 3414, -965, -1956, 1128, 1259, -1721,
	 428, 409, -171, -164, 85, 52, -35, 4}
};
static INT16 r_filt_x2[FREQBANDS][FILTERORDER] =
{
	{27055, 17345, -1096, -10143, -5962, 96, -912, -5481,
	 -6006, -1919, 1531, 1561, 227, -72, 416, 525, 202},
	{31132, -20842, 1754, 6829, -1930, -3912, 2256, 2518,
	 -3442, 857, 819, -342, -328, 171, 105, -70, 9}
};

/**********************************************************************/
/*  Initialize rate.                                                  */
/**********************************************************************/
void initialize_rate(
						ENCODER_MEM * e_mem
)
{
	INT16 i, j;					/* E. Chen: int */
	INT16 d_shiftcnt, n_shiftcnt;

	e_mem->last_rate = EIGHTH;	/* Added by D. Pian 12-Sep-94 */
	e_mem->last_rate_1st_stage = EIGHTH;	/* Added by D. Pian 19-Jan-95 */
	e_mem->last_rate_2nd_stage = EIGHTH;	/* Added by D. Pian 29-Mar-95 */

	/* rate decision initialization */

	e_mem->num_full_frames = 0;
	e_mem->hangover = 10;
	e_mem->hangover_in_progress = 0;

	e_mem->adaptcount = 0;
	e_mem->pitchrun = 0;
	e_mem->frame_energy_sm[0] = 3200000 * 16;	/* changed by D. Pian 08-Feb-95 */
	e_mem->frame_energy_sm[1] = 320000 * 16;	/* changed by D. Pian 08-Feb-95 */

	for (i = 0; i < FREQBANDS; i++)
	{
		e_mem->band_power_last[i] = 0;	/* E. Chen: 0.0; */
	}							/* Added by D. Pian 12-Sep-94 */

	/* Signal energy is double precision Q0 */
	e_mem->signal_energy[0] = (3200000 * 16);	/* / 65536;*/ /* E. Chen: 3200000*16; Added by D. Pian 12-Sep-94 */
	e_mem->signal_energy[1] = (320000 * 16);	/* /  65536;*/ /* E. Chen: 320000*16;  and Changed 5-Apr-95 */

	e_mem->snr_stat_once = 0;

	for (j = 0; j < FREQBANDS; j++)
	{
		e_mem->band_noise_sm[j] = HIGH_THRESH_LIM;
		for (i = 0; i < FILTERORDER; i++)
		{
			e_mem->r_filt[j][i] = 0;
			/* E. Chen: may not need to initialize e_mem->r_filt
			 * for (k=0; k<FILTERORDER-i; k++) {
			 * e_mem->r_filt[j][i]+=
			 * rate_filt[j][k]*rate_filt[j][k+i];
			 * }
			 */
			e_mem->r_filt[j][i] = r_filt[j][i];
		}
	}
	for (j = 0; j < FREQBANDS; j++)
	{
		/* E. Chen: e_mem->snr[j] = e_mem->signal_energy[j]/e_mem->band_noise_sm[j]; */
		n_shiftcnt = sub(norm_l(e_mem->signal_energy[j]), 1);
		d_shiftcnt = norm_l(e_mem->band_noise_sm[j]);
		e_mem->snr[j] = L_shr(L_divide(L_shl(e_mem->signal_energy[j], n_shiftcnt), L_shl(e_mem->band_noise_sm[j], d_shiftcnt)), sub(23, sub(d_shiftcnt, n_shiftcnt)));
		e_mem->snr_map[j] = 0;
	}
}

/**********************************************************************/
/*  Band energy.                                                      */
/**********************************************************************/
void band_energy_fcn(
						INT32 *R,	/* E. Chen: float* */
						INT32 *energy,	/* E. Chen: float* */
						ENCODER_MEM * e_mem
)
{
	INT16 i, j;					/* E. Chen: int */

	/* find the energy in each band   */
	for (j = 0; j < FREQBANDS; j++)
	{
		/* E. Chen: energy[j] = e_mem->r_filt[j][0]*R[0]; */
		energy[j] = L_shr(L_mpy_ls(R[0], r_filt_x2[j][0]), 1);	/* E. Chen: zeroth entry is not normed */
		for (i = 1; i < FILTERORDER; i++)
		{
			/* E. Chen: energy[j]+=2.0*e_mem->r_filt[j][i]*R[i]; */
			energy[j] = L_add(energy[j], L_mpy_ls(R[i], r_filt_x2[j][i]));
		}
	}

}

/**********************************************************************/
/*  Update background.                                                */
/**********************************************************************/
void update_background(
						  INT16 rate,
						  ENCODER_MEM * e_mem,
						  INT16 beta	/* E. Chen: float */
)
{
	INT16 i;					/* E. Chen: int */
	INT16 *first = &evrc_current_context()->encoder.rda_update_background_first;
	INT32 snr_temp;

	/* E. Chen:  added shift counters for normalizing numerator and denominator before dividing */
	INT16 n_shiftcnt, d_shiftcnt;

	/* Updating Smooth Energy estimate Esmb(i)(k), Eq. (2.4.4.2.1-1) */
	if (*first == 0)
	{
		*first = 1;
		for (i = 0; i < FREQBANDS; i++)
		{
			e_mem->frame_energy_sm[i] = L_shl(e_mem->band_power[i], 3);
		}
	}
	else
	{
		for (i = 0; i < FREQBANDS; i++)
		{
			/* E. Chen: e_mem->frame_energy_sm[i] = SMSNR*e_mem->frame_energy_sm[i] + (1.0-SMSNR)*e_mem->band_power[i]; */
			e_mem->frame_energy_sm[i] = L_add(L_mpy_ls(L_sub(e_mem->frame_energy_sm[i],
														 L_shl(e_mem->band_power[i], 3)), SMSNR), L_shl(e_mem->band_power[i], 3));
		}
	}

/*************************************************************************/
	/* Section 2.4.4.2.2 Updating background noise                           */

	/* Now do the threshold adaptation                                       */
	/* don't adapt thresholds unless NACF < NACF_ADAP_BGN_THR over ADP frames */
	/* we're assuming the background noise here doesn't have periodicity in  */
	/* the range of typical speech                                           */

	if (beta < NACF_ADAP_BGN_THR)
	{
		/* E. Chen: e_mem->adaptcount++; */
		e_mem->adaptcount = add(e_mem->adaptcount, 1);
	}
	else
	{
		e_mem->adaptcount = 0;
	}

	if (e_mem->adaptcount > ADP)
	{
		/* change the predictor filter for the noise whitening */
		/* noise                                            */
		/* update our background noise estimate */
		for (i = 0; i < FREQBANDS; i++)
		{
			/* just increase background noise until it matches */
			/* lowest stationary period background noise       */
			/* E. Chen: if(e_mem->band_noise_sm[i]+1 < e_mem->band_noise_sm[i]*INC_FACTOR) { */
			if (add(e_mem->band_noise_sm[i], 1) < L_shl(L_mpy_ls(e_mem->band_noise_sm[i], INC_FACTOR), 1))
			{
				/* E. Chen: e_mem->band_noise_sm[i] *= INC_FACTOR; */
				e_mem->band_noise_sm[i] = L_shl(L_mpy_ls(e_mem->band_noise_sm[i], INC_FACTOR), 1);
			}
			else
			{
				/* E. Chen: e_mem->band_noise_sm[i] += 1.0; */
				e_mem->band_noise_sm[i] = add(e_mem->band_noise_sm[i], 1);
			}
		}
		e_mem->adaptcount = ADP + 1;
	}
	else
	{

		/*if SNR map est. is valid & above SNR_MAP_THRESHOLD let background noise */
		/* estimate slowly inch up like it does in IS-96.  This will allow */
		/* the rate decision to perform as well as IS-96 in babble noise and */
		/* other non-stationary noises    */

		if (e_mem->snr_stat_once == 1)
		{						/* snr is valid  */
			for (i = 0; i < FREQBANDS; i++)
			{
				/* E. Chen: if( (e_mem->snr_map[i] > SNR_MAP_THRESHOLD) &&
				 * (e_mem->band_noise_sm[i] < e_mem->frame_energy_sm[i]) ){ */
				if ((e_mem->snr_map[i] > SNR_MAP_THRESHOLD) &&
					(e_mem->band_noise_sm[i] <
					 e_mem->frame_energy_sm[i]))
				{
					/* E. Chen: if(e_mem->band_noise_sm[i]+1.0 < e_mem->band_noise_sm[i]*IS96_INC) { */
					if (add(e_mem->band_noise_sm[i], 1) <
						L_shl(L_mpy_ls(e_mem->band_noise_sm[i], IS96_INC), 1))
					{
						/* E. Chen: e_mem->band_noise_sm[i] *= IS96_INC; */
						e_mem->band_noise_sm[i] =
							L_shl(L_mpy_ls(e_mem->band_noise_sm[i], IS96_INC), 1);
					}
					else
						/* E. Chen: e_mem->band_noise_sm[i] += 1.0; */
						e_mem->band_noise_sm[i] = add(
														 e_mem->band_noise_sm[i], 1);
				}
			}
		}
	}

	/* if input energy is lower than noise estimate, */
	/* reduce background noise estimate immediately  */

	for (i = 0; i < FREQBANDS; i++)
	{
		/* Update Background Noise Estimate */
		/* E. Chen: if(e_mem->frame_energy_sm[i] < e_mem->band_noise_sm[i]) {
		 * e_mem->band_noise_sm[i] = e_mem->frame_energy_sm[i]; */
		if (e_mem->frame_energy_sm[i] < e_mem->band_noise_sm[i])
		{
			e_mem->band_noise_sm[i] = e_mem->frame_energy_sm[i];
		}
	}

	for (i = 0; i < FREQBANDS; i++)
	{
		if (e_mem->band_noise_sm[i] > HIGH_THRESH_LIM)
		{
			e_mem->band_noise_sm[i] = HIGH_THRESH_LIM;
		}
		if (e_mem->band_noise_sm[i] < LOWEST_LEVEL[i])
		{
			e_mem->band_noise_sm[i] = LOWEST_LEVEL[i];
		}
	}

	/* end of updating backgound noise energy */
/***************************************************************/

/***************************************************************/
	/* Section 2.4.4.2.2 Updating Signal Energy Estimate */
	/* pitchrun keeps track of the number of frames in a row with periodicity */

	if (beta > NACF_SOLID_VOICED)
	{
		/* E. Chen: e_mem->pitchrun += 1; */
		e_mem->pitchrun = add(e_mem->pitchrun, 1);
	}
	else
	{
		e_mem->pitchrun = 0;
	}

	if (e_mem->pitchrun > STATVOICED)
	{							/* decrease the signal energy   */
		/* if we're in a voiced segment */
		e_mem->snr_stat_once = 1;	/* confidence that we have seen speech */

		/* indicates a stationary voiced segment       */
		for (i = 0; i < FREQBANDS; i++)
		{
			/* E. Chen: e_mem->signal_energy[i] *= SCALE_DOWN_ENERGY; *//* *= 0.97 */
			e_mem->signal_energy[i] = L_mpy_ls(e_mem->signal_energy[i],
											   SCALE_DOWN_ENERGY);
		}
	}

	/* assume that the highest energy sounds are speech signals and */
	/* thus the highest energy frames contain speech and can be used */
	/* to define the SNR */

	for (i = 0; i < FREQBANDS; i++)
	{
		/* Update Signal Energy Estimate */
		if (e_mem->frame_energy_sm[i] > e_mem->signal_energy[i])
		{
			e_mem->signal_energy[i] = e_mem->frame_energy_sm[i];
		}
	}

	/* end updating signal energy estimate */
/************************************************************************/

	/* This is now done near the top of select_mode1() */
	/* update SNR estimates */

	for (i = 0; i < FREQBANDS; i++)
	{
		/* E. Chen: if(e_mem->band_noise_sm[i] > 0.0) { */
		if (e_mem->band_noise_sm[i] > 0)
		{
			/* E. Chen: e_mem->snr[i] = e_mem->signal_energy[i]/e_mem->band_noise_sm[i]; */
			/* E. Chen: assumed that e_mem->signal_energy and e_mem->band_noise_sm have  */
			/*          bits in the highest word.  this allows 'norm_l(x)' to be         */
			/*          replaced by 'norm_s(extract_h(x))'                   */
			n_shiftcnt = sub(norm_l(e_mem->signal_energy[i]), 1);
			d_shiftcnt = norm_l(e_mem->band_noise_sm[i]);
			snr_temp = L_divide(L_shl(e_mem->signal_energy[i], n_shiftcnt), L_shl(e_mem->band_noise_sm[i], d_shiftcnt));
			e_mem->snr[i] = L_shr(snr_temp, sub(23, sub(d_shiftcnt, n_shiftcnt)));
		}
		else
		{
			/* E. Chen: e_mem->snr[i] = 100000000.0; *//* a very high snr if noise =0.0 */
			e_mem->snr[i] = 256000000;
		}
	}

	if (e_mem->snr_stat_once != 0)
	{
		/* we have seen some speech and are confident in our SNR measure */
		e_mem->snr_stat_once = 1;

		/* UPDATE THIS FOR GENERAL SNR MAPS! */
		for (i = 0; i < FREQBANDS; i++)
		{

			/* E. Chen: if(e_mem->snr[i] > 0.0){ */
			if (e_mem->snr[i] > 0)
			{
				/* E. Chen: e_mem->snr_map[i] = (int) rint((10*log10(e_mem->snr[i])-20.0)/5.0); */
				snr_temp = L_sub(L_add(fnLog10(e_mem->snr[i]), 626255212), 161614248);
				e_mem->snr_map[i] = extract_h(L_add(L_sub(L_shr(
																   snr_temp, 15 - 5 - 1), 4 * 65536), 32768));
			}
			else
			{
				e_mem->snr_map[i] = 0;
			}

			if (e_mem->snr_map[i] < 0)
				e_mem->snr_map[i] = 0;
			if (e_mem->snr_map[i] > 7)
				e_mem->snr_map[i] = 7;
		}
	}
	else
	{
		/* we haven't seen speech, aren't confident in our estimate of the */
		/* signal energy and will use a nominal energy of */
		/* VOICE_INITIAL dB (-18dbm0) for our signal energy */

		for (i = 0; i < FREQBANDS; i++)
		{

			/* E. Chen: if(e_mem->band_noise_sm[i] > 0.0){ */
			if (e_mem->band_noise_sm[i] > 0)
			{
				if (i == 0)
				{
					/* E. Chen: e_mem->snr_map[i]= (int) rint((VOICE_INITIAL- 10*log10(e_mem->band_noise_sm[i])-20.0)/5.0); */
					snr_temp = L_add(fnLog10(e_mem->band_noise_sm[i]), 626255212);
					e_mem->snr_map[i] = extract_h(L_add(L_sub((((VOICE_INITIAL - 20) * 65536) / 5),
															  L_shr(snr_temp, 15 - 5 - 1)), 32768));
				}
				else
				{
					/* E. Chen: e_mem->snr_map[i]= (int) rint((VOICE_INITIAL_HI- 10*log10(e_mem->band_noise_sm[i])-20.0)/5.0); */
					snr_temp = L_add(fnLog10(e_mem->band_noise_sm[i]), 626255212);
					e_mem->snr_map[i] = extract_h(L_add(L_sub((((VOICE_INITIAL_HI - 20) * 65536) / 5),
															  L_shr(snr_temp, 15 - 5 - 1)), 32768));
				}
			}
			else
			{
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
INT16 select_rate(
					 INT32 *R_interp,	/* E. Chen: float* */
					 INT16 max_rate,
					 INT16 min_rate,
					 INT16 beta	/* E. Chen: float */
)
{
	INT16 *first = &evrc_current_context()->encoder.rda_select_rate_first;

	INT16 rate;
	ENCODER_MEM *e_mem;
	INT16 i;					/* E. Chen: int */
	INT16 k;					/* E. Chen: int */

	e_mem = &evrc_current_context()->encoder.rda_rate_mem;			/* done for rate integration with AT&T simulation */

	if (*first == 0)
	{
		/*do initialization of rate_mem */
		initialize_rate(e_mem);
		*first = 1;
	}

	/* this is the acf of the noise reducing prediction filter */

	/* now filter the input speech by bandpass filters and     */
	/* derive the necessary band energies                      */

	band_energy_fcn(R_interp, e_mem->band_power, e_mem);

	/* now our threshold comparison and background noise estimation */
	/* is done for all frequency bands                              */
	/* rate thresholds are calculated below, but not saved */

	for (k = 0; k < FREQBANDS; k++)
	{
		e_mem->band_rate[k] = EIGHTH;
		for (i = 0; i < 2; i++)
		{
			/* rate_thresh[k][i] = THRESH_SNR[k][e_mem->snr_map[k]][i]* e_mem->band_noise_sm[k]; */
			/* since we changed SMSNR to 0.6 from 0.8 */
			/* E. Chen: if (e_mem->band_power[k]>THRESH_SNR[k][e_mem->snr_map[k]][i]* e_mem->band_noise_sm[k]) */
			if (L_shl(e_mem->band_power[k], 3) > L_shl(L_mpy_ls(e_mem->band_noise_sm[k], THRESH_SNR[k][e_mem->snr_map[k]][i]), 8 - 1))
			{
				if (e_mem->band_rate[k] == EIGHTH)
				{
					e_mem->band_rate[k] = HALFRATE_VOICED;
				}
				else if (e_mem->band_rate[k] == HALFRATE_VOICED)
				{
					e_mem->band_rate[k] = FULLRATE_VOICED;
				}
			}
		}
	}							/* do threshold comparisons for full and half rates  */

	/* use 10log10(e_mem->band_power[k]/band_noise_sm[k]) */

	/* the maximum rate for all bands is chosen as the rate */
	rate = e_mem->band_rate[0];
	for (k = 1; k < FREQBANDS; k++)
	{
		if (e_mem->band_rate[k] > rate)
			rate = e_mem->band_rate[k];
	}

	/* Section 2.4.4.1.3 Calculating Hangover Frames as Function of SNR */
	if ((e_mem->num_full_frames > FULL_THRESH) || e_mem->hangover_in_progress == 1)
	{
		/* only use hangover after so many full rate frames in a row */
		if (rate != FULLRATE_VOICED)
		{
			/* E. Chen: e_mem->hangover += 1; */
			e_mem->hangover = add(e_mem->hangover, 1);
			if (e_mem->hangover <= hangover[e_mem->snr_map[0]])
			{
				rate = FULLRATE_VOICED;
				e_mem->hangover_in_progress = 1;
			}
			else
			{
				e_mem->hangover_in_progress = 0;
			}
		}
		else
		{
			e_mem->hangover_in_progress = 0;
		}
	}

	if ((rate == FULLRATE_VOICED) && (e_mem->hangover_in_progress == 0))
	{
		e_mem->hangover = 0;
		/* E. Chen: e_mem->num_full_frames+=1; */
		e_mem->num_full_frames = add(e_mem->num_full_frames, 1);
	}
	else
	{
		e_mem->num_full_frames = 0;
	}
	/* end of hangover algorithm     */

	/* assuming only 1/8,1/2,and full rates pluse some 1/2 and full rates modes */
	/* if last rate is full force 1/2 rate before going to 1/8                  */

	/* Section 2.4.4.1.4 Constraints on Rate Selection */
	if ((e_mem->last_rate_1st_stage == FULLRATE_VOICED) && rate == EIGHTH)
	{
		rate = HALFRATE_VOICED;
	}

	/* allows fast convergence for EIGHTH rate */
	if (e_mem->frame_num == 0)
	{
		rate = FULLRATE_VOICED;
	}

	if (rate > max_rate)
	{
		rate = max_rate;
	}
	if (rate < min_rate)
	{
		rate = min_rate;
	}

	e_mem->last_rate_1st_stage = rate;
	e_mem->last_rate = rate;

	/* update background noise and signal energy estimates */
	update_background(rate, e_mem, beta);

	/* E. Chen: e_mem->frame_num += 1; */
	e_mem->frame_num = add(e_mem->frame_num, 1);

	return (rate);
}
