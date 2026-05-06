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
#include <stdio.h>
#include "dsp_math.h"

/*****************************************************************************
 *                                                                           *
 *   Function Name : L_divide                                                *
 *                                                                           *
 *   Purpose :                                                               *
 *             Fractionnal integer division of two 32 bit numbers.           *
 *             L_num / L_denom.                                              *
 *             L_num and L_denom must be positive and L_num < L_denom.       *
 *                                                                           *
 *   Inputs :                                                                *
 *                                                                           *
 *    L_num                                                                  *
 *             32 bit long signed integer whose value falls in the           *
 *             range : 0x0000 0000 < L_num < L_denom                         *
 *                                                                           *
 *    L_denom                                                                *
 *             32 bit positive normalized integer whose value falls in the   *
 *             range : 0x40000000 < L_denom < 0x7fffffff                     *
 *                                                                           *
 *   Return Value :                                                          *
 *                                                                           *
 *    L_div                                                                  *
 *             32 bit long signed integer whose value falls in the           *
 *             range : 0x0000 0000 <= L_div <= 0x7fff ffff.                  *
 *                                                                           *
 *  Algorithm:                                                               *
 *                                                                           *
 *  - find = 1/L_denom.                                                      *
 *      First approximation: approx = 1 / extract_h(L_denom)                 *
 *      1/L_denom = approx * (2.0 - L_denom * approx )                       *
 *                                                                           *
 *  - result = L_num * (1/L_denom)                                           *
 *****************************************************************************
*/

Longword L_divide(Longword L_num, Longword L_denom)
{
	Shortword approx;
	Longword L_div;

	if (L_num < 0 || L_denom < 0 || L_num > L_denom)
	{
		printf("ERROR: Invalid input into L_divide!\n");
		return (0);
	}

	/* First approximation: 1 / L_denom = 1/extract_h(L_denom) */

	approx = divide_s((Shortword) 0x3fff, extract_h(L_denom));

	/* 1/L_denom = approx * (2.0 - L_denom * approx) */

	L_div = L_mpy_ls(L_denom, approx);

	L_div = L_sub((Longword) 0x7fffffffL, L_div);

	L_div = L_mpy_ls(L_div, approx);

	/* L_num * (1/L_denom) */

	L_div = L_mpy_ll(L_num, L_div);
	L_div = L_shl(L_div, 2);

	return (L_div);
}

/***************************************************************************
 *
 *   FUNCTION NAME: sqroot
 *
 *   PURPOSE:
 *
 *     The purpose of this function is to perform a single precision square
 *     root function on a Longword
 *
 *   INPUTS:
 *
 *     L_SqrtIn
 *                     input to square root function
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     swSqrtOut
 *                     output to square root function
 *
 *   DESCRIPTION:
 *
 *      Input assumed to be normalized
 *
 *      The algorithm is based around a six term Taylor expansion :
 *
 *        y^0.5 = (1+x)^0.5
 *             ~= 1 + (x/2) - 0.5*((x/2)^2) + 0.5*((x/2)^3)
 *                - 0.625*((x/2)^4) + 0.875*((x/2)^5)
 *
 *      Max error less than 0.08 % for normalized input ( 0.5 <= x < 1 )
 *
 *************************************************************************/

