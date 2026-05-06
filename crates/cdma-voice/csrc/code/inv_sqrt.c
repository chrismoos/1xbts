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
/* 
 |                                                                           |
 |   Function Name : Inv_sqrt                                                |
 |                                                                           |
 |       Compute 1/sqrt(L_x).                                                |
 |       L_x is positive.                                                    |
 |                                                                           |
 |       if L_x is negative or zero, result is 1 (3fff ffff).                |
 |---------------------------------------------------------------------------|
 |  Algorithm:                                                               |
 |                                                                           |
 |   The function 1/sqrt(L_x) is approximated by a table and linear          |
 |   interpolation.                                                          |
 |                                                                           |
 |   1- Normalization of L_x.                                                |
 |   2- If (30-exponant) is even then shift right once.                      |
 |   3- exponant = (30-exponant)/2  +1                                       |
 |   4- i = bit25-b31 of L_x,    16 <= i <= 63  ->because of normalization.  |
 |   5- a = bit10-b24                                                        |
 |   6- i -=16                                                               |
 |   7- L_y = table[i]<<16 - (table[i] - table[i+1]) * a * 2                 |
 |   8- L_y >>= exponant                                                     |
 |___________________________________________________________________________|
*/

#include "typedefs.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathadv.h"

#include "inv_sqrt.dat"         /* Table for inv_sqrt() */

Longword Inv_sqrt(                /* (o) Q30 : output value   (range: 0<=val<1)           */
                   Longword L_x   /* (i) Q0  : input value    (range: 0<=val<=7fffffff)   */
)
{
    Shortword exp, i, a, tmp;
    Longword L_y;

    if (L_x <= (Longword) 0)
        return ((Longword) 0x3fffffff);

	exp = norm_l(L_x);
	L_x = L_shl(L_x, exp);		/* L_x is normalize */

	exp = sub(30, exp);

	if ((exp & 1) == 0)			/* If exponant even -> shift right */
		L_x = L_shr(L_x, 1);

	exp = shr(exp, 1);
	exp = add(exp, 1);

	L_x = L_shr(L_x, 9);
	i = extract_h(L_x);			/* Extract b25-b31 */
	L_x = L_shr(L_x, 1);
	a = extract_l(L_x);			/* Extract b10-b24 */
    a = a & (Shortword) 0x7fff;

	i = sub(i, 16);

	L_y = L_deposit_h(table[i]);	/* table[i] << 16          */
	tmp = sub(table[i], table[i + 1]);	/* table[i] - table[i+1])  */
	L_y = L_msu(L_y, tmp, a);	/* L_y -=  tmp*a*2         */

	L_y = L_shr(L_y, exp);		/* denormalization */

	return (L_y);
}
