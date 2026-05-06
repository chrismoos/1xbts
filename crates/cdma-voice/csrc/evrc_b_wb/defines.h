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


#ifndef _defines_h
#define _defines_h

#include <stdio.h>


#define MAX_MDCT_NWORDS 9	// Max number of words for storing MDCT indices (28 pulses -> 131 bits)
#define MUSIC_MODE_DAMPEN_HB_ENERGY_FACT (0.95/0.70)

#define DEEMPH_FAC -0.48F

/* RESIDUAL ENERGY GAIN QUANTIZATION  */
#  define   NOOFFCBLEVELFOR_REGQ    16
#  define   NUM_Q_LEVELS_E           8
#  define   MIN_LOG_ENERGY_E       4.0
#  define   MAX_LOG_ENERGY_E      11.5

// put these in mem class
extern float Erq;
extern short idxe;
extern float ckck;

float GAIN_RE_F (float acbGain, int j);

#define LASTRATE_TH 2


/* CLOSED LOOP DELAY ADJUST */

#define A_DEFAULT 0.0020f
#define B_DEFAULT 0.0025f
#define S_DEFAULT 0.20f
#define T_DEFAULT 1.10f
#define ABCG_DFLT 1.25f

extern int da_index[3];		// We know there are only 3 subframes, used to pass signal to cod7_35() & dec_fpmp()



#define PPF_COEF 1.0F
#define RATIO_FACTOR		1.0

#define     NUM_Q_LEVELS   32
#define     NUM_EQ_BITS_WB     5

void enc_lsp_vq_10 (float lsp_nq[], short codes[], float lsp[]);
void dec_lsp_vq_10 (short codes[], float lsp[]);


#define FSIZE 160
#define LPCOFFSET 40
#define DEC_DELAY 40
#define TX_SIZE (2*FSIZE+FSIZE+LPCOFFSET)
#define RX_SIZE (FSIZE+DEC_DELAY)
#define NUMMODES 7
#define ATYPE 0
#define BTYPE 1
#define YES 1
#define NO 0
#define FULL 0
#define THREE_QUARTER 1
#define HALF 2
#define QUARTER 3
#define ZERO 4
#define BLANK 5
#define ERASURE 14
#define CELP 0
#define PPP 1
#define NELP 2
#define ONSET 4
#define PF_ON 1
#define UNLIMITED -1
#define INACTIVE 0
#define ACTIVE 1
#define UNVOICED 2
#define VOICED 3
#define TRANSIENT 4
#define ENCODER 0
#define DECODER 1


#include <math.h>
static double
anint (double x)
{
    return (x) >= 0 ? (int) ((x) + 0.5) : (int) ((x) - 0.5);
}

#ifdef __SUNOS4__
static double
log2 (double x)
{
    return log (x) / log (2.0);
}
#endif

#ifdef WIN32
#define M_PI 3.1415926535898
static double
log2 (double x)
{
    return log (x) / log (2.0);
}
#endif

#define PI M_PI
#define TWO_PI (2*M_PI)

#define MAX(a,b) ((a)>(b)?(a):(b))
#define MIN(a,b) ((a)<(b)?(a):(b))
#define SIGN(a) ((a)>=0?(1):(-1))
#define SQR(a) ((a)*(a))
#define FRAC(a) ((a)-(float)((int)(a)))
#define ABS(a) ((a)>=0?(a):(-(a)))
/* Coder Specifics */
typedef struct
{
    int mode;
    int curr_voiced;
    int speech_type;
    int qspeech_type;
}
MODE;

#define LPCORDER 10
#define BPFORDER 23
#define LPCSF 4
#define IRLEN 20
#define MINLAG 16
#define MAXLAG 120
#define MINLAG_TRK 12		/* 4 lesser than actual min. to enable interp. of corr */
#define MINLAG_PPP 16
#define MAXLAG_PPP MAXLAG
#define MAXLAG_WI 61		/* Need to allocate more space to handle pitch doubling/tripling */
#define SUBSAMP 2
#define DECIM_FILTER_LENGTH 15
#define LAGRANGE (MAXLAG_PPP-MINLAG_TRK+1)
#define FR_INTERP_WIDTH 8
#define PITCH_FILT_ORDER (MAXLAG+FR_INTERP_WIDTH/2)
#define LENGTH_OF_IMPULSE_RESPONSE 40
#define PFZF 0.625
#define PFPF 0.775
#define AGC_FACTOR 0.9375
#define BRIGHT_COEFF 0.3


static int CBSF[NUMMODES] = { 4, 4, 0, 0, 4, 4, 4 };


#define MAXCBSF 8
#define NUMPPPCB 3

#define CELPB_LEVELS 9
#define MAX_CELPB 2.0
#define MIN_CELPB 0.0
#define PPPB_LEVELS 64
#define MAX_PPPB 4.0
#define MIN_PPPB 0.0625


#define CELPGFULLLENGTH 16
#define CELPGACELPFULLLENGTH 32
#define MAXGACELP 11.2636
#define MINGACELP 0
#define CELPGTHREEQLENGTH 256
#define CELPGHALFLENGTH 256
#define NUM_ERB 22
#define NUM_FIXED_BAND 17	/* Num of bands for multiband alignment in FPPP */

#define PTS 500			/* Number of points for integration in SD computation */
#endif