Shortword sqroot(Longword L_SqrtIn)
{

/*_________________________________________________________________________
 |                                                                         |
 |                              Local Constants                            |
 |_________________________________________________________________________|
*/

#define    PLUS_HALF          0x40000000L	/* 0.5 */
#define    MINUS_ONE          0x80000000L	/* -1 */
#define    TERM5_MULTIPLER    0x5000	/* 0.625 */
#define    TERM6_MULTIPLER    0x7000	/* 0.875 */

/*_________________________________________________________________________
 |                                                                         |
 |                            Automatic Variables                          |
 |_________________________________________________________________________|
*/

	Longword L_Temp0, L_Temp1;

	Shortword swTemp, swTemp2, swTemp3, swTemp4, swSqrtOut;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	/* determine 2nd term x/2 = (y-1)/2 */
	/* -------------------------------- */

	L_Temp1 = L_shr(L_SqrtIn, 1);	/* L_Temp1 = y/2 */
	L_Temp1 = L_sub(L_Temp1, PLUS_HALF);	/* L_Temp1 = (y-1)/2 */
	swTemp = extract_h(L_Temp1);	/* swTemp = x/2 */

	/* add contribution of 2nd term */
	/* ---------------------------- */

	L_Temp1 = L_sub(L_Temp1, MINUS_ONE);	/* L_Temp1 = 1 + x/2 */

	/* determine 3rd term */
	/* ------------------ */

	L_Temp0 = L_msu(0L, swTemp, swTemp);	/* L_Temp0 = -(x/2)^2 */
	swTemp2 = extract_h(L_Temp0);	/* swTemp2 = -(x/2)^2 */
	L_Temp0 = L_shr(L_Temp0, 1);	/* L_Temp0 = -0.5*(x/2)^2 */

	/* add contribution of 3rd term */
	/* ---------------------------- */

	L_Temp0 = L_add(L_Temp1, L_Temp0);	/* L_Temp0 = 1 + x/2 - 0.5*(x/2)^2 */

	/* determine 4rd term */
	/* ------------------ */

	L_Temp1 = L_msu(0L, swTemp, swTemp2);	/* L_Temp1 = (x/2)^3 */
	swTemp3 = extract_h(L_Temp1);	/* swTemp3 = (x/2)^3 */
	L_Temp1 = L_shr(L_Temp1, 1);	/* L_Temp1 = 0.5*(x/2)^3 */

	/* add contribution of 4rd term */
	/* ---------------------------- */

	/* L_Temp1 = 1 + x/2 - 0.5*(x/2)^2 + 0.5*(x/2)^3 */

	L_Temp1 = L_add(L_Temp0, L_Temp1);

	/* determine partial 5th term */
	/* -------------------------- */

	L_Temp0 = L_mult(swTemp, swTemp3);	/* L_Temp0 = (x/2)^4 */
	swTemp4 = round32(L_Temp0);	/* swTemp4 = (x/2)^4 */

	/* determine partial 6th term */
	/* -------------------------- */

	L_Temp0 = L_msu(0L, swTemp2, swTemp3);	/* L_Temp0 = (x/2)^5 */
	swTemp2 = round32(L_Temp0);	/* swTemp2 = (x/2)^5 */

	/* determine 5th term and add its contribution */
	/* ------------------------------------------- */

	/* L_Temp0 = -0.625*(x/2)^4 */

	L_Temp0 = L_msu(0L, TERM5_MULTIPLER, swTemp4);

	/* L_Temp1 = 1 + x/2 - 0.5*(x/2)^2 + 0.5*(x/2)^3 - 0.625*(x/2)^4 */

	L_Temp1 = L_add(L_Temp0, L_Temp1);

	/* determine 6th term and add its contribution */
	/* ------------------------------------------- */

	/* swSqrtOut = 1 + x/2 - 0.5*(x/2)^2 + 0.5*(x/2)^3 */
	/* - 0.625*(x/2)^4 + 0.875*(x/2)^5     */

	swSqrtOut = mac_r(L_Temp1, TERM6_MULTIPLER, swTemp2);

	/* return output */
	/* ------------- */

	return (swSqrtOut);
}

/***************************************************************************
 *
 *   FUNCTION NAME: fnLog2
 *
 *   PURPOSE:
 *     The purpose of this function is to take the log base 2 of input and
 *     divide by 32 and return; i.e. output = log2(input)/32
 *
 *   INPUTS:
 *
 *     L_Input
 *                     input
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     Longword
 *                     output
 *
 *   DESCRIPTION:
 *
 *     log2(x) = 4.0 * (-.3372223*x*x + .9981958*x -.6626105)
 *                           c0            c1          c2   (includes sign)
 *
 *************************************************************************/

