/**********************************************************************
Each of the companies; Qualcomm, and Lucent (hereinafter
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
all Normative portions of the QCELP-13 Variable Rate Speech Codec as
it exists on the date of submittal of this form, should the QCELP-13 be
approved as a Specification or Report by 3GPP2, or as a transposed
Standard by any of the 3GPP2's Organizational Partners, the Source(s)
state(s) that a worldwide license to reproduce, use and distribute the
Software, the license rights to which are held by the Source(s), will
be made available to applicants under terms and conditions that are
reasonable and non-discriminatory, which may include monetary compensation,
and only to the extent necessary for the practice of any or all of the
Normative portions of the QCELP-13 or the field of use of practice of the
QCELP-13 Specification, Report, or Standard.  The statement contained above
is irrevocable and shall be binding upon the Source(s).  In the event
the rights of the Source(s) in and to copyright or copyright license
rights subject to such commitment are assigned or transferred, the
Source(s) shall notify the assignee or transferee of the existence of
such commitments.
*******************************************************************/

/*_________________________________________________________________________
 |                                                                         |
 |                                  Include                                |
 |_________________________________________________________________________|
*/

#include "typedef_fx.h"
#include "basic_op.h"

#ifdef WMOPS_FX
#include "main_fx.h"
#include "const_fx.h"
#include "ext_var_fx.h"
#endif

/*_________________________________________________________________________
 |                                                                         |
 |                                  Define                                 |
 |_________________________________________________________________________|
*/

#define SW_MIN (short)0x8000           /* smallest Ram */
#define SW_MAX (short)0x7fff           /* largest Ram */

/*_________________________________________________________________________
 |                                                                         |
 |                                Functions                                |
 |_________________________________________________________________________|
*/

/****************************************************************************
 *
 *     FUNCTION NAME: L_mpy_ll
 *
 *     PURPOSE:    Multiply a 32 bit number (L_var1) and a 32 bit number
 *                 (L_var2), and return a 32 bit result.
 *
 *     INPUTS:
 *
 *       L_var1             A Word32 input variable
 *
 *       L_var2             A Word32 input variable
 *
 *     OUTPUTS:             none
 *
 *     IMPLEMENTATION:
 *
 *        Performs a 31x31 bit multiply, Complexity=24 Ops.
 *
 *        Let x1x0, or y1y0, be the two constituent halves
 *        of a 32 bit number.  This function performs the
 *        following:
 *
 *        low = ((x0 >> 1)*(y0 >> 1)) >> 16     (low * low)
 *        mid1 = [(x1 * (y0 >> 1)) >> 1 ]       (high * low)
 *        mid2 = [(y1 * (x0 >> 1)) >> 1]        (high * low)
 *        mid =  (mid1 + low + mid2) >> 14      (sum so far)
 *        output = (y1*x1) + mid                (high * high)
 *
 *
 *     RETURN VALUE:        A Word32 value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/

Word32 L_mpy_ll(Word32 L_var1, Word32 L_var2)
        {
	  Word16 swLow1,
	         swLow2,
	         swHigh1,
	         swHigh2;
	  Word32 L_varOut,
	         L_low,
	         L_mid1,
	         L_mid2,
	         L_mid;

#ifdef WMOPS_FX
	  counter_fx.L_mpy_ll++;

	  counter_fx.shr -= 2;
	  counter_fx.extract_l -= 2;
	  counter_fx.extract_h -= 2;
	  counter_fx.L_mult -= 3;
	  counter_fx.L_shr -= 4;
	  counter_fx.L_add -= 2;
	  counter_fx.L_mac--;
#endif

	  swLow1 = shr(extract_l(L_var1), 1);
	  swLow1 = SW_MAX & swLow1;

	  swLow2 = shr(extract_l(L_var2), 1);
	  swLow2 = SW_MAX & swLow2;
	  swHigh1 = extract_h(L_var1);
	  swHigh2 = extract_h(L_var2);

	  L_low = L_mult(swLow1, swLow2);
	  L_low = L_shr(L_low, 16);

	  L_mid1 = L_mult(swHigh1, swLow2);
	  L_mid1 = L_shr(L_mid1, 1);
	  L_mid = L_add(L_mid1, L_low);

	  L_mid2 = L_mult(swHigh2, swLow1);
	  L_mid2 = L_shr(L_mid2, 1);
	  L_mid = L_add(L_mid, L_mid2);

	  L_mid = L_shr(L_mid, 14);
	  L_varOut = L_mac(L_mid, swHigh1, swHigh2);

	  return (L_varOut);
	}

/*----------------------------------------------------------------------------*/

/****************************************************************************
 *
 *     FUNCTION NAME: L_mpy_ls
 *
 *     PURPOSE:    Multiply a 32 bit number (L_var2) and a 16 bit
 *                 number (var1) returning a 32 bit result. L_var2
 *                 is truncated to 31 bits prior to executing the
 *                 multiply.
 *
 *     INPUTS:
 *
 *       L_var2             A Word32 input variable
 *
 *       var1               A Word16 input variable
 *
 *     OUTPUTS:             none
 *
 *     RETURN VALUE:        A Word32 value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/

Word32 L_mpy_ls(Word32 L_var2, Word16 var1)
        {
	  Word32 L_varOut;
	  Word16 swtemp;

#ifdef WMOPS_FX
	  counter_fx.L_mpy_ls++;

	  counter_fx.shr--;
	  counter_fx.extract_l--;
	  counter_fx.L_mult--;
	  counter_fx.L_shr--;
	  counter_fx.L_mac--;
#endif

	  swtemp = shr(extract_l(L_var2), 1);
	  swtemp = (short) 32767 & (short) swtemp;

	  L_varOut = L_mult(var1, swtemp);
	  L_varOut = L_shr(L_varOut, 15);
	  L_varOut = L_mac(L_varOut, var1, extract_h(L_var2));

	  return (L_varOut);

	}
