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
/* ns127.c */
/*****************************************************************
*
* EVRC Noise Suppression
*
* Input:  The input to the function is a Shortword pointer to the
*         array of data to be noise suppressed.
*
* Output: There is no return value.  The input array is replaced
*         with the noise suppressed values.
*
*
* Written by:                   Tenkasi V. Ramabadran
* Date:                         December 28, 1994
*
* Last Modified:		James Ashley
* Date:				November 7, 1996
*
* Version    Date      Description
*
*   1.0    12/01/95    Released to TIA TR45.5.1.1
*   1.1    02/14/96    Init Noise to first 4 frames      
*   1.2    02/19/96    Bug fix in frame_cnt declaration  
*          03/27/96    Revised for Fixed Point Calculations
*          05/28/96    Adjust thresholds for -6 dB input level,
*                      add block_norm/denorm around FFT/IFFT,
*                      general scaling cleanup.
*   1.3    08/09/96    Adjusted thresholds to 0dB input level, 
*                      Modified block_denormalization to provide 6 db adjustment.
*   1.4    10/15/96    Apply block_norm call to farray_ptr instead of data_buffer
*                      (fixes high level input problems). (JPA)
*   1.5    11/07/96    Fix bug in block_norm(). (JPA)
*****************************************************************/
#include <stdio.h>

/* Includes */
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"
#include "evrc_state.h"

/* Defines */
#define         FRM_LEN                 80
#define         DELAY                   24
#define         FFT_LEN                 128

#define         NUM_CHAN                16
#define         LO_CHAN                 0
#define         MID_CHAN                5
#define         HI_CHAN                 15

#define         TRUE                    1
#define         FALSE                   0

#define         UPDATE_THLD             35
#define         METRIC_THLD             45
#define         INDEX_THLD              12
#define         SETBACK_THLD            12
#define         SNR_THLD                6
#define         INDEX_CNT_THLD          5
#define         HYSTER_CNT_THLD         6
#define         UPDATE_CNT_THLD         50

/* Define the following if 0db input to ns */
#define		INPUT_0_DB
#ifdef		INPUT_MINUS_6_DB

#define         NOISE_FLOOR_CHAN        64	/* 1.0/4 scaled as 23,8 */
#define         MIN_CHAN_ENRG           4	/* 0.0625/4 scaled as 23,8 */
#define         MIN_NOISE_ENRG          4	/* 0.0625/4 scaled as 23,8 */
#define         INE_CHAN                1024	/* 16.0/4 scaled as 23,8 */
#define         INE_NOISE               1024	/* 16.0/4 scaled as 23,8 */

#define         HIGH_TCE_DB             1408	/* 44.0 scaled as 10,5 */
#define         LOW_TCE_DB              768	/* 24.0 scaled as 10,5 */

#endif
#ifdef		INPUT_0_DB

#define         NOISE_FLOOR_CHAN        256	/* 1.0 scaled as 23,8 */
#define         MIN_CHAN_ENRG           16	/* 0.0625 scaled as 23,8 */
#define         MIN_NOISE_ENRG          16	/* 0.0625 scaled as 23,8 */
#define         INE_CHAN                4096	/* 16.0 scaled as 23,8 */
#define         INE_NOISE               4096	/* 16.0 scaled as 23,8 */

#define         HIGH_TCE_DB             1600	/* 50.0 scaled as 10,5 */
#define         LOW_TCE_DB              960	/* 30.0 scaled as 10,5 */

#endif



#define         TCE_RANGE               (HIGH_TCE_DB - LOW_TCE_DB)
#define         HIGH_ALPHA_S5_10        1023
#define         LOW_ALPHA_S5_10         511
#define         HIGH_ALPHA              32440	/* 0.99 scaled as 0,15 */
#define         LOW_ALPHA               16383	/* 0.50 scaled as 0,15 */

#define         ALPHA_RANGE             (HIGH_ALPHA - LOW_ALPHA)
#define         ALPHA_RAN_DIV_TCE_RAN   803		/* (0.99 - 0.50) / 20 scaled as 0,15 */
#define         DEV_THLD                896		/* 28.0 scaled as 10,5 */
#define         PRE_EMP_FAC             -26214	/* -0.8 scaled as 0,15 */
#define         CEE_SM_FAC              1181116006	/* 0.55 scaled as 0,31 */
#define         ONE_MINUS_CEE_SM_FAC    966367642	/* 0.55 scaled as 0,31 */
#define         MIN_GAIN                -27262976	/* (-13.0) scaled as 11,20 */
#define         GAIN_SLOPE              14879	/* 0.45 scaled as 0,15 */
#define         CNE_SM_FAC              3277	/* 0.1 scaled as 0,15 */

