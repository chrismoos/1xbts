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
/*_____________________
 |                     |
 | Basic types.        |
 |_____________________|
*/

#ifndef __TYPEDEF_FX
 #define __TYPEDEF_FX

#define LW_SIGN (long)0x80000000       /* sign bit */
#define LW_MIN (long)0x80000000
#define LW_MAX (long)0x7fffffff

#define SW_SIGN (short)0x8000          /* sign bit for Word16 type */
#define SW_MIN (short)0x8000           /* smallest Ram */
#define SW_MAX (short)0x7fff           /* largest Ram */

/* Definition of Types *
 ***********************/

#if defined(__BORLANDC__) || defined (__WATCOMC__) || defined(_MSC_VER) || defined(__ZTC__) || defined(__HIGHC__) || defined (__CYGWIN32__)
typedef short int Word16;           /* 16 bit "register"  (sw*) */
typedef unsigned short int UNS_Word16;  /* 16 bit "register"  (sw*) */
typedef short int Word16Rom;        /* 16 bit ROM data    (sr*) */
typedef long int Word32;             /* 32 bit "accumulator" (L_*) */
typedef long int Word32Rom;          /* 32 bit ROM data    (L_r*)  */
typedef double Word40;		     /* 40 bit accumulator        */
#elif defined( __sun)
typedef short Word16;           /* 16 bit "register"  (sw*) */
typedef unsigned short UNS_Word16;  /* 16 bit "register"  (sw*) */
typedef short Word16Rom;        /* 16 bit ROM data    (sr*) */
typedef long Word32;             /* 32 bit "accumulator" (L_*) */
typedef long Word32Rom;          /* 32 bit ROM data    (L_r*)  */
typedef double Word40;				   /* 40 bit accumulator        */
#elif defined(__unix__) || defined(__unix)
typedef short Word16;           /* 16 bit "register"  (sw*) */
typedef unsigned short UNS_Word16;  /* 16 bit "register"  (sw*) */
typedef short Word16Rom;        /* 16 bit ROM data    (sr*) */
typedef int Word32;             /* 32 bit "accumulator" (L_*) */
typedef int Word32Rom;          /* 32 bit ROM data    (L_r*)  */
typedef double Word40;				   /* 40 bit accumulator        */
#elif defined(VMS) || defined(__VMS)
typedef short Word16;           /* 16 bit "register"  (sw*) */
typedef unsigned short UNS_Word16;  /* 16 bit "register"  (sw*) */
typedef short Word16Rom;        /* 16 bit ROM data    (sr*) */
typedef long Word32;             /* 32 bit "accumulator" (L_*) */
typedef long Word32Rom;          /* 32 bit ROM data    (L_r*)  */
typedef double Word40;				   /* 40 bit accumulator        */
#else
#error  COMPILER NOT TESTED typedef.h needs to be updated, see readme
#endif

/* global variables */
/********************/

//extern int giFrmCnt;                   /* 0,1,2,3,4..... */
//extern int giSfrmCnt;                  /* 0,1,2,3 */

//extern int giDTXon;                    /* DTX Mode on/off */

extern int giOverflow;
extern int giOldOverflow;

#endif
