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

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     macro.h                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     11/01/94  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

#ifndef  _MACRO_H_
#define  _MACRO_H_

/* #define UNIX */

/* Macros */
#define  Min(a,b) (a<b ? a:b)
#define  Max(a,b) (a>b ? a:b)
#define  Sign(Z) ((Z) < 0 ? -1l : 1l)
#define  UNIX_DEBUG(x)
#define  SPACING   9

/* includes */
#include <stdlib.h>
#include <math.h>
#include <stdio.h>
#include <fcntl.h>
#include <assert.h>

#include <iostream>
using namespace std;

#ifdef WIN32
#define rint(x)  (double)(__int64)((x) + 0.5)	/*...define non-ANSI function... */
#endif

#include "defines.h"
#include "upgrades.h"		/* Motorola's FOURGV enhancements */

/* generic definitions */
#define pi 3.14159265		/* the number pi */
#define pi2 6.2831853		/* 2*pi */
#define FALSE 0
#define TRUE 1

/* precision definition */
typedef double intAcc;
typedef float Float;

#define AccOverflow 3.4359738e10


/* speech coder parameters */
#define  LOOKAHEAD_LEN            80
#define  SPEECH_BUFFER_LEN        160

#define  BITSTREAM_BUFFER_LEN      12

#define FrameSize   160		/* CELP frame size */
#define NoOfSubFrames 3		/* Number of sub frames in one frame     */
#define SubFrameSize  54
#define HPMEMORY      FrameSize+SubFrameSize

/* Memory required for HPspeech array    */

#define ORDER	      10	/* LPC order                             */

#define GAMMA1        0.97F	/* Weighting filter fraction coefficient */	///// ------- with HNW
#define GAMMA1_NB        0.9	/* Weighting filter fraction coefficient */
#define GAMMA2        0.5	/* Weighting filter fraction coefficient (>.5 muffled) */

#define maxFCBGainSize  32	/* Size of fcb scalar gain (8k,=16 for 4k */
#define ACBGainSize   8		/* Size of acb scalar gain               */

#define Hlength       SubFrameSize	/* Length of impulse response       */
#define ACBMemSize    128	/* Size of adaptive c.b. memory          */

#define PACKWDSNUM (BITSTREAM_BUFFER_LEN-1)	/* Without one word for frame erasure signaling */

#define PACKBYTESNUM  16

#define FM_COEF 0.4


#define _Gamma_4      0.994

/* Post filter definitions */
#define ALPHA         0.57	/* Short term post filter parameter (whiten formants) */
#define BETA          0.75	/* Short term post filter parameter (boost formants) */
#define U             0.20	/* Spectral tilt (orig=.2, >.2 clear, <.2 muffled) */
#define AGC           0.85	/* AGC factor                       */
#define LTGAIN        0.50	/* Long term post filter gain       */

/* Post filter defines for half rate */
#define HALF_ALPHA    0.50	/* Short term post filter parameter (whiten formants) */
#define HALF_U        0.35	/* Spectral tilt (orig=.2, >.2 clear, <.2 muffled) */

/* Rcelp parameters */
#define GUARD          80	/* Guard buffers for RCELP          */
#define RRESOLUTION    8	/* Jitter resolution                */


#define RSHIFT         3	/* Search boundary                  */

#define DMIN          20	/* Minimum delay                    */
#define DMAX         120	/* Maximum delay                    */
#define BLPRECISION    8	/* Interpolation filter taps        */

#define BLFREQ      0.95	/* Cut-off filter frequency         */
#define BLFREQ_NB   0.9

#define EXTRA         10	/* Extra samples calc. in exc.      */


/* some nacf related macros */
#define LPFRMEM_LEN (DMAX+2)/SUBSAMP
#define D_RANGE ((DMAX+2)-(DMIN-4))/SUBSAMP
#define RLPF_ORDER 12
#define RHPF_ORDER 3

//need the following two for struct.h
//note needs to be in sync with whats in NS files

#define			FRM_LEN			160
#define			DELAY			160
#define			FFT_LEN			512
#define			NUM_CHAN		61

//the following was in rda.h also, but need to put in macro for struct also
#define FILTERORDER           17

#define FREQBANDS              3

#define FREQBANDS_NB           2

#define FULLRATE_VOICED        4
#define HALFRATE_VOICED        3
#define EIGHTH                 1

#define PITCH_NUM 2
#define INC_FACTOR 1.03		/* Changed from 1.0625 on 20-Jan-95 */
#define SNR_MAP_THRESHOLD 3
#define IS96_INC  1.00547	/* changed from 1.0065 on 20-Jan-95 */
#define VOICE_INITIAL 65+12	/* for EVRC increasing dynamic range */
#define VOICE_INITIAL_HI 55+12	/* for EVRC increasing dynamic range */

#define STATVOICED 5
#define SCALE_DOWN_ENERGY 0.97	/* changed from 0.96 on 20-Jan-95 */

#define TLEVELS 8

#define SMSNR  0.6		/* leaky integration constant used smooth snr estimate */
		    /* changed on 8-Dec-94 per Sharath's recommendation */
#define ADP 8
#define NACF_ADAP_BGN_THR  0.3	/* threshold signifying frame does           */
			       /* not have any voiced speech in it           */
			       /* so we might start to adapt thresholds      */
#define NACF_SOLID_VOICED  0.5	/* threshold above which we are pretty sure  */
			       /* speech is present and thus SNRs can be     */
			       /* adjusted accordingly                       */

#define HIGH_THRESH_LIM  5059644*16

#define FULL_THRESH 2		// 1 changed 2/17/00 JPA   /* the number of full rates in a row before */
#define LSP_SPREAD_FACTOR_FX 0.01

#define AUTO_PI2  6.283185307
#define F0 20


#define LSP_SPREAD_FACTOR_FX_NB 0.008
#endif
