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


#include  "macro.h"

#define M3_N_LSP_SEG0		3	/* For 22-bit quantizer */
#define M3_N_LSP_SEG1		3
#define M3_N_LSP_SEG2		4

#define   M3_L_LSP_SEG0	128
#define   M3_L_LSP_SEG1	128
#define   M3_L_LSP_SEG2	256

#define M3_S_LSP_SEG0		7
#define M3_S_LSP_SEG1		7
#define M3_S_LSP_SEG2		8

#define   M3_B_LSP_SEG0	0
#define   M3_B_LSP_SEG1	3
#define   M3_B_LSP_SEG2	6

#include  "lsp_cb334_778.h"



#define	PRE_LSP_NUM0	4	/* Total # of joint search: 6 + 5 + 4 + 3= 18 times */
#define	PRE_LSP_NUM2	6



void
enc_lsp_vq_22 (float lsp_nq[], short codes[], float lsp[])
{
    int i, j, k, ii, jj, kk, j0;
    float delta, delta1, delta2, w[ORDER];
    static float alpha = 0.5 / (2 * 3.141592653589);

    float tmp_tbl0[128 * 3], tmp_tbl2[256 * 4];
    int lsp_index0[PRE_LSP_NUM0], lsp_index2[PRE_LSP_NUM2], i0, i1, i2;
    float s_min, s0, s2, s, t;

    for (i = 0; i < 128 * 3; i++)
	tmp_tbl0[i] = lsp_tbl22_0[i];
    for (i = 0; i < 256 * 4; i++)
	tmp_tbl2[i] = lsp_tbl22_2[i];

    /* Generate LSP(i) weights */
    for (i = 0; i < ORDER; i++) {
	delta1 = (i == 0) ? lsp_nq[i] : lsp_nq[i] - lsp_nq[i - 1];
	delta2 =
	    (i == ORDER - 1) ? 0.5 - lsp_nq[i] : lsp_nq[i + 1] - lsp_nq[i];
	delta = sqrt (delta1 * delta2);
	w[i] = alpha / delta;
    }

    /* Quantization. First and last segs absolute. The rest delta */
    for (i = 0; i < PRE_LSP_NUM0; i++) {
	codes[0] = quant_lsp (lsp_nq, 0.0, 0.5, tmp_tbl0,
			      M3_L_LSP_SEG0, M3_N_LSP_SEG0, w, lsp);
	lsp_index0[i] = codes[0];
	tmp_tbl0[M3_N_LSP_SEG0 * codes[0]] = 1e38;
    }

    for (i = 0; i < PRE_LSP_NUM2; i++) {
	codes[2] = quant_lsp (lsp_nq + M3_B_LSP_SEG2, 0.0, 0.5, tmp_tbl2,
			      M3_L_LSP_SEG2, M3_N_LSP_SEG2, w + M3_B_LSP_SEG2,
			      lsp + M3_B_LSP_SEG2);
	lsp_index2[i] = codes[2];
	tmp_tbl2[M3_N_LSP_SEG2 * codes[2]] = 1e38;
    }

    s_min = 1e38;
    for (i = 0; i < PRE_LSP_NUM0; i++) {

	i0 = lsp_index0[i];
	dequant_lsp (i0, 0.0, 0.5, lsp_tbl22_0, M3_N_LSP_SEG0, tmp_tbl0);

	for (k = 0, s0 = 0.0; k < 3; k++) {
	    s = tmp_tbl0[k] - lsp_nq[k];
	    s0 += w[k] * s * s;
	}

	for (j = 0; j < PRE_LSP_NUM2 - i; j++) {

	    i2 = lsp_index2[j];
	    dequant_lsp (i2, 0.0, 0.5, lsp_tbl22_2, M3_N_LSP_SEG2,
			 tmp_tbl0 + M3_B_LSP_SEG2);
	    for (k = 6, s2 = s0; k < 10; k++) {
		s = tmp_tbl0[k] - lsp_nq[k];
		s2 += w[k] * s * s;
	    }

	    i1 = quant_lsp (lsp_nq + M3_B_LSP_SEG1,
			    tmp_tbl0[M3_B_LSP_SEG1 - 1],
			    tmp_tbl0[M3_B_LSP_SEG2], lsp_tbl22_1,
			    M3_L_LSP_SEG1, M3_N_LSP_SEG1, w + M3_B_LSP_SEG1,
			    tmp_tbl0 + M3_B_LSP_SEG1);
	    for (k = 3; k < 6; k++) {
		s = tmp_tbl0[k] - lsp_nq[k];
		s2 += w[k] * s * s;
	    }

	    if (s2 < s_min) {

		//    if(s_min < 1e37) printf("\n%f to %f", s_min, s2);

		codes[0] = i0;
		codes[1] = i1;
		codes[2] = i2;
		for (k = 0; k < 10; k++)
		    lsp[k] = tmp_tbl0[k];
		s_min = s2;
	    }
	}
    }


}

void
dec_lsp_vq_22 (short codes[], float lsp[])
{
    int i;

    /* Decode the LSPs codevector codes[] */
    dequant_lsp (codes[0], 0.0, 0.5, lsp_tbl22_0, M3_N_LSP_SEG0, lsp);

    dequant_lsp (codes[2], 0.0, 0.5, lsp_tbl22_2, M3_N_LSP_SEG2,
		 lsp + M3_B_LSP_SEG2);

    dequant_lsp (codes[1], lsp[M3_B_LSP_SEG1 - 1], lsp[M3_B_LSP_SEG2],
		 lsp_tbl22_1, M3_N_LSP_SEG1, lsp + M3_B_LSP_SEG1);

}
