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
#include <stdio.h>
#include <stdlib.h>
#include <math.h>
#include <string.h>

#define CRD_OUTPUT_FILE 0
#define CRD_STDOUT 0

#define SIZE_UNDERFLOW_ARRAY			50

#define	SMART_BLANKING_ENABLED			1

#define NON_DROP_OPTION					2
#define TIME_WARP_OPTION				1

#define BEGINNING_ENHANCEMENT_FACTOR	1
//#define END_ENHANCEMENT                                       0

/* This default number may be overwrote by the run-time variable: target_pkt_underflow */
#define TARGET_PKT_UNDERFLOW			0.005

#define	JITTER_LOWER_LIMIT				0.04
#define	JITTER_UPPER_LIMIT				0.1
#define DEFAULT_JITTER					0.06

#define JITTER_INCREMENT				0.02

#define TIME_WARP_SPACING				8
#define TIME_WARP_SPACING_END			1

#define FLAG_DECREASING_BUFFER_DEPTH		1
#define FLAG_BUNDLING_HANDLING			1

/* The phase matching should be enabled in NB mode and disabled in WB mode, by default */
/* So it is acutally controlled by the run-time variable "do_phasematching*/
#define PHASE_MATCHING_ENABLED

#define PHASE_MATCHING				1

#define FLAG_INTRA_TALKSPURT			1

#define MAINTAIN_INTRA_TALKSPURT_SILENCE_EXPAND		0.4
#define MAINTAIN_INTRA_TALKSPURT_SILENCE_COMPRESS	0.2

#define NUM_SILENCE_SENTENCE			10

#define TIME_WARP_SPACING_EXPAND		1

#define NUM_SAMPLES_PER_FRAME_8k		160
#define NUM_SAMPLES_PER_FRAME_16k               320

void Init_Jitter_Buffer ();
void Add_To_Jitter_Buffer (int sn, long ts, double rcvd_time, int rate);
int Remove_Packet_From_Buffer (double, float *, int *, int *, int *);
void Add_Packet_To_Buffer (int sn, long ts, double rcvd_time, int rate);

void Algorithm5 (double);

#ifdef	__cplusplus
extern "C"
{
#endif				/* __cplusplus */

    void InitDecoder ();
    void decode (short *, short, short, float *);
    void InitEncoder ();

#ifdef	__cplusplus
}				// extern "C"
#endif				/* __cplusplus */
