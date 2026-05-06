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
/* rate_integrate.h - basic structures for celp coder                 */
/**********************************************************************/
#ifndef _RDA_H_
#define _RDA_H_

#include <stdio.h>

#include "qcoder.h"

typedef struct
{
    INT32 band_noise_sm[FREQBANDS];  /* G. Foerster: Band Noise in Q0 - INT32 */
    INT16 last_rate;            /* E. Chen: int; rate decisision after 2nd stage select_mode2() */
    INT16 last_rate_1st_stage;  /* E. Chen: int; rate decision after 1st stage */
    INT16 last_rate_2nd_stage;  /* E. Chen: int; rate decision after 2nd stage */
    INT16 num_full_frames;      /* E. Chen: int */
    INT16 hangover, hangover_in_progress;   /* E. Chen: int */
    INT16 band_rate[FREQBANDS]; /* E. Chen: int */
    INT32 band_power[4];         /* E. Chen: float (5.000000 to 1675191040.000000); change 4 to FREQBANDS? */
    INT32 signal_energy[FREQBANDS];  /* G. Foerster: Signal Energy in Q0 - INT32 */
    /* E. Chen: float (299010.312500 to 1244383488.00000), (640864.250000 to 1516120832.000000) / 65536 */
    INT32 frame_energy_sm[FREQBANDS];    /* E. Chen: float (3.406658 to 1244383488.000000) */
    INT16 adaptcount;           /* E. Chen: int */
    INT16 pitchrun;             /* E. Chen: int */
    INT16 band_power_last[FREQBANDS];   /* E. Chen: float (0.000000 to 0.000000); may delete because not used? */
    INT16 snr_stat_once;        /* E. Chen: int */
    INT16 snr_map[FREQBANDS];   /* E. Chen: int */
    INT32 snr[FREQBANDS];        /* G. Foerster: SNR in Q8 - INT32 */
    /* E. Chen: float (2241.314941 to 6294507.000000) / 512 */
    INT16 r_filt[FREQBANDS][FILTERORDER];   /* E. Chen: float (0.000151 to 0.475050, -0.001076 to -0.318032) */
    INT16 frame_num;            /* E. Chen: int */
}
ENCODER_MEM;

extern INT16 select_rate(
                            INT32 *R_interp,     /* E. Chen: float* */
                            INT16 max_rate,
                            INT16 min_rate,
                            INT16 beta  /* E. Chen: float */
);

#endif