#define         ONE_MINUS_CNE_SM_FAC    (32767-CNE_SM_FAC)
#define         DE_EMP_FAC              26214	/* 0.8 scaled as 0,15 */
#define         LOG_OFFSET              626255212	/* 9.3319 scaled as 5,26 */
#define         ONE_OVER_20             1638	/* 1/20 scaled as 0,15 */
#define         TEN_S5_10               10270	/* 10.0 scaled as 5,10 */
#define         CONST_0_1875_S10_21     393216	/* 0.1875 scaled as 10,21 */
#define         CONST_2_667_S5_10       2731	/* 2.667 scaled as 5,10 */

#define         FFT_HEADROOM            2
#define         IFFT_HEADROOM           4

/* Local functions */

Shortword block_norm(Shortword * data, Shortword size, Shortword headroom)
{

	Shortword i, max, scnt, adata;

	//max = abs(data[0]);
	max = abs_s(data[0]);		//maolin 2014-07-22
	for (i = 1; i < size; i++)
	{
		//adata = abs(data[i]);
		adata = abs_s(data[i]);		//maolin 2014-07-22
		if (adata > max)
			max = adata;
	}
	scnt = norm_s(max) - headroom;
	for (i = 0; i < size; i++)
	{
		data[i] = shift_r(data[i], scnt);
	}
	return (scnt);

}

void block_denorm(Shortword * data, Shortword size, Shortword scnt)
{

	Shortword i;

	for (i = 0; i < size; i++)
	{
		data[i] = shift_r(data[i], negate(scnt));
	}
	return;

}

