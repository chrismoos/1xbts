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
/*===================================================================*/
/* LIBRARY: math_adv40.c                                             */
/*-------------------------------------------------------------------*/
/* PURPOSE: Advanced mathematical functions 40bits.                  */
/*===================================================================*/

#include "typedef_fx.h"
#include "basic_op.h"
#include "basic_op40.h"
#include "math_ext32.h"
#include "math_adv.h"
#include "math_adv40.h"

#ifdef WMOPS_FX
#include "typedef_fx.h"
#include "main_fx.h"
#include "const_fx.h"
#include "lib_wmp_fx.h"

#endif

/*----------------------------------------------------------------------------*/
/*-------------------------------- FUNCTIONS ---------------------------------*/
/*----------------------------------------------------------------------------*/


/***************************************************************************
 *
 *   FUNCTION NAME: fn1_sqroot
 *
 *   PURPOSE:
 *     The purpose of this function is to implement an approximation
 *     of 1/sqroot(x) function
 *
 *
 *   INPUTS:
 *
 *     Input
 *                     normalized input (input range constrained to be < 1.0)
 *                     (Q15)
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     Output
 *                     = 1/sqroot( L_Input) (Q14)
 *
 *   DESCRIPTION:
 *
 *     The following recursive algorithm is used for the approximation
 *
 *     x = 1.0;
 *     for (i = 0; i < M; i++)
 *	  x = x*(1.5 - v*x*x/2);
 *
 *     where v i the normalized input value
 *
 *************************************************************************/

Word16  fn1_sqroot (Word16 Input)
{

/*_________________________________________________________________________
 |                                                                         |
 |                           Local Static Variables                        |
 |_________________________________________________________________________|
*/

/*_________________________________________________________________________
 |                                                                         |
 |                            Automatic Variables                          |
 |_________________________________________________________________________|
*/
        Word16 i;
	Word16 Output;
	Word16 reg16_1, reg16_2;
	Word32 reg32;
	Word40 acc40;
/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	reg16_1    = 0x4000;                     //(1.0 in Q14)
	for (i = 0; i < 5; i++)
	  {
#ifdef WMOPS_FX
	    move16();
#endif
	    reg16_2 = reg16_1;
	    reg32   = L_mult(reg16_1, reg16_1);  // x^2 Q29 32bits
	    reg32   = L_shl (reg32, 1);          // x^2 Q30 32bits
	    reg16_1 = round (reg32);             // x^2 Q14 16bits

	    acc40   = L_mac40(0, reg16_1, Input); // v/2*x^2 Q31 40 bits
	    acc40   = L_shr40(acc40, 1);          // v/2*x^2 Q30 40 bits
	    reg32   = (Word32)acc40;              // no need saturation
	    reg16_1 = round (reg32);              // y = v/2*x^2 Q14 16bits

	    reg16_1 = sub(0x6000, reg16_1);       // z = 1.5 -y Q14 16 bits

	    reg32   = L_mult(reg16_2, reg16_1);   // x*z Q29 32bits
	    reg32   = L_shl (reg32, 1);           // x*z Q30 32bits
	    reg16_1 = round (reg32);              // x*z Q14 16bits
	  }
	Output = reg16_1;

	/* return result */
	/* ------------- */

	return (Output);
}


/***************************************************************************
 *
 *   FUNCTION NAME: sqrt_ratio40
 *
 *   PURPOSE:
 *
 *     The purpose of this function is to perform a single precision square
 *     root function on a ratio of Word40
 *
 *   INPUTS:
 *
 *     L_num
 *                     input numerator
 *     L_den
 *                     input denominator
 *     shift_result
 *                     input additional shift for the result
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     reg16_2
 *                     output to square root function 16 bits
 *
 *   DESCRIPTION:
 *
 *      Input assumed to be NOT normalized
 *
 *
 *************************************************************************/

Word16 sqrt_ratio40 (Word40 L_num40, Word40 L_den40, Word16 shift)

{
  /*-------------------------------------------------------------------------*/

  Word16 exp_num, exp_den, exp_ratio;
  Word32 num, den;

  Word16 reg16, reg16_2;
  Word32 reg32;

  /*-------------------------------------------------------------------------*/
  /*                              Normalize the Energies                     */
  /*-------------------------------------------------------------------------*/

  exp_num = norm32_l40(L_num40);
  num = (Word32) L_shl40(L_num40, exp_num);

  exp_den = norm32_l40(L_den40);
  den = (Word32) L_shl40(L_den40, exp_den);

  /*-------------------------------------------------------------------------*/
  /*                          Calculate the ratio num / den                  */
  /*-------------------------------------------------------------------------*/

  if (L_sub(num, den) > 0L)
    {
      num = L_shr(num, 1);
      exp_num = sub(exp_num, 1);
    }

  reg32 = L_divide(num, den);     //Q31

  /*-------------------------------------------------------------------------*/
  /*              Normalize num / den to calculate sqroot(num / den)         */
  /*-------------------------------------------------------------------------*/

  exp_ratio = norm_l(reg32);
  reg32 = L_shl (reg32, exp_ratio);

#ifdef WMOPS_FX
  test();
#endif

  if (reg32 == 0)
    reg16_2 = 0;
  else
    reg16_2 = sqroot (reg32);

  /*-------------------------------------------------------------------------*/
  /*                    Calculate the sqroot for the exponent                */
  /*-------------------------------------------------------------------------*/
  /*-------------------------------------------------------------------------*/
  /*                     Test if the exponent is even or odd                 */
  /*-------------------------------------------------------------------------*/

#ifdef WMOPS_FX
  test();
  logic16();
#endif
  reg16 = add(exp_ratio, exp_num);
  reg16 = sub(reg16, exp_den);

  if ((reg16 & 0x0001) != 0)
    reg16_2 = mult_r(reg16_2, 0x5A82);     // 1/sqrt(2) in Q15

  /*-------------------------------------------------------------------------*/
  /*                      Calculate the sqroot for the exponent              */
  /*-------------------------------------------------------------------------*/

  reg16 = shr(reg16, 1);

  /*-------------------------------------------------------------------------*/
  /*                          reg16 = sqroot( L_num/L_den)                   */
  /*-------------------------------------------------------------------------*/

  reg16   = add(reg16, shift);
  reg16_2 = shr(reg16_2, reg16);    // Q(15 - shift)

  /*-------------------------------------------------------------------------*/

  return (reg16_2);

  /*-------------------------------------------------------------------------*/
}

/*---------------------------------------------------------------------------*/

/*============================================================================*/
/*----------------------------------- END ------------------------------------*/
/*============================================================================*/