Longword fnLog2(Longword L_Input)
{

	static Shortword
	    swC0 = -0x2b2a, swC1 = 0x7fc5, swC2 = -0x54d0;

	Shortword siShiftCnt, swInSqrd, swIn;
	Longword LwIn, Ltemp, lwInSqrd;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	/* normalize input and store shifts required */
	/* ----------------------------------------- */

	siShiftCnt = norm_l(L_Input);
	LwIn = L_shl(L_Input, siShiftCnt);
	siShiftCnt = add(siShiftCnt, 1);
	siShiftCnt = negate(siShiftCnt);

	/* calculate x*x*c0 */
	/* ---------------- */

	swIn = extract_h(LwIn);
	swInSqrd = mult_r(swIn, swIn);
	LwIn = L_mult(swInSqrd, swC0);

	/* add x*c1 */
	/* --------- */

	LwIn = L_mac(LwIn, swIn, swC1);

	/* add c2 */
	/* ------ */

	LwIn = L_add(LwIn, L_deposit_h(swC2));

	/* apply *(4/32) */
	/* ------------- */

	LwIn = L_shr(LwIn, 3);
	LwIn = LwIn & 0x03ffffff;
	siShiftCnt = shl(siShiftCnt, 10);
	LwIn = L_add(LwIn, L_deposit_h(siShiftCnt));

	/* return log2 */
	/* ----------- */

	return (LwIn);
}

/***************************************************************************
 *
 *   FUNCTION NAME: fnLog
 *
 *   PURPOSE:
 *     The purpose of this function is to take the natural log of input and
 *     divide by 32 and return; i.e. output = log(input)/32
 *
 *   INPUTS:
 *
 *     L_Input
 *                     input
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     Longword
 *                     output
 *
 *   DESCRIPTION:
 *
 *     log(x) = log(2) * log2(x)
 *            = 0.693147 * log2(x)
 *
 *     log2(x) = 4.0 * (-.3372223*x*x + .9981958*x -.6626105)
 *                           c0            c1          c2   (includes sign)
 *
 *************************************************************************/

Longword fnLog(Longword L_Input)
{

	static Shortword
	    Scale = 22713;			/* 0.693147 = log(2) */
	Longword LwIn;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	/* 0.693147*log2(x) */
	/* ---------------- */

	LwIn = fnLog2(L_Input);
	LwIn = L_mpy_ls(LwIn, Scale);

	return (LwIn);
}

/***************************************************************************
 *
 *   FUNCTION NAME: fnLog10
 *
 *   PURPOSE:
 *     The purpose of this function is to take the log base 10 of input and
 *     divide by 32 and return; i.e. output = log10(input)/32
 *
 *   INPUTS:
 *
 *     L_Input
 *                     input
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     Longword
 *                     output
 *
 *   DESCRIPTION:
 *
 *     log10(x) = log10(2) * log2(x)
 *              = 0.30103  * log2(x)
 *
 *     log2(x) = 4.0 * (-.3372223*x*x + .9981958*x -.6626105)
 *                           c0            c1          c2   (includes sign)
 *
 *************************************************************************/

Longword fnLog10(Longword L_Input)
{

	static Shortword
	    Scale = 9864;			/* 0.30103 = log10(2) */
	Longword LwIn;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	/* 0.30103*log2(x) */
	/* ------------------- */

	LwIn = fnLog2(L_Input);
	LwIn = L_mpy_ls(LwIn, Scale);

	return (LwIn);
}

/***************************************************************************
 *
 *   FUNCTION NAME: fnExp2
 *
 *   PURPOSE:
 *     The purpose of this function is to implement a base two exponential
 *     2**(32*x) by polynomial approximation
 *
 *
 *   INPUTS:
 *
 *     L_Input
 *                     unnormalized input exponent (input range constrained
 *                     to be < 0)
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     LwIn
 *                     exponential output
 *
 *   DESCRIPTION:
 *
 *     polynomial approximation is used for the generation of the exponential
 *
 *     2**(32*X) = 0.1713425*X*X + 0.6674432*X + 0.9979554
 *                     c2              c1            c0
 *
 *************************************************************************/

