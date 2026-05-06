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
/*  Module:     macro.h                                                 */
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
//#include <math.h>
#include <stdio.h>
#include <fcntl.h>

/* generic definitions */
#define FALSE 0
#define TRUE 1

/* user function definition */
#include "proto.h"

/* speech coder parameters */
#define FrameSize   160     /* CELP frame size */
#define NoOfSubFrames 3     /* Number of sub frames in one frame     */
#define SubFrameSize  54
#define HPMEMORY      FrameSize+SubFrameSize

/* Memory required for HPspeech array    */

#define ORDER         10    /* LPC order                             */
#define MIN_LSP_SEP   261       /* (64Hz=0.05/(2*PI)*32767 Min. LSP Separation */
#define GAMMA1        29491 /*0.9*/ /* Weighting filter fraction coefficient */
#define GAMMA2        16384 /*0.5*/ /* Weighting filter fraction coefficient (>.5 muffled)*/

#define maxFCBGainSize  32  /* Size of fcb scalar gain (8k,=16 for 4k*/
#define ACBGainSize   8     /* Size of acb scalar gain               */

#define Hlength       SubFrameSize  /* Length of impulse response       */
#define ACBMemSize    128   /* Size of adaptive c.b. memory          */
#define PACKWDSNUM    11    /* Without one word for frame erasure signaling */
#define PACKBYTESNUM  16

#define _Gamma_4      32571  /*0.994*/

/* Post filter definitions */
#define ALPHA         18678  /*0.57*/   /* Short term post filter parameter (whiten formants)*/
#define BETA          24576  /*0.75*/   /* Short term post filter parameter (boost formants)*/
#define U             6554 /*0.20*/ /* Spectral tilt (orig=.2, >.2 clear, <.2 muffled)*/
#define AGC           27853 /*0.85*/    /* AGC factor                       */
#define LTGAIN        16384 /*0.50*/    /* Long term post filter gain       */

/* Post filter defines for half rate */
#define HALF_ALPHA    16384 /*0.50*/    /* Short term post filter parameter (whiten formants)*/
#define HALF_U        11469 /*0.35*/    /* Spectral tilt (orig=.2, >.2 clear, <.2 muffled)*/

/* Rcelp parameters */
#define GUARD          80   /* Guard buffers for RCELP          */
#define RRESOLUTION    8    /* Jitter resolution                */
#define RSHIFT         3    /* Search boundary                  */
#define DMIN          20    /* Minimum delay                    */
#define DMAX         120    /* Maximum delay                    */
#define BLPRECISION    8    /* Interpolation filter taps        */
#define BLFREQ       29491 /*0.9*/  /* Cut-off filter frequency         */
#define EXTRA         10    /* Extra samples calc. in exc.      */

#endif