/* The noise supression function */
void noise_suprs(Shortword * farray_ptr)
{

/*
 * The channel table is defined below.  In this table, the
 * lower and higher frequency coefficients for each of the 16
 * channels are specified.  The table excludes the coefficients
 * with numbers 0 (DC), 1, and 64 (Foldover frequency).  For
 * these coefficients, the gain is always set at 1.0 (0 dB).
 */

	static const Shortword ch_tbl[NUM_CHAN][2] =
	{

		{2, 3},
		{4, 5},
		{6, 7},
		{8, 9},
		{10, 11},
		{12, 13},
		{14, 16},
		{17, 19},
		{20, 22},
		{23, 26},
		{27, 30},
		{31, 35},
		{36, 41},
		{42, 48},
		{49, 55},
		{56, 63}

	};

	static const Shortword ch_tbl_sh[NUM_CHAN][2] =
	{

		{TRUE, 1},
		{TRUE, 1},
		{TRUE, 1},
		{TRUE, 1},
		{TRUE, 1},
		{TRUE, 1},
		{FALSE, 10922},
		{FALSE, 10922},
		{FALSE, 10922},
		{TRUE, 2},
		{TRUE, 2},
		{FALSE, 6553},
		{FALSE, 5462},
		{FALSE, 4681},
		{FALSE, 4681},
		{TRUE, 3}

	};

/*
 * The voice metric table is defined below.  It is a non-
 * linear table with a deadband near zero.  It maps the SNR
 * index (quantized SNR value) to a number that is a measure
 * of voice quality.
 */

	static const int vm_tbl[90] =
	{

		2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
		3, 3, 3, 3, 3, 4, 4, 4, 5, 5, 5, 6, 6, 7, 7, 7,
		8, 8, 9, 9, 10, 10, 11, 12, 12, 13, 13, 14, 15,
		15, 16, 17, 17, 18, 19, 20, 20, 21, 22, 23, 24,
		24, 25, 26, 27, 28, 28, 29, 30, 31, 32, 33, 34,
		35, 36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 45,
		46, 47, 48, 49, 50, 50, 50, 50, 50, 50, 50, 50,
		50, 50

	};

	static const Shortword window[DELAY + FRM_LEN] =
	{

		35, 315, 869, 1690, 2761, 4066, 5581, 7281, 9137,
		11117, 13187, 15312, 17455, 19580, 21650, 23630,
		25486, 27186, 28701, 30006, 31077, 31898, 32452,
		32732, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32767, 32767, 32767, 32767, 32767, 32767,
		32767, 32732, 32452, 31898, 31077, 30006, 28701,
		27186, 25486, 23630, 21650, 19580, 17455, 15312,
		13187, 11117, 9137, 7281, 5581, 4066, 2761, 1690,
		869, 315, 35

	};

#define first (evrc_current_context()->encoder.ns_first)
#define pre_emp_mem (evrc_current_context()->encoder.ns_pre_emp_mem)
#define de_emp_mem (evrc_current_context()->encoder.ns_de_emp_mem)
#define overlap (evrc_current_context()->encoder.ns_overlap)
#define ch_gain (evrc_current_context()->encoder.ns_ch_gain)
#define update_cnt (evrc_current_context()->encoder.ns_update_cnt)
#define window_overlap (evrc_current_context()->encoder.ns_window_overlap)
#define hyster_cnt (evrc_current_context()->encoder.ns_hyster_cnt)
#define last_update_cnt (evrc_current_context()->encoder.ns_last_update_cnt)
#define ch_enrg_long_db (evrc_current_context()->encoder.ns_ch_enrg_long_db)
#define frame_cnt (evrc_current_context()->encoder.ns_frame_cnt)
#define ch_enrg (evrc_current_context()->encoder.ns_ch_enrg)
#define ch_noise (evrc_current_context()->encoder.ns_ch_noise)
#define last_normb_shift (evrc_current_context()->encoder.ns_last_normb_shift)

	Longword enrg;				/* scaled as 30,1 */
	Longword tne;				/* scaled as 15,16 (change to 23,8) */
	Longword tce;				/* scaled as 23,8 */
	Longword gain;				/* scaled as 11,20 */

	Shortword data_buffer[FFT_LEN];
	Shortword ch_snr[NUM_CHAN];	/* scaled as 15,0 */
	Shortword ftmp2;			/* scaled as 0,15 */
	Shortword vm_sum;			/* scaled as 15,0 */
	Shortword ch_enrg_dev;		/* scaled as 10,5 */
	Shortword ch_enrg_db[NUM_CHAN];		/* scaled as 10,5 */
	Shortword alpha;			/* scaled as 0,15 */

	int i, j, j1, j2;
	int update_flag, modify_flag, index_cnt;

	Longword Ltmp, Ltmp1, Ltmp2, Ltmp3;
	Shortword tmp, tmp1, tmp2, norm_shift, norm_shift1;

	Shortword normb_shift;		/* block norm shift count */


/* Functions */
	void r_fft(short *, short);

/* Init the window function, channel gains one time */
	if (first == TRUE)
	{
		ch_gain[0] = ch_gain[1] = SW_MAX;
		for (i = LO_CHAN; i <= HI_CHAN; i++)
			ch_enrg[i] = 0;
		for (i = 0; i < DELAY; i++)
			window_overlap[i] = 0;
		for (i = 0; i < FFT_LEN - FRM_LEN; i++)
			overlap[i] = 0;
		pre_emp_mem = 0;
		de_emp_mem = 0;
		update_cnt = 0;
		frame_cnt = 0;
	}

/* Increment frame counter */
	frame_cnt++;

/* Block normalize the input */
	normb_shift = block_norm(farray_ptr, FRM_LEN, FFT_HEADROOM);

/*
 * Preemphasize the input data and store in the data buffer with
 * appropriate delay 
 */
	for (i = 0; i < DELAY; i++)
		data_buffer[i] = shift_r(window_overlap[i], normb_shift-last_normb_shift);

	pre_emp_mem = shift_r(pre_emp_mem, normb_shift-last_normb_shift);
	last_normb_shift = normb_shift;

	data_buffer[DELAY] = add(*farray_ptr, mult(PRE_EMP_FAC, pre_emp_mem));

	for (i = DELAY + 1, j = 1; i < DELAY + FRM_LEN; i++, j++)
		data_buffer[i] = add(*(farray_ptr + j), mult(PRE_EMP_FAC, *(farray_ptr + j - 1)));

	pre_emp_mem = *(farray_ptr + FRM_LEN - 1);

	for (i = DELAY + FRM_LEN; i < FFT_LEN; i++)
		data_buffer[i] = 0;

/* update window_overlap buffer */
	for (i = 0, j = FRM_LEN; i < DELAY; i++, j++)
		window_overlap[i] = data_buffer[j];

/* Apply window to frame prior to FFT */
	for (i = 0; i < FRM_LEN + DELAY; i++)
		data_buffer[i] = mult_r(data_buffer[i], window[i]);

/* Perform FFT on the data buffer */
	r_fft(data_buffer, +1);

/* Estimate the energy in each channel */
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		enrg = 0;
		j1 = ch_tbl[i][0];
		j2 = ch_tbl[i][1];

		for (j = j1; j <= j2; j++)
		{
			enrg = L_mac(enrg, data_buffer[2 * j], data_buffer[2 * j]);
			enrg = L_mac(enrg, data_buffer[2 * j + 1], data_buffer[2 * j + 1]);
		}

		if (ch_tbl_sh[i][0] == TRUE)
			enrg = L_shr(enrg, ch_tbl_sh[i][1]);
		else
		{
			norm_shift = norm_l(enrg);
			tmp = extract_h(L_shl(enrg, norm_shift));
			enrg = L_mult(tmp, ch_tbl_sh[i][1]);
			enrg = L_shr(enrg, norm_shift);
		}

		if (first == TRUE)
			ch_enrg[i] = L_shl(enrg, 7 - (2 * normb_shift));	/* rescaled from 30,1 to 23,8 (w/block denorm) */
		else
		{
			norm_shift = norm_l(enrg);
			Ltmp1 = L_shl(enrg, norm_shift);
			Ltmp1 = L_mpy_ls(CEE_SM_FAC, extract_h(Ltmp1));
			Ltmp1 = L_shr(Ltmp1, norm_shift);
			Ltmp2 = L_shl(Ltmp1, 7 - (2 * normb_shift));	/* rescaled from 30,1 to 23,8 (w/block denorm) */

			norm_shift = norm_l(ch_enrg[i]);
			Ltmp1 = L_shl(ch_enrg[i], norm_shift);
			Ltmp3 = L_mpy_ls(ONE_MINUS_CEE_SM_FAC, extract_h(Ltmp1));
			Ltmp3 = L_shr(Ltmp3, norm_shift);

			ch_enrg[i] = L_add(Ltmp3, Ltmp2);
		}

		if (ch_enrg[i] < MIN_CHAN_ENRG)
			ch_enrg[i] = MIN_CHAN_ENRG;

	}

