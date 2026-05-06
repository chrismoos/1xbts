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
 
/*============================================================================*/

/*===================================================================*/
/*                                                                   */
/* Conexant Systems, Inc., Qualcomm Inc., Nokia Inc,. Nortel         */
/* Networks, Lucent Technologies, and Motorola Inc. grants a free,   */
/* irrevocable license to 3GPP2 and its Organizational Partners to   */
/* incorporate text or other copyrightable material contained in the */
/* contribution and any modifications thereof in the creation of     */
/* 3GPP2 publications; to copyright and sell in Organizational       */
/* Partner's name any Organizational Partner's standards publication */
/* even though it may include portions of the contribution; and at   */
/* the Organizational Partner's sole discretion to permit others to  */
/* reproduce in whole or in part such contributions or the resulting */
/* Organizational Partner's standards publication. The contributor   */
/* must also be willing to grant licenses under such contributor     */
/* copyrights to third parties on reasonable, non-discriminatory     */
/* terms and conditions, as appropriate.                             */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* Copyright 2001 Conexant Systems, Inc.  This document is provided  */
/* as a discussion instrument only and is not binding on Conexant    */
/* Systems, Inc. The contributor reserves the right to add to, amend */
/* or otherwise modify the contents thereof.                         */
/*                                                                   */
/* Conexant Systems, Inc. grants a free, irrevocable license to 3GPP2*/
/* and its Organization Partners to incorporate, for any legitimate  */
/* 3GPP2 purpose, any copyrightable material contained in this       */
/* contribution and any revisions thereof, in publications of 3GPP2. */
/* The contributor may hold one or more patents or applications for  */
/* letters patent that cover the information contained in this       */
/* contribution.  Noting contained in this copyright statement shall */
/* be construed as conferring by implication or estoppel, or         */
/* otherwise a license or any other right under any existing  or     */
/* later issuing patent, whether or not the use of information       */
/* herein necessarily employs an invention of any such patent or     */
/* later issuing patent.                                             */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* Copyright QUALCOMM Incorporated. All rights reserved.             */
/* The information contained in this contribution is provided for    */
/* the sole purpose of promoting discussion within the 3GPP2 and its */ 
/* Organization Partners and is not binding on the contributor.      */
/* The contributor reserves the right to add to, amend, or withdraw  */
/* the statements contained herein.                                  */
/* QUALCOMM Incorporated grants a free, irrevocable license to 3GPP2 */
/* and its Organization Partners to incorporate text or other        */
/* copyrightable material contained in the  contribution and any     */
/* modifications thereof in the creation of 3GPP2 publications;      */
/* to copyright and sell in Organizational Partner's name any        */
/* Organizational Partner's standards publication even though it may */
/* include portions of the contribution; and at the Organization     */
/* Partner's sole discretion to permit others to reproduce in whole  */
/* or in part such contributions or the resulting Organizational     */
/* Partner's standards publication.  The contributor may hold one or */
/* more patents or copyrights that cover information contained in    */
/* this contribution. A license will be made available to applicants */
/* under reasonable terms and conditions that are demonstrably free  */
/* of any unfair discrimination.                                     */
/* Nothing contained herein shall be construed as conferring by      */
/* implication, estoppel, or otherwise any license or right under    */
/* any patent, whether or not the use of information herein          */
/* necessarily employs an invention of any existing or later issued  */
/* patent, or copyright. The contributor reserves the right to use   */
/* all material submitted in this contribution for his own purposes, */
/* including republication and distribution to others.               */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* The proposals in this submission have been formulated by Nokia,   */
/* Inc. (Nokia), to assist the 3GPP2 Standards Committee. This       */
/* document is offered  to the committee as a basis for discussion   */
/* and is not binding on Nokia. The requirements are subject to      */
/* change in form and in numerical values after more study. Nokia    */
/* specifically reserves the right to add to, or amend, the          */
/* quantitative statements made herein. Nothing contained herein     */
/* shall be construed as conferring by implication, estoppel, or     */
/* otherwise any license or right under any patent, whether or not   */
/* the use of information herein necessarily employs an invention of */
/* any existing or later issued patent.                              */
/* Copyright 2001 Nokia, Inc. All Rights Reserved. Nokia hereby      */
/* gives permission for copying this submission for the legitimate   */
/* purposes of the 3GPP2 Standards Committee, provided Nokia, Inc.   */
/* is credited on all copies. Distribution or reproduction of this   */
/* document, by any means, electronic, mechanical, or otherwise, in  */
/* its entirety, or any portion thereof, for monetary gain or any    */
/*  non-3GPP2 purposes is expressly prohibited.                      */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* Nortel Networks grants a free, irrevocable license to 3GPP2 and   */
/* its Organizational Partners to incorporate text or other          */
/* copyrightable material contained in the contribution and any      */
/* modifications thereof in the creation of 3GPP2 publications; to   */
/* copyright and sell in Organizational Partner's name any           */
/* Organizational Partner's standards publication even though it may */
/* include portions of the contribution; and at the Organizational   */
/* Partner's sole discretion to permit others to reproduce in whole  */
/* or in part such contributions or the resulting Organizational     */
/* Partner's standards publication. The contributor must also be     */
/* willing to grant licenses under such contributor copyrights to    */
/* third parties on reasonable, non-discriminatory terms and         */
/* conditions, as appropriate.                                       */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* Notice:                                                           */
/* This contribution has been prepared by the contributor to assist  */
/* 3GPP2 Technical Specifications Group C. This document is offered  */
/* to 3GPP2 Technical Specifications Group C as a basis for          */
/* discussion and should not be construed as a binding proposal on   */
/* Lucent Technologies Inc. or any other company.  Specifically,     */
/* Lucent Technologies Inc. reserves the right to modify, amend, or  */
/* withdraw the statements contained herein.                         */
/*                                                                   */
/* Permission is granted to 3GPP2 Technical Specifications Group C   */
/* participants to copy any portion of this document for legitimate  */
/* purposes of 3GPP2 Technical Specifications Group C. Copying for   */
/* monetary gain or other non-3GPP2 Technical Specifications Group C */
/* purposes is prohibited.                                           */
/*                                                                   */
/*-------------------------------------------------------------------*/
/*                                                                   */
/* Grant of license Motorola Inc. grants a free, irrevocable license */
/* to 3GPP2 and its organizational partners to incorporate Motorola- */
/* supplied text or other copyrightable material contained in the    */
/* contribution and any modifications thereof in the creation of     */ 
/* 3GPP2 publications, to copyright and sell in organizational       */
/* partners name any organizational partners standards publications  */
/* even though it may include portions of the contribution; and at   */
/* the organizational partners sole discretion to permit others      */
/* to reproduce in whole or in part such contributions or the        */
/* resulting organizational partners standards publication. Motorola */
/* is also willing to grant licenses under such Motorola copyrights  */
/* to third parties on reasonable, non-discriminatory terms and      */
/* conditions, as appropriate.                                       */
/*                                                                   */
/* Notice:                                                           */
/* This document has been prepared by Motorola Inc. to assist the    */
/* 3GPP2 standards committee. This document is offered to the        */
/* committee as a basis for discussion and should not be considered  */
/* as a binding proposal on Motorola Inc.  or any other company.     */
/* Specifically, Motorola Inc. reserves the right to modify, amend,  */
/* or withdraw the statement contained herein. Permission is granted */
/* to 3GPP2 and its organizational partners to copy any portion of   */
/* this document for the legitimate purposes of the 3GPP2.  Copying  */
/* this document for monetary gain or other non-3GPP2 purpose is     */
/* prohibited.  Motorola Inc. may hold one or more patents of        */
/* copyrights that cover information contained in this contribution, */
/* and agrees that a license under those rights will be made         */
/* available on reasonable and non-discriminatory terms and          */
/* conditions, subject to receiving a reciprocal license in return.  */
/* Nothing contained herein shall be construed as conferring by      */
/* implication, estoppel, or otherwise any license or right under    */
/* any patent, whether or not the use of information herein          */
/* necessarily employs an invention of any existing or later issued  */
/* patent, or copyright.                                             */
/*                                                                   */
/* Notice                                                            */
/* Permission is granted to 3GPP2 participants to copy any portion of*/
/* this contribution for the legitimate purpose of the 3GPP2.        */
/* Copying this contribution for monetary gain or other non-3GPP2    */
/* purpose is prohibited.                                            */
/*                                                                   */
/*===================================================================*/
/* FILE: basic_op40.c                                                */
/*-------------------------------------------------------------------*/
/* PURPOSE : Modified TIA basic operations for 40-bit accumulators   */
/*===================================================================*/