Longword fnExp2(Longword L_Input)
{

/*_________________________________________________________________________
 |                                                                         |
 |                           Local Static Variables                        |
 |_________________________________________________________________________|
*/
	static Shortword pswPCoefE[3] =
	{							/* c2,   c1,    c0 */
		0x15ef, 0x556f, 0x7fbd
	};

/*_________________________________________________________________________
 |                                                                         |
 |                            Automatic Variables                          |
 |_________________________________________________________________________|
*/

	Shortword swTemp1, swTemp2, swTemp3, swTemp4;
	Longword LwIn;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	/* initialize */
	/* ---------- */

	swTemp3 = 0x0020;

	/* determine normlization shift count */
	/* ---------------------------------- */

	swTemp1 = extract_h(L_Input);
	LwIn = L_mult(swTemp1, swTemp3);
	swTemp2 = extract_h(LwIn);

	/* determine un-normalized shift count */
	/* ----------------------------------- */

	swTemp3 = -0x0001;
	swTemp4 = sub(swTemp3, swTemp2);

	/* normalize input */
	/* --------------- */

	LwIn = LwIn & 0xffff;
	LwIn = L_add(LwIn, L_deposit_h(swTemp3));

	LwIn = L_shr(LwIn, 1);
	swTemp1 = extract_l(LwIn);

	/* calculate x*x*c2 */
	/* ---------------- */

	swTemp2 = mult_r(swTemp1, swTemp1);
	LwIn = L_mult(swTemp2, pswPCoefE[0]);

	/* calculate x*x*c2 + x*c1 */
	/* ----------------------- */

	LwIn = L_mac(LwIn, swTemp1, pswPCoefE[1]);

	/* calculate x*x*c2 + x*c1 + c0 */
	/* --------------------------- */

	LwIn = L_add(LwIn, L_deposit_h(pswPCoefE[2]));

	/* un-normalize exponent if its requires it */
	/* ---------------------------------------- */

	if (swTemp4 > 0)
	{
		LwIn = L_shr(LwIn, swTemp4);
	}

	/* return result */
	/* ------------- */

	return (LwIn);
}

/***************************************************************************
 *
 *   FUNCTION NAME: fnExp10
 *
 *   PURPOSE:
 *     The purpose of this function is to implement a base ten exponential
 *     10**(32*x) by polynomial approximation
 *
 *
 *   INPUTS:
 *
 *     L_Input
 *                     unnormalized input exponent (input range constrained
 *                     to be < 0)
 *
 *   OUTPUTS:
 *
 *     none
 *
 *   RETURN VALUE:
 *
 *     LwIn
 *                     exponential output
 *
 *   DESCRIPTION:
 *
 *     polynomial approximation is used for the generation of the exponential
 *
 *     10**(32*X) = 2**((32*X) / log10(2))
 *                = 2**((32*X) / 0.30103)
 *
 *     2**(32*X) = 0.1713425*X*X + 0.6674432*X + 0.9979554
 *                     c2              c1            c0
 *
 *************************************************************************/

Longword fnExp10(Longword L_Input)
{

/*_________________________________________________________________________
 |                                                                         |
 |                           Local Static Variables                        |
 |_________________________________________________________________________|
*/
	static Shortword
	    InvScale = 27213;		/* (1/log10(2))/4 */

/*_________________________________________________________________________
 |                                                                         |
 |                            Automatic Variables                          |
 |_________________________________________________________________________|
*/

	Longword LwIn;

/*_________________________________________________________________________
 |                                                                         |
 |                              Executable Code                            |
 |_________________________________________________________________________|
*/

	LwIn = L_mpy_ls(L_Input, InvScale);
	LwIn = L_shl(LwIn, 2);
	LwIn = fnExp2(LwIn);

	/* return result */
	/* ------------- */

	return (LwIn);
}
