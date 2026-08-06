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