/*___________________________________________________________________________
 |                                                                           
 |           Basic arithmetic operators using 16-bit/40-bit arithmatic   
 |                                                                           
 | These operators come originally from ETSI TCH-HS library 
 | (used in GSM EFR 13kbit/s). 
 | They were modified to support 16-bit/40-bit arithmetic operations.  
 | Standard 32-bit basic operators are replaced by 40-bit operators to be 
 | compatible with   
 | most of the new 40-bit DSP in the industry.  The use of 40 bits        
 | accumulator allow the DSP to avoid overflow/saturation in the calculation 
 | of energy, correlation, filtering or any other procedure that need more than
 | 32 bits in the summation of product.  These 40-bit operators should be  
 | used when the target platform is DSP with 40-bit accumulator.            
 |                                                                           
 | Word40 is represented using a floating point format (double)
 | ~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~~
 | Because some platforms do not allow integer of 40 bits, a floating point 
 | format is used, with at less 40 bits in the mantissa, to represent long   
 | integer of 40 bits.  MAX_40 and MIN_40 are defined in this file.          
 |____________________________________________________________________________
*/

/***********************************************************************/

/*_________________________________________________________________________
 |                                                                         |
 |                            Include Files                                |
 |_________________________________________________________________________|
*/

#include <math.h>

