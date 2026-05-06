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
/*----------------------------------------------------------------------*/
/*  Memory Usage:                                                       */
/*      ROM:                0                                           */
/*      Static/Global RAM:  0                                           */
/*      Stack/Local RAM:    57                                          */
/*----------------------------------------------------------------------*/

/*++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++
 * 
 * IMPORTANT NOTE ***** IMPORTANT NOTE ***** IMPORTANT NOTE
 * 
 * Because the gcb values in the floating point model can exceed
 * one, the fixed point values of gcb = gcb_float / 2.  The
 * outputs from this module for gcb are 1/2 the corresponding
 * floating point value.
 * 
 * +++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++++ */

#include "macro.h"
#include <stdio.h>
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"

//#include "tty.h"

void getgain(INT16 *exctation, INT16 *lambda, INT16 *H, INT16 *idxcb, INT16 *gcb, INT16 *gcb_mid, INT16 gcb_size, INT16 Quantize, INT16 *mresidual, INT16 subframel, INT16 hlength)
{

	register INT16 i;
	INT32 cross, ener;
	INT16 temp[54];
        INT16 shift;
        INT32  Ltemp;

	ConvolveImpulseR(temp, exctation, H, hlength, subframel);

	cross = ener = 0;
        shift = 1;
	for (i = 0; i < subframel; i++)
        {
            Ltemp = L_mult(temp[i],mresidual[i]);
            Ltemp = L_shr(Ltemp,shift);
            cross = L_add(cross,Ltemp);
            Ltemp = L_mult(temp[i],temp[i]);
            Ltemp = L_shr(Ltemp,shift);
            ener  = L_add(ener,Ltemp);
            if( ener >= 0x40000000L )
            {
                shift++;
                ener = L_shr(ener,1);
                cross = L_shr(cross,1);
            }
	}
#if 0
    if( tty_option == TTY_NO_GAIN && tty_enc_flag != 0 )
    {
        cross = 0.0;
    }
#endif

	if (cross > 0)
	{
		*idxcb = 0;
		for (i = 1; i < gcb_size; i++)
		{
			if (cross > L_shl(L_mpy_ls(ener, gcb_mid[i - 1]), 1))
				*idxcb = i;
		}
		*lambda = gcb[*idxcb];
	}
	else
	{							/* No possitive cross correlation */
		*lambda = gcb[0];		/* Ex1 = 0.0 */
		*idxcb = 0;
	}

	for (i = 0; i < subframel; i++)
		exctation[i] = round32(L_shl(L_mult(*lambda, exctation[i]), 1));
}
