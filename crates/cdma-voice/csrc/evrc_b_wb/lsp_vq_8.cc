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

#include  "macro.h"
#include "lsp_cb10_8.h"

void
enc_lsp_vq_8 (float lsp_nq[], short codes[], float lsp[])
{
    int i, j, k, ii, jj, kk, j0;
    float delta, delta1, delta2, w[ORDER];
    static float alpha = 0.5 / (2 * 3.141592653589);

    /* Generate LSP(i) weights */
    for (i = 0; i < ORDER; i++) {
	delta1 = (i == 0) ? lsp_nq[i] : lsp_nq[i] - lsp_nq[i - 1];
	delta2 =
	    (i ==
	     ORDER - 1) ? 0.5 - lsp_nq[i - 1] : lsp_nq[i + 1] - lsp_nq[i];
	delta = sqrt (delta1 * delta2);
	w[i] = alpha / delta;
    }

    codes[0] = quant_lsp (lsp_nq, 0.0, 0.5, lsp_tbl8, 256, 10, w, lsp);
    codes[1] = codes[0] % 16;
    codes[0] = codes[0] / 16;
}

void
dec_lsp_vq_8 (short codes[], float lsp[])
{
    int i;

    codes[0] = codes[0] * 16 + codes[1];

    /* Decode the LSPs codevector codes[] */
    dequant_lsp (codes[0], 0.0, 0.5, lsp_tbl8, 10, lsp);
}
