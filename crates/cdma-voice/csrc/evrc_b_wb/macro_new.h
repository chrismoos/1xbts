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
/*  4GV - Fourth Generation Vocoder Speech Service Option for           */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


#include "defines.h"

	/* delay for high band, to align with narrowband excitation (in samples, at 7 kHz sampling rate) */
#define HB_ANA_DELAY_NS_LB			58	// with low-band noise suppression
#define HB_ANA_DELAY				23	// without noise suppression

	/* delay for low band, to align with high band and make up for overlap add delay (in samples, at 8 kHz sampling rate) */
#define LB_SYN_DELAY				28

	/* overlap add length for high band (in samples, at 7 kHz sampling rate) */
#define HB_OVERLAP_ADD				14

	/* filter bank characteristics */
#define ANA_FILT_LEN_LB				14
#define SYN_FILT_LEN_LB				16
#define FRAC_DN_HB					7
#define LEN_DN_HB					10
#define FRAC_UP_HB					4
#define LEN_UP_HB					10
#define ALLPASSSECTIONS				2
#define ALLPASSSECTIONS_STEEP		3

	/* coefficients for low band analysis filter */
extern float coefs_ana_filt_lb[2][ANA_FILT_LEN_LB];

	/* coefficients for low band synthesis filter */
extern float coefs_syn_filt_lb[2][SYN_FILT_LEN_LB];
extern float coefs_syn_filt_lb_a[3];
extern float coefs_syn_filt_lb_b[3];

	/* coefficients for high band analysis filter */
extern float coefs_down_HB[FRAC_DN_HB][LEN_DN_HB];
extern float coefs_ana_filt_hb_a[2];
extern float coefs_ana_filt_hb_b[2];

	/* coefficients for high band synthsis filter */
extern float coefs_up_HB[FRAC_UP_HB][LEN_UP_HB];
extern float coefs_syn_filt_hb_a1[3];
extern float coefs_syn_filt_hb_b1[3];
extern float coefs_syn_filt_hb_a2[3];
extern float coefs_syn_filt_hb_b2[3];


#define LB_FRAMESIZE		160
#define UB_FRAMESIZE		140
#define OVERLAP_7			14

#define FRAC1				7

#define LEN					12
#define LPC_UB_ORDER		6

#define Min(a,b)			(a<b ? a:b)
#define Pi					3.14159265358979

#define FRAME_LENGTH_8R		280
#define OVERLAP_8R			28
#define	BUFFER_LENGTH_8R	308

#define LPC_ORD_WB_8R		10

#define LPF_8R_DR_ORD		2
#define LPF_8R_NR_ORD		2

#define NFACT				39.47841760435743	// 4*pi^2
