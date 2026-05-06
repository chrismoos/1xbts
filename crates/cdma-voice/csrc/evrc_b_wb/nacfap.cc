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
    "$Id: nacfap.cc,v 1.8.22.4 2007/06/24 06:43:15 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/* This routine computes nacf at pitch for 2 subframes (1 at current frame and 1 at look-ahead   frame. Input residual is low-passed with cut-off at 1k and sub-sample by 2 */


#include "struct.h"
#include "filt.h"
#include "macro.h"

static float rhpf_num_coef[RHPF_ORDER + 1] = {
    7425.0 / 8192.0,
    -22274.0 / 8192.0,
    22274.0 / 8192.0,
    -7425.0 / 8192.0
};
static float rhpf_den_coef[RHPF_ORDER + 1] = {
    8192.0 / 8192.0,
    -22965.0 / 8192.0,
    21511.0 / 8192.0,
    -6730.0 / 8192.0
};

static float rlpf_coef[RLPF_ORDER + 1] = { 0.00679069413153213,
    0.0151687288718389,
    0.0394709096395305,
    0.0779828776055001,
    0.121184903661687,
    0.155283877206347,
    0.168236017767129,
    0.155283877206347,
    0.121184903661687,
    0.0779828776055001,
    0.0394709096395305,
    0.0151687288718389,
    0.00679069413153213
};
float
FGV_MEM::get_nacf_at_pitch (float *resid, int pitch0, int pitch1, float *nacf)
{
    int i, j, k;

    float *decim_buf = new float[FSIZE + LOOKAHEAD_LEN];
    float *ptr;
    double Exx, Eyy, Exy;
    float xcorr, maxcorr;
    float max;
    int wl;
    int pitch[2];

    pitch[0] = pitch0;
    pitch[1] = pitch1;
    float rlpf_filt_mem_saved[RLPF_ORDER];
    ZERO_FILTER rlpf_filter = { RLPF_ORDER, 0, rlpf_filt_mem };
    double rhpf_filt_mem_saved[RHPF_ORDER];
    POLEZERO_FILTER rhpf_filter =
	{ RHPF_ORDER, RHPF_ORDER, RHPF_ORDER, 0, rhpf_filt_mem };

    if (get_nacf_at_pitch_FirstTime) {
	get_nacf_at_pitch_FirstTime = 0;
	for (i = 0; i < LPFRMEM_LEN; i++)
	    lpfr[i] = 0;

	for (i = 0; i < RLPF_ORDER; i++)
	    rlpf_filt_mem[i] = 0;
	for (i = 0; i < RHPF_ORDER; i++)
	    rhpf_filt_mem[i] = 0;
    }

    polezero_filter (resid, decim_buf, FSIZE, rhpf_num_coef, rhpf_den_coef,
		     rhpf_filter);
    for (i = 0; i < RHPF_ORDER; i++)
	rhpf_filt_mem_saved[i] = rhpf_filt_mem[i];
    polezero_filter (resid + FSIZE, decim_buf + FSIZE, LOOKAHEAD_LEN,
		     rhpf_num_coef, rhpf_den_coef, rhpf_filter);
    for (i = 0; i < RHPF_ORDER; i++)
	rhpf_filt_mem[i] = rhpf_filt_mem_saved[i];

    zero_filter (decim_buf, decim_buf, FSIZE, rlpf_coef, rlpf_filter);
    for (i = 0; i < RLPF_ORDER; i++)
	rlpf_filt_mem_saved[i] = rlpf_filt_mem[i];
    zero_filter (decim_buf + FSIZE, decim_buf + FSIZE, LOOKAHEAD_LEN,
		 rlpf_coef, rlpf_filter);
    for (i = 0; i < RLPF_ORDER; i++)
	rlpf_filt_mem[i] = rlpf_filt_mem_saved[i];
    ptr = lpfr + LPFRMEM_LEN;
    for (i = 0; i < (FSIZE + LOOKAHEAD_LEN) / SUBSAMP; i++)
	ptr[i] = decim_buf[SUBSAMP * i];

    delete[]decim_buf;

    for (k = 0; k < 2; k++) {	// compute nacf around pitch at 2 subframes   
	Exx = Eyy = Exy = 0;
	maxcorr = 0;
	ptr = lpfr + LPFRMEM_LEN + (k + 1) * FSIZE / SUBSAMP / 2;
	for (i = 0; i < FSIZE / SUBSAMP / 2; i++)
	    Exx += SQR (ptr[i]);
	Exx = (Exx == 0) ? 1.0 : Exx;
	wl = (MAX (6, (int) MIN (anint (0.2 * pitch[k]), 16)) + 1) / SUBSAMP;

	for (i = -wl; i <= wl; i++) {
	    Eyy = Exy = 0;
	    for (j = 0; j < FSIZE / SUBSAMP / 2; j++) {
		Eyy += SQR (ptr[i + j - pitch[k] / SUBSAMP]);
		Exy += ptr[j] * ptr[i + j - pitch[k] / SUBSAMP];
	    }
	    Eyy = (Eyy == 0) ? 1.0 : Eyy;
	    xcorr = SIGN (Exy) * Exy * Exy / Exx / Eyy;
	    if (xcorr > maxcorr)
		maxcorr = xcorr;
	}
	nacf[k] = maxcorr;
    }

    // Compute the last nacf with full search (not pitch based)
    Exx = Eyy = Exy = 0;
    maxcorr = 0;
    ptr = lpfr + LPFRMEM_LEN + (FSIZE / 2 + LOOKAHEAD_LEN) / SUBSAMP;
    for (i = 0; i < FSIZE / SUBSAMP / 2; i++)
	Exx += SQR (ptr[i]);
    Exx = (Exx == 0) ? 1.0 : Exx;
    for (i = DMIN / SUBSAMP; i < DMAX / SUBSAMP; i++) {
	Eyy = Exy = 0;
	for (j = 0; j < FSIZE / SUBSAMP / 2; j++) {
	    Eyy += SQR (ptr[j - i]);
	    Exy += ptr[j] * ptr[j - i];
	}
	Eyy = (Eyy == 0) ? 1.0 : Eyy;
	xcorr = SIGN (Exy) * Exy * Exy / Exx / Eyy;
	if (xcorr > maxcorr)
	    maxcorr = xcorr;
    }
    nacf[2] = maxcorr;

    // Search for the best lag and nacf on 4 sub-frame nacf track
    {
	double corr[2][D_RANGE];
	double next_corr[2][D_RANGE];
	double *Eyy = new double[LPFRMEM_LEN];
	double *Exy = new double[D_RANGE];
	double *Exyptr;

	double rightmax[D_RANGE];
	double maxcorr[D_RANGE * SUBSAMP];
	static int FAN[D_RANGE][2] = {
	    {0, 2}, {0, 3}, {1, 3}, {2, 3}, {2, 4}, {3, 4}, {4, 4}, {5, 4},
		{6, 5}, {7, 5}, {8, 5},
	    {8, 6}, {9, 6}, {10, 6}, {11, 6}, {12, 7}, {13, 7}, {14, 7}, {15,
									  8},
		{15, 8}, {16, 8},
	    {17, 9}, {18, 9}, {19, 9}, {20, 9}, {20, 10}, {21, 10}, {22, 10},
		{23, 11}, {24, 11},
	    {25, 11}, {25, 12}, {26, 12}, {27, 12}, {28, 12}, {29, 13}, {30,
									 13},
		{31, 13},
	    {31, 14}, {32, 14}, {33, 14}, {33, 15}, {34, 15}, {35, 15}, {36,
									 15},
		{37, 16},
	    {37, 16}, {38, 15}, {39, 14}, {40, 13}, {41, 12}, {42, 11}, {43,
									 10}
	};
	static double corr_filt[4] = { -0.0625, 0.5625, 0.5625, -0.0625 };

	// First subframe of current frame
	ptr = lpfr + LPFRMEM_LEN;
	Eyy[0] = 0.0;
	for (i = 0; i < FSIZE / 2 / SUBSAMP; i++)
	    Eyy[0] += SQR (ptr[i]);
	for (i = 1; i < LPFRMEM_LEN; i++, ptr--) {
	    Eyy[i] =
		Eyy[i - 1] + SQR (ptr[-1]) -
		SQR (ptr[FSIZE / 2 / SUBSAMP - 1]);
	}
	ptr = lpfr + LPFRMEM_LEN;
	Exyptr = Exy;
	for (i = (DMIN - 4) / SUBSAMP; i < (DMAX + 2) / SUBSAMP; i++) {
	    *Exyptr = 0.0;
	    for (j = 0; j < FSIZE / 2 / SUBSAMP; j++)
		*Exyptr += ptr[j] * (*(ptr - i + j));
	    Exyptr++;
	}

	Eyy[0] = ((Eyy[0] == 0.0) ? 1.0 : Eyy[0]);
	for (i = 0, j = (DMIN - 4) / SUBSAMP; i < D_RANGE; i++, j++) {
	    Eyy[j] = ((Eyy[j] == 0.0) ? 1.0 : Eyy[j]);
	    corr[0][i] = SIGN (Exy[i]) * SQR (Exy[i]) / Eyy[0] / Eyy[j];
	}
	// Second subframe of current frame
	ptr = lpfr + LPFRMEM_LEN + FSIZE / 2 / SUBSAMP;
	Eyy[0] = 0.0;
	for (i = 0; i < FSIZE / 2 / SUBSAMP; i++)
	    Eyy[0] += SQR (ptr[i]);
	for (i = 1; i < LPFRMEM_LEN; i++, ptr--) {
	    Eyy[i] =
		Eyy[i - 1] + SQR (ptr[-1]) -
		SQR (ptr[FSIZE / 2 / SUBSAMP - 1]);
	}
	ptr = lpfr + LPFRMEM_LEN + FSIZE / 2 / SUBSAMP;
	Exyptr = Exy;
	for (i = (DMIN - 4) / SUBSAMP; i < (DMAX + 2) / SUBSAMP; i++) {
	    *Exyptr = 0.0;
	    for (j = 0; j < FSIZE / 2 / SUBSAMP; j++)
		*Exyptr += ptr[j] * (*(ptr - i + j));
	    Exyptr++;
	}

	Eyy[0] = ((Eyy[0] == 0.0) ? 1.0 : Eyy[0]);
	for (i = 0, j = (DMIN - 4) / SUBSAMP; i < D_RANGE; i++, j++) {
	    Eyy[j] = ((Eyy[j] == 0.0) ? 1.0 : Eyy[j]);
	    corr[1][i] = SIGN (Exy[i]) * SQR (Exy[i]) / Eyy[0] / Eyy[j];
	}

	// First subfame of look ahead frame
	ptr = lpfr + LPFRMEM_LEN + FSIZE / SUBSAMP;
	Eyy[0] = 0.0;
	for (i = 0; i < FSIZE / 2 / SUBSAMP; i++)
	    Eyy[0] += SQR (ptr[i]);
	for (i = 1; i < LPFRMEM_LEN; i++, ptr--) {
	    Eyy[i] =
		Eyy[i - 1] + SQR (ptr[-1]) -
		SQR (ptr[FSIZE / 2 / SUBSAMP - 1]);
	}
	ptr = lpfr + LPFRMEM_LEN + FSIZE / SUBSAMP;
	Exyptr = Exy;
	for (i = (DMIN - 4) / SUBSAMP; i < (DMAX + 2) / SUBSAMP; i++) {
	    *Exyptr = 0.0;
	    for (j = 0; j < FSIZE / 2 / SUBSAMP; j++)
		*Exyptr += ptr[j] * (*(ptr - i + j));
	    Exyptr++;
	}

	Eyy[0] = ((Eyy[0] == 0.0) ? 1.0 : Eyy[0]);
	for (i = 0, j = (DMIN - 4) / SUBSAMP; i < D_RANGE; i++, j++) {
	    Eyy[j] = ((Eyy[j] == 0.0) ? 1.0 : Eyy[j]);
	    next_corr[0][i] = SIGN (Exy[i]) * SQR (Exy[i]) / Eyy[0] / Eyy[j];
	}
	// Second subfame of look ahead frame
	ptr =
	    lpfr + LPFRMEM_LEN + LOOKAHEAD_LEN / SUBSAMP +
	    FSIZE / 2 / SUBSAMP;
	Eyy[0] = 0.0;
	for (i = 0; i < FSIZE / 2 / SUBSAMP; i++)
	    Eyy[0] += SQR (ptr[i]);
	for (i = 1; i < LPFRMEM_LEN; i++, ptr--) {
	    Eyy[i] =
		Eyy[i - 1] + SQR (ptr[-1]) -
		SQR (ptr[FSIZE / 2 / SUBSAMP - 1]);
	}
	ptr =
	    lpfr + LPFRMEM_LEN + LOOKAHEAD_LEN / SUBSAMP +
	    FSIZE / 2 / SUBSAMP;
	Exyptr = Exy;
	for (i = (DMIN - 4) / SUBSAMP; i < (DMAX + 2) / SUBSAMP; i++) {
	    *Exyptr = 0.0;
	    for (j = 0; j < FSIZE / 2 / SUBSAMP; j++)
		*Exyptr += ptr[j] * (*(ptr - i + j));
	    Exyptr++;
	}

	Eyy[0] = ((Eyy[0] == 0.0) ? 1.0 : Eyy[0]);
	for (i = 0, j = (DMIN - 4) / SUBSAMP; i < D_RANGE; i++, j++) {
	    Eyy[j] = ((Eyy[j] == 0.0) ? 1.0 : Eyy[j]);
	    next_corr[1][i] = SIGN (Exy[i]) * SQR (Exy[i]) / Eyy[0] / Eyy[j];
	}
	delete[]Exy;
	delete[]Eyy;

	for (i = 0; i < D_RANGE; i++) {
	    for (j = 0, max = 0.0; j < FAN[i][1]; j++)
		max = MAX (max, next_corr[1][j + FAN[i][0]]);
	    rightmax[i] = next_corr[0][i] + max;
	}
	for (i = 0; i < D_RANGE; i++) {
	    for (j = 0, max = 0.0; j < FAN[i][1]; j++)
		max = MAX (max, rightmax[j + FAN[i][0]]);
	    maxcorr[SUBSAMP * i] = corr[1][i] + max;
	}
	for (i = 0; i < D_RANGE; i++) {
	    for (j = 0, max = 0.0; j < FAN[i][1]; j++)
		max = MAX (max, corr[0][j + FAN[i][0]]);
	    maxcorr[SUBSAMP * i] += max;
	}
	// Interpolate maxcorr to get values at odd positions, SUBSAMP=2 assumed
	for (i = 1; i < D_RANGE - 2; i++) {
	    maxcorr[2 * i + 1] = 0.0;
	    for (j = 0; j < 4; j++)
		maxcorr[2 * i + 1] += corr_filt[j] * maxcorr[2 * (i - 1 + j)];
	}
	maxcorr[1] = (maxcorr[0] + maxcorr[2]) / 2;
	maxcorr[SUBSAMP * D_RANGE - 3] =
	    (maxcorr[SUBSAMP * D_RANGE - 4] +
	     maxcorr[SUBSAMP * D_RANGE - 2]) / 2;
	maxcorr[SUBSAMP * D_RANGE - 1] = maxcorr[SUBSAMP * D_RANGE - 2];
	// End Interp maxcorr

	for (i = 4, max = 0.0; i < D_RANGE * SUBSAMP - 1; i++)
	    if (max < maxcorr[i]) {
		max = maxcorr[i];
	    }

    }

    for (i = 0; i < LPFRMEM_LEN; i++)
	lpfr[i] = lpfr[i + FSIZE / SUBSAMP];
    return (max / 4.0);
}
