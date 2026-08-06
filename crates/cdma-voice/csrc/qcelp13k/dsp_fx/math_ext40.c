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
/* LIBRARY: math_ext40.c                                             */
/*-------------------------------------------------------------------*/
/* PURPOSE: Extended mathematical functions 40bits.                  */
/*===================================================================*/

#include "typedef_fx.h"
#include "basic_op40.h"
#include "math_ext32.h"

/*----------------------------------------------------------------------------*/
/*-------------------------------- FUNCTIONS ---------------------------------*/
/*----------------------------------------------------------------------------*/

/*****************************************************************************
 *
 *   Function Name : L_msu40_16_32
 *
 *   Purpose :
 *
 *   Multiply var1 by L_var2 and shift the result left by 1. Add the 40
 *   bit result to acc40 with saturation, return a 40 bit result:
 *        L_msu(acc40,var1,var2) = L_sub40(acc40,(L_mult40(var1,var2)).
 *
 *   Complexity weight : 1
 *
 *   Inputs :
 *
 *    acc40    40 bit long signed integer (Word40) whose value falls in the
 *             range : MIN_40 <= L_var3 <= MAX_40.
 *
 *    var1     16 bit short signed integer (Word16) whose value falls in
 *             the range : MIN_16 <= var1 <= MAX_16.
 *
 *    var2     32 bit long signed integer (Word32) whose value falls in
 *            therange : MIN_32 <= var1 <= MAX_32.
 *
 *   Return Value :
 *
 *    L_var_out
 *             40 bit long signed integer (Word40) whose value falls in the
 *             range : MIN_40 <= L_var_out <= MAX_40.
 *****************************************************************************/

Word40 L_msu40_16_32 (Word40 acc40, Word16 var1, Word32 L_var2)
        {
	  Word32 reg32;

	  reg32 = L_mpy_ls(L_var2, var1);
	  acc40 = L_sub40 (acc40, reg32);

	  return (acc40);
	}

/*---------------------------------------------------------------------------*/

/*****************************************************************************
 *
 *   Function Name : L_mac40_16_32
 *
 *   Purpose :
 *
 *   Multiply var1 by L_var2 and shift the result left by 1. Add the 40
 *   bit result to acc40 with saturation, return a 40 bit result:
 *        L_mac(acc40,var1,var2) = L_add40(L_var3,(L_mult40(var1,var2)).
 *
 *   Complexity weight : 1
 *
 *   Inputs :
 *
 *    acc40    40 bit long signed integer (Word40) whose value falls in the
 *             range : MIN_40 <= L_var3 <= MAX_40.
 *
 *    var1     16 bit short signed integer (Word16) whose value falls in
 *             the range : MIN_16 <= var1 <= MAX_16.
 *
 *    L_var2   32 bit long  signed integer (Word32) whose value falls in
 *             therange : MIN_32 <= var1 <= MAX_32.
 *
 *   Return Value :
 *
 *    L_var_out
 *             40 bit long signed integer (Word40) whose value falls in the
 *             range : MIN_40 <= L_var_out <= MAX_40.
 ******************************************************************************/

Word40 L_mac40_16_32 (Word40 acc40, Word16 var1, Word32 L_var2)
        {
	  Word32 reg32;

	  reg32 = L_mpy_ls(L_var2, var1);
	  acc40 = L_add40 (acc40, reg32);

	  return (acc40);
	}

/*---------------------------------------------------------------------------*/

/*============================================================================*/
/*----------------------------------- END ------------------------------------*/
/*============================================================================*/
