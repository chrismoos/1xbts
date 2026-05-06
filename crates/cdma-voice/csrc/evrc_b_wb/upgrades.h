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

/*****************************************************************
 *   (C) COPYRIGHT 1999,2000 Motorola, Inc.
 *****************************************************************/

#ifndef _UPGRADES_H
#define _UPGRADES_H

#include "defines.h"


#define	TRUE		1
#define	FALSE		0
#define	YES		1
#define	NO		0
#define	ENABLED		1
#define	DISABLED	0


#define FIXED_EIGHTH_RATE_ATTEN_AMT     3.0	/* Specifies ER attenuation level in dB */

void wsnr (float *signal, float *synth, int i, int subframesize);
void print_wsnr (void);


#define NUM_Q_LEVELS_NB    64
#define NUM_EQ_BITS_WB      5	//WB
#define NUM_EQ_BITS     6	//NB

#define BBG_NUM_CHAN 16
#define BBG_DELAY 24



#define LPC_ANALYSIS_WINDOW_LENGTH      320


/* ND LSP VQ */
int quant_lsp (			/* return index for quantized lsp */
		  float lsp_nq[],	/* input unquantized lsp segment */
		  float ref_l,	/* lower end reference */
		  float ref_u,	/* upper end reference */
		  float codebook[],	/* lsp quantization table */
		  int codebook_size,	/* # of entries in codebook */
		  int vec_dimension,	/* size of lsp segment */
		  float w[],	/* weight */
		  float lsp_q[]);	/* output quantized lsp segment */
void dequant_lsp (int index,
		  float ref_l,
		  float ref_u,
		  float codebook[], int vec_dimension, float lsp[]);
void enc_lsp_vq_28 (float lsp_nq[], short codes[], float lsp[]);
void dec_lsp_vq_28 (short codes[], float lsp[]);
void enc_lsp_vq_22 (float lsp_nq[], short codes[], float lsp[]);
void dec_lsp_vq_22 (short codes[], float lsp[]);
void enc_lsp_vq_16 (float lsp_nq[], short codes[], float lsp[]);
void dec_lsp_vq_16 (short codes[], float lsp[]);
void enc_lsp_vq_8 (float lsp_nq[], short codes[], float lsp[]);
void dec_lsp_vq_8 (short codes[], float lsp[]);
/* End ND LSP VQ */


int check_fpmp_code (int *, int);



/* Half rate Variable configuration ACELP codebook */

#define VCM_L0    			55
#define VCM_L    			54
#define VCM_UDU_OVERSTEP		3
#define VCM_UuU_OVERSTEP		2	/* glottal side pulse */
#define VCM_UDU_PIT			66	/* roughly (1 + 1/4) * subframesize */
#define VCM_UD_PIT			95	/* roughly (1 + 3/4) * subframesize */
#define VCM_FAC			0.9 / (120 - VCM_UD_PIT)
#define PIT_THRSH			0.3

int JCELP_Config (int lag,	/* input */
		  float gain, int l_subfr, int pos[512][3],	/* output */
		  int offset[],
		  int *step, int *max, float *sign, float *ratio);

/* END Half rate Variable configuration ACELP codebook */

void hnwFilter (float input[], float state[], float delay3[], short length,
		float &ghnw, float output[]);
void hnwZeroState (float input[], float delay3[], short length, float ghnw,
		   float output[]);
void hnwZeroInput (float input[], float state[], float delay3[], short length,
		   float ghnw, float output[]);
void HNW_interp (float *input, float *output, float locdelay, int length);

#endif
