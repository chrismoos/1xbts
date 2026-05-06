/**********************************************************************
Each of the companies; Qualcomm, Motorola, Lucent, and Nokia (hereinafter 
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
all Normative portions of the EVRC-WB Variable Rate Speech Codec as 
it exists on the date of submittal of this form, should the EVRC-WB be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC-WB or the field of use of practice of the 
EVRC-WB Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/

#ifndef _FPLIB_DEF_
#define _FPLIB_DEF_

#include <stdio.h>
#include <stdlib.h>		/* For malloc() */
#define MAX_32 (int)0x7fffffffL
#define MIN_32 (int)0x80000000L

#define MAX_16 (short)0x7fff
#define MIN_16 (short)0x8000

#define MAXSHORT 32768
#define MAXSIZE  10
#define MAXLENGTH 15
#define Boolean short
#define TRUE  1
#define FALSE 0
typedef short Word16;
typedef int Word32;
typedef struct
{
    short x[MAXSIZE];
}
BigInt;

#if 1
short isgreaterB (BigInt, BigInt);
short islessB (BigInt, BigInt);
short isequalB (BigInt, BigInt);
BigInt addBigInt (BigInt, BigInt);
BigInt subBigInt (BigInt, BigInt);
BigInt mulBigInt (BigInt, BigInt);
BigInt divBigInt (BigInt, BigInt, BigInt *);
BigInt divBigInt (BigInt, int, Word16 *);
BigInt shiftright (BigInt, Word16);
BigInt shiftleft (BigInt, Word16);
#endif





class VPint
{
  public:
    VPint ();
    VPint (Word16);
    VPint (const VPint &);
      VPint (BigInt);
     ~VPint ();
    int getNumBits ();
    Word16 *unpackToWordArray (Word16 *, Word16);
    int *unpackToWordArray (int *, Word16);
    void packFromWordArray (Word16 *, Word16);
    void packFromWordArray (int *, Word16);
      VPint & operator= (const VPint &);	/* Assignment operator. */
      VPint & operator= (Word16);	/* Assignment operator. */
      VPint & operator= (int);	/* Assignment operator. */
      VPint & operator+ ();	/* Unary plus sign (don't do anything) */
      VPint & operator- ();	/* Unary minus sign (change sign only) */
    VPint operator+ (const VPint &);	/* Binary addition operation */
    VPint operator- (const VPint &);	/* Binary subtraction operation */
    VPint operator* (const VPint &);	/* multiplication operation */
    VPint operator* (Word16);	/* multiplication operation */
    VPint operator* (int);	/* multiplication operation */
    VPint operator/ (const VPint &);	/* division operation */
    VPint operator/ (int);	/* division operation */
    VPint operator/ (Word16);	/* division operation */
    VPint operator% (const VPint &);	/* modulo operation */
      VPint & operator+= (const VPint &);	/* add to left operand (accumulate) operation */
      VPint & operator-= (const VPint &);	/* subtract from left operand operation */
      VPint & operator++ ();	/* increment operand by one */
      VPint & operator-- ();	/* decrement operand by one */
    VPint operator| (const VPint &);	/* bitwise OR operation */
    VPint operator& (const VPint &);	/* Binary bitwise AND operation */
    VPint operator^ (const VPint &);	/* bitwise XOR operation */
    Boolean operator< (const VPint &);	/* Logical "less than" comparison */
    Boolean operator> (const VPint &);	/* Logical "greater than" comparison */
    Boolean operator<= (const VPint &);	/* Logical "less than or equal to" comparison */
    Boolean operator>= (const VPint &);	/* Logical "greater than or equal to" comparison */
    Boolean operator== (const VPint &);	/* Logical "equal to" comparison */
    Boolean operator!= (const VPint &);	/* Logical "not equal to" comparison */
    VPint operator<< (Word16);	/* Shift-Left operation */
    VPint operator>> (Word16);	/* Shift-Right operation */
    VPint extract (Word16);
    Word16 getKthWord (Word16);
    void clear (Word16, Word16);
    BigInt mpvalue;
};


typedef VPint intVar;
#endif