#include "typedef_fx.h"
#include "basic_op40.h"

#ifdef WMOPS_FX
#include "typedef_fx.h"
#include "main_fx.h"
#include "const_fx.h"
#include "ext_var_fx.h"
#include "lib_wmp_fx.h"
#endif

//*************************************************************************
// local functions for 40 bit accumulator	 
/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_shift_left40                                          |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Perform an integer arithmetic shift on the double format value.         |
 |   If nbits > 0, arithmetically shift the value left nbits positions.      |
 |   Zero fill the nbits LSB of the value.                                   |
 |   If nbits < 0, arithmetically shift the value right by -nbits positions  |
 |   with sign extension.                                                    |
 |   No overflow control and saturation.                                     |
 |___________________________________________________________________________|
*/

static Word40 L_shift_left40(Word40 value, Word16 nbits)
{
  if (nbits > 40) nbits = 40;      /* to avoid needless loop after overflow */
  if (nbits < -40) nbits = -40;    

  if (nbits < 0)
  {
    for(; nbits<0; nbits++)
    {
      value = value * 0.5;         /* remove fraction bits */
    }
    value = floor(value);
  }
  else
  {
    value = floor(value);          /* remove fraction bits */
    for(; nbits>0; nbits--)
    {
      value = value * 2;
    }
  }

  return(value);
}

/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_saturate40                                            |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |    Limit the value to the range of a 40 bit word.                         |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    var_out  40 bit short signed integer (Word40) whose value falls in the |
 |             range : MIN_40 <= var_out <= MAX_40.                          |
 |___________________________________________________________________________|
*/

static Word40 L_saturate40(Word40 value)
{
  Word40 L_var_out;

  L_var_out = value;
  
  if (L_var_out > MAX_40)
  {
    L_var_out = MAX_40;
    giOverflow = 1;
  }
  else if (L_var_out < MIN_40)
  {
    L_var_out = MIN_40;
    giOverflow = 1;
  }

  return(L_var_out);
}


//***************************************************************************
//***************************************************************************
//***************************************************************************
//***************************************************************************

// public functions for 40 bit accumulator	 
//    
// J. Klein, Conexant Systems   12/01/00

/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_add40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Add L_var1 by L_var2. Return a 40 bit result without saturation:        |
 |        L_add40 (L_var1, L_var2).                                          |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var1 <= MAX_40.                           |
 |                                                                           |
 |                                                                           |
 |    L_var2   32 bit long signed integer (Word32) whose value falls in      |
 |             the : MIN_32 <= L_var2 <= MAX_32.                             |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_add40(Word40 L_var1, Word32 L_var2)
{
  Word40 L_var_out;

  L_var_out = L_var1 + (Word40)L_var2;

  L_var_out = L_saturate40(L_var_out);

#ifdef WMOPS_FX
  counter_fx.L_add40++;
#endif

  return(L_var_out);
}

