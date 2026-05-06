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
/***********************************************************************
 *
 *   FILE : mathevrc.h
 *
 *   PURPOSE:
 *     
 *     Modified ETSI basic operations.  Bit-exact simulation of a
 *     generic 32 bit accumulator DSP chip (fractional math).  This
 *     version has a latching overflow bit (giOverflow) and
 *     non-compound MAC's (One where initial mult does not saturate)
 *     
 *   SCCS Data:
 *             File Version                  = 1.1
 *             Archived Date (put/delta)     = 5/20/96  14:49:49
 *             Archive Extraction Date (get) = 5/20/96  15:17:56
 *                
 *   Eric Winter, October 1995
 *     
 *
 ***********************************************************************/

#ifndef __MATHHALF
#define __MATHHALF

#include "typedefs.h"

/*_________________________________________________________________________
 |                                                                         |
 |                            Function Prototypes                          |
 |_________________________________________________________________________|
*/

/* addition */
/************/

Shortword add(Shortword var1, Shortword var2);  /* 1 ops */
Shortword sub(Shortword var1, Shortword var2);  /* 1 ops */
Longword L_add(Longword L_var1, Longword L_var2);       /* 2 ops */
Longword L_sub(Longword L_var1, Longword L_var2);       /* 2 ops */

/* multiplication */
/******************/

Shortword mult(Shortword var1, Shortword var2); /* 1 ops */
Longword L_mult(Shortword var1, Shortword var2);        /* 1 ops */
Shortword mult_r(Shortword var1, Shortword var2);       /* 2 ops */


/* arithmetic shifts */
/*********************/

Shortword shr(Shortword var1, Shortword var2);  /* 1 ops */
Shortword shl(Shortword var1, Shortword var2);  /* 1 ops */
Longword L_shr(Longword L_var1, Shortword var2);        /* 2 ops */
Longword L_shl(Longword L_var1, Shortword var2);        /* 2 ops */
Shortword shift_r(Shortword var, Shortword var2);       /* 2 ops */
Longword L_shift_r(Longword L_var, Shortword var2);     /* 3 ops */

/* absolute value  */
/*******************/

Shortword abs_s(Shortword var1);       /* 1 ops */
Longword L_abs(Longword var1);         /* 3 ops */


/* multiply accumulate  */
/************************/

Longword L_mac(Longword L_var3,
                      Shortword var1, Shortword var2);  /* 1 op */
Shortword mac_r(Longword L_var3,
                       Shortword var1, Shortword var2); /* 2 op */
Longword L_msu(Longword L_var3,
                      Shortword var1, Shortword var2);  /* 1 op */
Shortword msu_r(Longword L_var3,
                       Shortword var1, Shortword var2); /* 2 op */

/* negation  */
/*************/

Shortword negate(Shortword var1);      /* 1 ops */
Longword L_negate(Longword L_var1);    /* 2 ops */


/* Accumulator manipulation */
/****************************/

Longword L_deposit_l(Shortword var1);  /* 1 ops */
Longword L_deposit_h(Shortword var1);  /* 1 ops */
Shortword extract_l(Longword L_var1);  /* 1 ops */
Shortword extract_h(Longword L_var1);  /* 1 ops */

/* Round */
/*********/

Shortword round32(Longword L_var1);      /* 1 ops */

/* Normalization */
/*****************/

Shortword norm_l(Longword L_var1);     /* 30 ops */
Shortword norm_s(Shortword var1);      /* 15 ops */

/* Division */
/************/
Shortword divide_s(Shortword var1, Shortword var2);     /* 18 ops */

/* Saturation manipulation routines */
/************************************/

int  clearOverflow(void);
int  isOverflow(void);
int  popOverflow(void);
int  setOverflow(void);
Longword L_saturate(double dvar1);


/* Non-saturating instructions */
/*******************************/
Longword L_add_c(Longword L_Var1, Longword L_Var2);     /* 2 ops */
Longword L_sub_c(Longword L_Var1, Longword L_Var2);     /* 2 ops */
Longword L_sat(Longword L_var1);       /* 4 ops */
Longword L_macNs(Longword L_var3,
                        Shortword var1, Shortword var2);        /* 1 ops */
Longword L_msuNs(Longword L_var3,
                        Shortword var1, Shortword var2);        /* 1 ops */



/* OP Counter defines  LT 6/96  */
/********************************/
//#define OP_COUNT(x) op_counter+=x
#define OP_COUNT(x) 
#define op_counter (evrc_current_context()->dsp.op_counter)
#define OP_RESET    op_counter=0

#endif
