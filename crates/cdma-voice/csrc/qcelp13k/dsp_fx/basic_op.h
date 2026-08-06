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
/***********************************************************************
 *
 *   FILE : basic_op.h (essentially identical to mathevrc.h)
 *
 *   PURPOSE:
 *
 *     Modified TIA basic operations.  Bit-exact simulation of a
 *     generic 32 bit accumulator DSP chip (fractional math).  This
 *     version has a latching overflow bit (giOverflow) and
 *     non-compound MAC's (One where initial mult does not saturate)
 *
 ***********************************************************************/

#ifndef __MATHTIA
#define __MATHTIA

#include "typedef_fx.h"

#define MAX_32 (Word32)0x7fffffffL
#define MIN_32 (Word32)0x80000000L

#define MAX_16 (Word16)0x7fff
#define MIN_16 (Word16)0x8000

/*_________________________________________________________________________
 |                                                                         |
 |                            Function Prototypes                          |
 |_________________________________________________________________________|
*/

/* addition */
/************/

Word16 add(Word16 var1, Word16 var2);  /* 1 ops */
Word16 sub(Word16 var1, Word16 var2);  /* 1 ops */
Word32 L_add(Word32 L_var1, Word32 L_var2);       /* 2 ops */
Word32 L_sub(Word32 L_var1, Word32 L_var2);       /* 2 ops */

/* multiplication */
/******************/

Word16 mult(Word16 var1, Word16 var2); /* 1 ops */
Word32 L_mult(Word16 var1, Word16 var2);        /* 1 ops */
Word16 mult_r(Word16 var1, Word16 var2);       /* 2 ops */


/* arithmetic shifts */
/*********************/

Word16 shr(Word16 var1, Word16 var2);  /* 1 ops */
Word16 shl(Word16 var1, Word16 var2);  /* 1 ops */
Word32 L_shr(Word32 L_var1, Word16 var2);        /* 2 ops */
Word32 L_shl(Word32 L_var1, Word16 var2);        /* 2 ops */
Word16 shift_r(Word16 var, Word16 var2);       /* 2 ops */
Word32 L_shift_r(Word32 L_var, Word16 var2);     /* 3 ops */

/* absolute value  */
/*******************/

Word16 abs_s(Word16 var1);       /* 1 ops */
Word32 L_abs(Word32 var1);         /* 3 ops */


/* multiply accumulate  */
/************************/

Word32 L_mac(Word32 L_var3,
                      Word16 var1, Word16 var2);  /* 1 op */
Word16 mac_r(Word32 L_var3,
                       Word16 var1, Word16 var2); /* 2 op */
Word32 L_msu(Word32 L_var3,
                      Word16 var1, Word16 var2);  /* 1 op */
Word16 msu_r(Word32 L_var3,
                       Word16 var1, Word16 var2); /* 2 op */

/* negation  */
/*************/

Word16 negate(Word16 var1);      /* 1 ops */
Word32 L_negate(Word32 L_var1);    /* 2 ops */


/* Accumulator manipulation */
/****************************/

Word32 L_deposit_l(Word16 var1);  /* 1 ops */
Word32 L_deposit_h(Word16 var1);  /* 1 ops */
Word16 extract_l(Word32 L_var1);  /* 1 ops */
Word16 extract_h(Word32 L_var1);  /* 1 ops */

/* Round */
/*********/

Word16 round(Word32 L_var1);      /* 1 ops */

/* Normalization */
/*****************/

Word16 norm_l(Word32 L_var1);     /* 30 ops */
Word16 norm_s(Word16 var1);      /* 15 ops */

/* Division */
/************/
Word16 divide_s(Word16 var1, Word16 var2);     /* 18 ops */

/* Saturation manipulation routines */
/************************************/

int  clearOverflow(void);
int  isOverflow(void);
int  popOverflow(void);
int  setOverflow(void);


/* Non-saturating instructions */
/*******************************/
Word32 L_add_c(Word32 L_Var1, Word32 L_Var2);     /* 2 ops */
Word32 L_sub_c(Word32 L_Var1, Word32 L_Var2);     /* 2 ops */
Word32 L_macNs(Word32 L_var3,
                        Word16 var1, Word16 var2);        /* 1 ops */
Word32 L_msuNs(Word32 L_var3,
                        Word16 var1, Word16 var2);        /* 1 ops */

#endif
