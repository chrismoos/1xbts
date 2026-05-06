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
/***************************************************************************
 *
 *   File Name:  mathdp31.c
 *
 *   Purpose:  Contains functions increased-precision arithmetic operations.
 *             Unsigned multiplication is used to calculate interim products.
 *
 *      Below is a listing of all the functions in this file.  There
 *      is no interdependence among the functions.
 *
 *      L_mpy_ls()
 *      L_mpy_ll()
 *
 *
 *  There are two types of double precision multiplies supported in this
 *  file. The default uses unsigned multiplication for the interim steps.
 *  The alternate may be used when compiling the library by defining
 *  "USE_ALT_DP31" on the compile line. See the makefile for more info.
 *
 ***************************************************************************/
/*_________________________________________________________________________
 |                                                                         |
 |                              Include Files                              |
 |_________________________________________________________________________|
*/
#include <math.h>

//#include "mathevrc.h"
#include "dsp_math.h"
#include "typedefs.h"

#ifndef USE_ALT_DP31
/****************************************************************************
 *
 *
 *     FUNCTION NAME: L_mpy_ll
 *
 *     PURPOSE:    Multiply a 32 bit number (L_var1) and a 32 bit number
 *                 (L_var2), and return a 32 bit result.
 *
 *     INPUTS:
 *
 *       L_var1             A Longword input variable
 *
 *       L_var2             A Longword input variable
 *
 *     OUTPUTS:             none
 *
 *     IMPLEMENTATION:
 *
 *        Performs a 32x32 bit multiply, Complexity=?? ops
 *
 *        Let x1x0, or y1y0, be the two constituent halves
 *        of a 32 bit number.  This function performs the
 *        following:
 *
 *        low = [(unsigned)x0 * (unsigned)y0] <<1;  (low * low)
 *        mid1 = [(unsigned)x0 * y1] << 1;          (low * high)
 *        mid2 = [(unsigned)y0 * x1] << 1;          (low * high)
 *        mid =  [mid1 + mid2] >> 16 ;              (sum so far)
 *        output = (x1*y1)<<1 + mid;                (high * high)
 *
 *        Note that this version uses unsigned * signed to reduce complexity
 *        and gain precision.
 *
 *
 *     RETURN VALUE:        A Longword value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/
Longword L_mpy_ll(Longword L_var1, Longword L_var2)
{
	double  aReg;
    Longword lvar;

    OP_COUNT(3);
    /* (unsigned)low1 * (unsigned)low1 */
    aReg = (double)(0xffff & L_var1) * (double)(0xffff & L_var2) * 2.0;

    OP_COUNT(1);
    /* >> 16 */
    aReg = (aReg / 65536);
    aReg = floor(aReg);

    OP_COUNT(1);
    /* (unsigned)low1 * (signed)high2 */
    aReg += (double)(0xffff & L_var1) * ((double)L_shr(L_var2,16)) * 2.0;

    OP_COUNT(1);
    /* (unsigned)low2 * (signed)high1 */
    aReg += (double)(0xffff & L_var2) * ((double)L_shr(L_var1,16)) * 2.0;

    OP_COUNT(1);
    /* >> 16 */
    aReg = (aReg / 65536);
    aReg = floor(aReg);

    OP_COUNT(1);
    /* (signed)high1 * (signed)high2 */
    aReg += (double)(L_shr(L_var1,16)) * (double)(L_shr(L_var2,16)) * 2.0;

    /* saturate result.. */
    lvar = L_saturate(aReg);

    return(lvar);
}

/****************************************************************************
 *
 *     FUNCTION NAME: L_mpy_ls
 *
 *     PURPOSE:    Multiply a 32 bit number (L_var2) and a 16 bit
 *                 number (var1) returning a 32 bit result.
 *
 *     INPUTS:
 *
 *       L_var2             A Longword input variable
 *
 *       var1               A Shortword input variable
 *
 *     OUTPUTS:             none
 *
 *     RETURN VALUE:        A Longword value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/
Longword L_mpy_ls(Longword L_var2, Shortword L_var1)
{
	double  aReg;
    Longword lvar;

    OP_COUNT(1);
    /* (unsigned)low2 * (signed)low1 */
	aReg  = (double)(0xffff & L_var2) * (double)(L_var1) * 2.0;

    OP_COUNT(1);
    /* >> 16 */
    aReg = (aReg / 65536);
    aReg = floor(aReg);

    OP_COUNT(1);
    /* (signed)high2 * (signed)low1 */
    aReg += ((double)L_shr(L_var2,16)) * (double)(L_var1) * 2.0;

    /* Saturate to 32 bits.. */
    lvar = L_saturate(aReg);
    return(lvar);
}

#else
/* (Use alternate double precision library routines... ) */
/*======================================================================*/
/*  Motorola Diversified Technology Services                            */
/*  EVRC fixed-point C simulation.                                      */
/*  Copyright (C) 1996 Motorola Diversified Technology Services.        */
/*  All rights reserved. Motorola proprietary and confidential.         */
/*----------------------------------------------------------------------*/
/****************************************************************************
 *
 *     FUNCTION NAME: L_mpy_ll
 *
 *     PURPOSE:    Multiply a 32 bit number (L_var1) and a 32 bit number
 *                 (L_var2), and return a 32 bit result.
 *
 *     INPUTS:
 *
 *       L_var1             A Longword input variable
 *
 *       L_var2             A Longword input variable
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
 *     RETURN VALUE:        A Longword value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/

Longword L_mpy_ll(Longword L_var1, Longword L_var2)
{
  Shortword swLow1,
         swLow2,
         swHigh1,
         swHigh2;
  Longword L_varOut,
         L_low,
         L_mid1,
         L_mid2,
         L_mid;


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
 *       L_var2             A Longword input variable
 *
 *       var1               A Shortword input variable
 *
 *     OUTPUTS:             none
 *
 *     RETURN VALUE:        A Longword value
 *
 *     KEYWORDS: mult,mpy,multiplication
 *
 ***************************************************************************/

Longword L_mpy_ls(Longword L_var2, Shortword var1)
{
  Longword L_varOut;
  Shortword swtemp;

  swtemp = shr(extract_l(L_var2), 1);
  swtemp = (short) 32767 & (short) swtemp;

  L_varOut = L_mult(var1, swtemp);
  L_varOut = L_shr(L_varOut, 15);
  L_varOut = L_mac(L_varOut, var1, extract_h(L_var2));
  return (L_varOut);
}
#endif
