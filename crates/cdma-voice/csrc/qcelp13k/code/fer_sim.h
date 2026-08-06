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

/*============================================================================*/
/*  Lucent Network Wireless Systems EVRC Error Model (modified from Qualcomm    */
/*  CDMA frame-error model).                                                  */
/*----------------------------------------------------------------------------*/
#ifndef  _ERRMOD_H_
#define  _ERRMOD_H_

/*============================================================================*/
/*         ..Defines.                                                         */
/*----------------------------------------------------------------------------*/

/* values for array dimensioning */
#define NSTATES          2    /* number of probability transition matrices  */
#define NRATES_IN        4    /* dimensions of each transition matrix       */
#define NRATES_OUT       7

/* 5 sets of error masks required, since there are two categories for full rate,
   with errors detected by physical layer or not  */
#define NMASK_TYPES      5


/* maximum number of different error masks of each type */
/* #define NMASK_MAX      100 */
#define NMASK_MAX      256

/* bits per frame for each rate */
#define lengthFull     171
#define lengthHalf      80
#define lengthQuarter   40
#define lengthEighth    16

/* info bytes per frame in binary coding of input and output (not including rate
     byte)   */
#define NBYTES          22
#define NBLANKS        137

/* state values */
#define GOOD             0
#define BAD              1

/* event values -- codes for correct, erasure, random, and modified */
#define CORRECT          0
#define ERASURE          1
#define MODIFIED         2
#define RANDOM           3

/* rate definitions -- note that two rate codes are defined so that the
   I/O rate byte coding may be changed without affecting internal values used
   to address arrays  */

/* rate definitions used for coding of the rate byte in the encoded data  */
/* these may be changed to any desired values without affecting the operation
     of this code */
#define fullRate         4
#define halfRate         3
#define quarRate         2
#define eighRate         1
#define fullProb        15
#define erasRate        14

/* rate definitions used to address arrays (these must not be changed)  */
#define FULL             0
#define HALF             1
#define QUARTER          2
#define EIGHTH           3
#define FULL_ERRS        4       /* used only to address error masks  */

/* number of error events to be recorded in log file, approx 10-15% of longest
   input file should be sufficient to record all error events */
#define NEVENTS       2000

/* frame duration (seconds) */
#define TFRAME       20e-3

#endif
