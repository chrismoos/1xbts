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

#include "typedefs.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathadv.h"

#define NB_PULSE  8
#define STEP      5
#define NB_TRACK  5

#define Q15_1_11  2979

/*-------------------------------------------------------------------*
 * Function  dec_8i55_35bits()                                       *
 * ~~~~~~~~~~~~~~~~~~~~~~~~~~~                                       *
 * Algebraic codebook decoder (35 bits): 8 pulses/55 samples.        *
 *-------------------------------------------------------------------*
 * See c8_35pf.c    for more details of the algebraic code           *
 *-------------------------------------------------------------------*/

void dec8_35(
						Shortword * indx,	/* (i)    : indx of 8 pulses (8+8+8+11 = 35 bits)    */
						Shortword cod[],	/* (o) Q12: algebraic (fixed) codebook excitation    */
						Shortword l_subfr	/* (i)    : lenght of subframe (53 or 54)            */
)
{
	Shortword i, k, pos, pos1, pos2, offset, track, sign;

	for (i = 0; i < l_subfr; i++)
		cod[i] = 0;

	offset = shr(indx[3], 9) & 3;

	for (k = 0; k < NB_TRACK - 2; k++)
	{
		pos = (indx[k] & 127);
		if (pos < 121)
		{
			track = add(k, offset);
			if (track >= NB_TRACK)
				track = sub(track, NB_TRACK);

			i = mult(pos, Q15_1_11);
			/* pos1 = (pos/11)*5 + track */
			pos1 = add(shr(extract_l(L_mult(i, 5)), 1), track);

			i = sub(pos, shr(extract_l(L_mult(i, 11)), 1));
			/* pos2 = (pos%11)*5 + track */
			pos2 = add(shr(extract_l(L_mult(i, 5)), 1), track);

			if ((indx[k] & 128) != 0)
				sign = -1;
			else
				sign = 1;

			if (pos1 < l_subfr)
				cod[pos1] = sign;
			if (pos2 < l_subfr)
			{
				if (pos2 < pos1)
					cod[pos2] = negate(sign);
				else
					cod[pos2] = add(cod[pos2], sign);
			}
		}
	}

	pos = (indx[3] & 127);
	if (pos < 121)
	{
		track = add(3, offset);
		if (track >= NB_TRACK)
			track = sub(track, NB_TRACK);

		i = mult(pos, Q15_1_11);
		/* pos1 = (pos/11)*5 + track */
		pos1 = add(shr(extract_l(L_mult(i, 5)), 1), track);

		track = add(4, offset);
		if (track >= NB_TRACK)
			track = sub(track, NB_TRACK);

		i = sub(pos, shr(extract_l(L_mult(i, 11)), 1));
		/* pos2 = (pos%11)*5 + track */
		pos2 = add(shr(extract_l(L_mult(i, 5)), 1), track);

		if ((indx[3] & 256) != 0)
			sign = -1;
		else
			sign = 1;
		if (pos1 < l_subfr)
			cod[pos1] = sign;

		if ((indx[3] & 128) != 0)
			sign = -1;
		else
			sign = 1;
		if (pos2 < l_subfr)
			cod[pos2] = sign;
	}

	return;
}