/* Initialize channel noise estimate to channel energy of first four frames */
	if (frame_cnt <= 4)
	{
		for (i = LO_CHAN; i <= HI_CHAN; i++)
		{
			if (ch_enrg[i] < INE_CHAN)
				ch_noise[i] = INE_NOISE;
			else
				ch_noise[i] = ch_enrg[i];
		}
	}

/* Compute the channel SNR indices */
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{

		norm_shift = norm_l(ch_noise[i]);
		Ltmp = L_shl(ch_noise[i], norm_shift);

		norm_shift1 = norm_l(ch_enrg[i]);
		Ltmp3 = L_shl(ch_enrg[i], norm_shift1 - 1);

		Ltmp2 = L_divide(Ltmp3, Ltmp);
		Ltmp2 = L_shr(Ltmp2, 27 - 1 + norm_shift1 - norm_shift);	/* scaled as 27,4 */

		if (Ltmp2 == 0)
			Ltmp2 = 1;

		Ltmp1 = fnLog10(Ltmp2);
		Ltmp3 = L_add(Ltmp1, LOG_OFFSET - 80807124);	/* -round32(log10(2^4)*2^26 */
		Ltmp2 = L_mult(TEN_S5_10, extract_h(Ltmp3));
		if (Ltmp2 < 0)
			Ltmp2 = 0;
		/* 0.1875 scaled as 10,21 */
		Ltmp1 = L_add(Ltmp2, CONST_0_1875_S10_21);
		/* tmp / 0.375  2.667 scaled as 5,10, Ltmp is scaled 15,16 */
		Ltmp = L_mult(extract_h(Ltmp1), CONST_2_667_S5_10);
		ch_snr[i] = extract_h(Ltmp);

	}

