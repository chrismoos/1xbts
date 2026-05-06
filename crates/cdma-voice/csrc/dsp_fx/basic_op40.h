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

/***********************************************************************
 *
 *
 *   DESCRIPTION:
 *     
 *     Modified TIA basic operations.  Bit-exact simulation of a
 *     generic 32 bit accumulator DSP chip (fractional math).  This
 *     version has a latching overflow bit (giOverflow) and
 *     non-compound MAC's (One where initial mult does not saturate)
 *     
 *     J. Klein, Conexant Systems, Inc.      11/30/00
 ***********************************************************************/

#ifndef __MATH40
#define __MATH40

#include "typedef_fx.h"

#define MAX_40 ((Word40) 549755813887.0)
#define MIN_40 ((Word40)-549755813888.0)

#define MAX_32 (Word32)0x7fffffffL
#define MIN_32 (Word32)0x80000000L

#define MAX_16 (Word16)0x7fff
#define MIN_16 (Word16)0x8000

/*_________________________________________________________________________
 |                                                                         |
 |                            Function Prototypes                          |
 |_________________________________________________________________________|
*/

/* 40-bit instructions */
/*******************************/
Word40 L_add40(Word40 L_var1, Word32 L_var2);
Word40 L_sub40(Word40 L_var1, Word32 L_var2);
Word40 L_mac40(Word40 L_var3, Word16 var1, Word16 var2); 
Word40 L_msu40(Word40 L_var3, Word16 var1, Word16 var2); 
Word40 L_shr40(Word40 L_var1, Word16 var2);				   
Word40 L_shl40(Word40 L_var1, Word16 var2);				   
Word40 L_sat32_40(Word40 L_var1);							   
Word16 norm32_l40(Word40 L_var1);						Word16 extract_l40(Word40 L_var1);
Word40 L_deposit_l40(Word40 L_var1, Word16 var_in);
   

#endif

/*============================================================================*/
/*---------------------------------- END -------------------------------------*/
/*============================================================================*/