/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_sub40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Substract L_var1 by L_var2. Return a 40 bit result without saturation:  |
 |        L_sub40 (L_var1, L_var2).                                          |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var1 <= MAX_40.                           |
 |                                                                           |
 |                                                                           |
 |    L_var2   32 bit long signed integer (Word32) whose value falls in      |
 |             the : MIN_32 <= L_var2 <= MAX_32.                             |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_sub40(Word40 L_var1, Word32 L_var2)
{
  Word40 L_var_out;

  L_var_out = L_var1 - (Word40)L_var2;

  L_var_out = L_saturate40(L_var_out);

#ifdef WMOPS_FX
  counter_fx.L_sub40++;
#endif

  return(L_var_out);
}


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_mac40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Multiply var1 by var2 and shift the result left by 1. Add the 40 bit    |
 |   result to L_var3 with saturation, return a 40 bit result:               |
 |        L_mac(L_var3,var1,var2) = L_add(L_var3,(L_mult(var1,var2)).        |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var3   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var3 <= MAX_40.                           |
 |                                                                           |
 |    var1     16 bit short signed integer (Word16) whose value falls in  |
 |             the : MIN_16 <= var1 <= MAX_16.                               |
 |                                                                           |
 |    var2     16 bit short signed integer (Word16) whose value falls in  |
 |             the : MIN_16 <= var1 <= MAX_16.                               |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_mac40(Word40 L_var3, Word16 var1, Word16 var2)
{
  Word40 L_var_out;

  L_var_out = L_var3 + ((Word40)var1 * (Word40)var2 * 2.0);

  L_var_out = L_saturate40(L_var_out);

#ifdef WMOPS_FX
  counter_fx.L_mac40++;
#endif

  return(L_var_out);
}


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_msu40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Multiply var1 by var2 and shift the result left by 1. Subtract the 40   |
 |   bit result to L_var3 with saturation, return a 40 bit result:           |
 |        L_msu(L_var3,var1,var2) = L_sub(L_var3,(L_mult(var1,var2)).        |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var3   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var3 <= MAX_40.                           |
 |                                                                           |
 |    var1     16 bit short signed integer (Word16) whose value falls in  |
 |             the range : MIN_16 <= var1 <= MAX_16.                         |
 |                                                                           |
 |    var2     16 bit short signed integer (Word16) whose value falls in  |
 |             therange : MIN_16 <= var1 <= MAX_16.                          |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_msu40(Word40 L_var3, Word16 var1, Word16 var2)
{
  Word40 L_var_out;

  L_var_out = L_var3 - ((Word40)var1 * (Word40)var2 * 2.0);

  L_var_out = L_saturate40(L_var_out);

#ifdef WMOPS_FX
  counter_fx.L_msu40++;
#endif

  return(L_var_out);
}


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_shr40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Arithmetically shift the 40 bit input L_var1 right var2 positions with  |
 |   sign extension. If var2 is negative, arithmetically shift L_var1 left   |
 |   by -var2 and zero fill the var2 LSB of the result. Saturate the result  |
 |   in case of underflows or overflows.                                     |
 |                                                                           |
 |   Complexity weight : 2                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var3 <= MAX_40.                           |
 |                                                                           |
 |    var2     16 bit short signed integer (Word16) whose value falls in  |
 |             therange : MIN_16 <= var1 <= MAX_16.                          |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_shr40(Word40 L_var1, Word16 var2)
{
  Word40 L_var_out;

  Word40 L_shl40(Word40 L_var1, Word16 var2);

  if (var2 < 0)
  {
    L_var_out = L_shl40(L_var1,(Word16)(-var2));
#ifdef WMOPS_FX
    counter_fx.L_shl40--;
#endif
  }
  else
  {
    L_var_out = L_shift_left40(L_var1, (Word16)(-var2));
  }

#ifdef WMOPS_FX
  counter_fx.L_shr40++;
#endif

  return(L_var_out);
}

/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_shl40                                                 |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Arithmetically shift the 40 bit input L_var1 left var2 positions. Zero  |
 |   fill the var2 LSB of the result. If var2 is negative, L_var1 right by   |
 |   -var2 arithmetically shift with sign extension. Saturate the result in  |
 |   case of underflows or overflows.                                        |
 |                                                                           |
 |   Complexity weight : 2                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var3 <= MAX_40.                           |
 |                                                                           |
 |    var2     16 bit short signed integer (Word16) whose value falls in  |
 |             therange : MIN_16 <= var1 <= MAX_16.                          |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= L_var_out <= MAX_40.                        |
 |___________________________________________________________________________|
*/

Word40 L_shl40(Word40 L_var1, Word16 var2)
{
  Word40 L_var_out;

  if (var2 <= 0)
  {
    L_var_out = L_shr40(L_var1,(Word16)(-var2));
#ifdef WMOPS_FX
    counter_fx.L_shr40--;
#endif
  }
  else
  {
    L_var_out = L_shift_left40(L_var1, var2);

    L_var_out = L_saturate40(L_var_out);
  }

#ifdef WMOPS_FX
  counter_fx.L_shl40++;
#endif

  return(L_var_out);
}


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_sat32_40                                              |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |    40 bit L_var1 is limited to the range MAX_32..MIN_32.  L_var1 is set   |
 |    to MAX_32 on overflow or MIN_32 on underflow.                          |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= var1 <= MAX_40.                             |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    L_var_out                                                              |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_32 <= var_out <= MAX_32.                          |
 |___________________________________________________________________________|
*/

Word40 L_sat32_40(Word40 L_var1)
{
  Word40 L_var_out;

  if (L_var1 > MAX_32)
  {
    L_var_out = MAX_32;
    giOverflow = 1;
  }
  else if (L_var1 < MIN_32)
  {
    L_var_out = MIN_32;
    giOverflow = 1;
  }
  else L_var_out = L_var1;

#ifdef WMOPS_FX
  counter_fx.L_sat32_40++;
#endif

  return (L_var_out);
}


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : norm32_l_40                                             |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Produces the number of left shift needed to normalize in 32 bits format |
 |   the 40 bit variable l_var1 for positive values on the interval with     |
 |   minimum of (MAX_32+1)/2 and maximum of MAX_32, and for negative values  |
 |   on the interval with minimum of MIN_32 and maximum of (MIN_32/2)+1;     |
 |   in order to normalize the result, the following operation must be done: |
 |                                                                           |
 |                   norm_L_var1 = L_shl40(L_var1,norm32_l(L_var1)).         |
 |                                                                           |
 |   Complexity weight : 3                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= var1 <= MAX_40.                             |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    var_out  16 bit short signed integer (Word16) whose value falls in  |
 |             therange : -8 <= var_out <= 31.                               |
 |___________________________________________________________________________|
*/

Word16 norm32_l40(Word40 L_var1)
{
  Word16 var_out;

  L_var1 = floor(L_var1);
  var_out = 0;

  if (L_var1 != 0)
  {
    while ((L_var1 > MIN_32) && (L_var1 < MAX_32))
    {
      L_var1 = L_shift_left40(L_var1, 1);
      var_out++;
    }

    while ((L_var1 < MIN_32) || (L_var1 > MAX_32))
    {
      L_var1 = L_shift_left40(L_var1, -1);
      var_out--;
    }
  }

#ifdef WMOPS_FX
  counter_fx.norm32_l40++;
#endif

  return(var_out);
 }


/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : extract_l40                                             |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Get the lower 16 bits of a 40-bit variable into a 16-bit variable       |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= var1 <= MAX_40.                             |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |    var_out  16 bit short signed integer (Word16) which may take any value |
 |___________________________________________________________________________|
*/

Word16 extract_l40(Word40 L_var1)
{
  Word16 out;
  
  out = (Word16)(L_var1 - floor(L_var1/65536.)*65536.);

#ifdef WMOPS_FX
  counter_fx.extract_l40++;
#endif

  return (out);
}



/*___________________________________________________________________________
 |                                                                           |
 |   Function Name : L_deposit_l40                                           |
 |                                                                           |
 |   Purpose :                                                               |
 |                                                                           |
 |   Replace lower 16 bits of a 40-bit variable by those of 16-bit input     |
 |   variable var_in                                                         |
 |                                                                           |
 |   Complexity weight : 1                                                   |
 |                                                                           |
 |   Inputs :                                                                |
 |                                                                           |
 |    L_var1   40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= var1 <= MAX_40.                             |
 |    var_in   16 bit short signed integer (Word16) which may take any value |
 |                                                                           |
 |   Return Value :                                                          |
 |                                                                           |
 |             40 bit long signed integer (Word40) whose value falls in the  |
 |             range : MIN_40 <= var1 <= MAX_40.                             |
 |___________________________________________________________________________|
*/

Word40 L_deposit_l40(Word40 L_var1, Word16 var_in)
{
  Word40 out;
  Word32 tmp32;
    
  tmp32 = 0x0000ffff & (Word32)var_in;  /* no sign extend  */
  out = L_add40(floor(L_var1/65536.)*65536., tmp32);

#ifdef WMOPS_FX
  counter_fx.L_add40--;
  counter_fx.L_deposit_l40++;
#endif

  return (out);
}