/* Compute the sum of voice metrics */
	vm_sum = 0;
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		if (ch_snr[i] < 89)
			j = ch_snr[i];
		else
			j = 89;

		vm_sum = add(vm_sum, vm_tbl[j]);
	}
/* Compute the total noise estimate (tne) and total channel energy estimate (tce) */
	tne = tce = 0;
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		tne = L_add(tne, ch_noise[i]);
		tce = L_add(tce, ch_enrg[i]);
	}

/* Calculate log spectral deviation */
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		Ltmp = ch_enrg[i];
		if (Ltmp == 0)
			Ltmp = 1;
		Ltmp1 = fnLog10(Ltmp);
		Ltmp2 = L_add(Ltmp1, LOG_OFFSET - 161614248);	/* -round32(log10(2^8)*2^26) */
		ch_enrg_db[i] = mult(TEN_S5_10, extract_h(Ltmp2));
	}

	if (first == TRUE)
		for (i = LO_CHAN; i <= HI_CHAN; i++)
			ch_enrg_long_db[i] = ch_enrg_db[i];

	ch_enrg_dev = 0;
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		tmp = abs_s(sub(ch_enrg_long_db[i], ch_enrg_db[i]));
		ch_enrg_dev = add(ch_enrg_dev, tmp);
	}

/*
 * Calculate long term integration constant as a function of total channel energy (tce) 
 * (i.e., high tce (-40 dB) -> slow integration (alpha = 0.99),
 *         low tce (-60 dB) -> fast integration (alpha = 0.50)
 */

	Ltmp1 = fnLog10(tce);
	Ltmp2 = L_add(Ltmp1, LOG_OFFSET - 161614248);	/* -round32(log10(2^8)*2^26) */
	tmp = mult(TEN_S5_10, extract_h(Ltmp2));

	tmp2 = sub(HIGH_TCE_DB, tmp);	/* HIGH_TCE_DB and tmp scaled as 10,5 */
	tmp2 = shl(tmp2, 5);		/* move scale to 5,10 get more fraction */
	tmp1 = mult(ALPHA_RAN_DIV_TCE_RAN, tmp2);
	alpha = sub(HIGH_ALPHA_S5_10, tmp1);

	if (alpha > HIGH_ALPHA_S5_10)
		alpha = HIGH_ALPHA;
	else if (alpha < LOW_ALPHA_S5_10)
		alpha = LOW_ALPHA;
	else
		alpha = shl(alpha, 5);	/* rescale from 5,10 to 0,15 alpha is a fraction */

/* Calc long term log spectral energy */
	tmp = sub(SW_MAX, alpha);
	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		Ltmp1 = L_mult(tmp, ch_enrg_db[i]);
		Ltmp2 = L_mult(alpha, ch_enrg_long_db[i]);
		ch_enrg_long_db[i] = extract_h(L_add(Ltmp1, Ltmp2));
	}

/* Set or reset the update flag */
	update_flag = FALSE;

	if (vm_sum <= UPDATE_THLD)
	{
		update_flag = TRUE;
		update_cnt = 0;
	}
	else if (tce > NOISE_FLOOR_CHAN && ch_enrg_dev < DEV_THLD)
	{
		update_cnt++;
		if (update_cnt >= UPDATE_CNT_THLD)
			update_flag = TRUE;
	}

	if (update_cnt == last_update_cnt)
		hyster_cnt++;
	else
		hyster_cnt = 0;

	last_update_cnt = update_cnt;

	if (hyster_cnt > HYSTER_CNT_THLD)
		update_cnt = 0;

/* Set or reset modify flag */
	index_cnt = 0;

	for (i = MID_CHAN; i <= HI_CHAN; i++)
		if (ch_snr[i] >= INDEX_THLD)
			index_cnt++;

	modify_flag = (index_cnt < INDEX_CNT_THLD) ? TRUE : FALSE;

/* Modify the SNR indices */
	if (modify_flag == TRUE)
	{
		for (i = LO_CHAN; i <= HI_CHAN; i++)
			if ((vm_sum <= METRIC_THLD) || (ch_snr[i] <= SETBACK_THLD))
				ch_snr[i] = 1;
	}

/* Compute the channel gains */
	Ltmp1 = fnLog10(tne);
	Ltmp1 = L_add(Ltmp1, LOG_OFFSET - 161614248);	/* -round32(log10(2^8)*2^26) */
	Ltmp1 = L_negate(Ltmp1);
	gain = L_mpy_ls(Ltmp1, TEN_S5_10);
	if (gain < MIN_GAIN)
		gain = MIN_GAIN;

	for (i = LO_CHAN; i <= HI_CHAN; i++)
	{
		if (ch_snr[i] <= SNR_THLD)
			ch_snr[i] = SNR_THLD;

		tmp = sub(ch_snr[i], SNR_THLD);
		Ltmp1 = L_mult(tmp, GAIN_SLOPE);
		Ltmp2 = L_shl(Ltmp1, 5);	/* rescaled to 10,5 */
		Ltmp = L_add(Ltmp2, gain);	/* gain scaled as 10,5 */
		if (Ltmp > 0)
			Ltmp = 0;

		Ltmp1 = L_mpy_ls(Ltmp, ONE_OVER_20);
		Ltmp1 = L_shl(Ltmp1, 5);	/* rescale Ltmp1 to 5,26 */
		if (Ltmp1 == 0)
			Ltmp1 = -1;
		Ltmp2 = fnExp10(Ltmp1);
		ftmp2 = extract_h(Ltmp2);

		j1 = ch_tbl[i][0], j2 = ch_tbl[i][1];
		for (j = j1; j <= j2; j++)
		{
			ch_gain[j] = ftmp2;
		}
	}

/* Update the channel noise estimates */

	if (update_flag == TRUE)
	{
		for (i = LO_CHAN; i <= HI_CHAN; i++)
		{
			norm_shift = norm_l(ch_noise[i]);
			Ltmp = L_shl(ch_noise[i], norm_shift);
			Ltmp1 = L_mult(extract_h(Ltmp), ONE_MINUS_CNE_SM_FAC);
			Ltmp1 = L_shr(Ltmp1, norm_shift);

			norm_shift = norm_l(ch_enrg[i]);
			Ltmp = L_shl(ch_enrg[i], norm_shift);
			Ltmp2 = L_mult(extract_h(Ltmp), CNE_SM_FAC);
			Ltmp2 = L_shr(Ltmp2, norm_shift);

			ch_noise[i] = L_add(Ltmp1, Ltmp2);

			if (ch_noise[i] < MIN_NOISE_ENRG)
				ch_noise[i] = MIN_NOISE_ENRG;
		}
	}

/* Filter the input data in the frequency domain and perform IFFT */

	for (i = 0; i < FFT_LEN / 2; i++)
	{
		data_buffer[2 * i] = mult(data_buffer[2 * i], ch_gain[i]);
		data_buffer[2 * i + 1] = mult(data_buffer[2 * i + 1], ch_gain[i]);
	}

/* Block normalize data_buffer */
	norm_shift = block_norm(data_buffer, FFT_LEN, IFFT_HEADROOM);

/* Inverse FFT */
	r_fft(data_buffer, -1);

/* Block denormalize data_buffer */
/*	block_denorm(data_buffer, FFT_LEN, normb_shift + norm_shift); */
	block_denorm(data_buffer, FFT_LEN, normb_shift + norm_shift + 1); 

/* Overlap add the filtered data from previous block.
 * Save data from this block for the next. */

	for (i = 0; i < FFT_LEN - FRM_LEN; i++)
		data_buffer[i] = add(data_buffer[i], overlap[i]);

	for (i = FRM_LEN; i < FFT_LEN; i++)
		overlap[i - FRM_LEN] = data_buffer[i];

/* Deemphasize the filtered speech and write it out to farray */
	tmp = mult(DE_EMP_FAC, de_emp_mem);
	*farray_ptr = add(data_buffer[0], tmp);

	for (i = 1; i < FRM_LEN; i++)
	{
		tmp = mult_r(DE_EMP_FAC, *(farray_ptr + i - 1));
		*(farray_ptr + i) = add(data_buffer[i], tmp);
	}

	de_emp_mem = *(farray_ptr + FRM_LEN - 1);

	first = FALSE;

#undef last_normb_shift
#undef ch_noise
#undef ch_enrg
#undef frame_cnt
#undef ch_enrg_long_db
#undef last_update_cnt
#undef hyster_cnt
#undef window_overlap
#undef update_cnt
#undef ch_gain
#undef overlap
#undef de_emp_mem
#undef pre_emp_mem
#undef first
}								/* end noise_suprs () */


