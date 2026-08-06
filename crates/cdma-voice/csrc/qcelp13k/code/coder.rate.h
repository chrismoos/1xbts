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
/* coder.rate.h - defines for ratedec.c */


#define PITCH_NUM 2
#define INC_FACTOR 1.03
#define LPC_ORDER_NOISE 8
#define SNR_MAP_THRESHOLD 3
#define IS96_INC  1.00547
#define VOICE_INITIAL 65
#define VOICE_INITIAL_HI 55

#define STATVOICED 5
#define SCALE_DOWN_ENERGY 0.97
#define FULL_THRESH 0

#define TLEVELS 8

#define THR_SIL_1_4  15 /* 14db below current energy is declared 1/4 rate silence */
#define THR_SIL_1_2  9 /* 9db below current energy is declared 1/2 rate silence */
#define THR_DIFFLSP 0.02 /* lsp must not change too quickly also for rate to
			    get bumped down                                 */
#define SMSNR  0.6  /* leaky integration constant used smooth snr estimate */
                    /* changed on 8-Dec-94 per Sharath's recommendation */

#define RATEM 0.02         /* bounds around avg rate that are acceptable */
#define ALPHA_STAT 0.9995   /*40second time constant   */

#define ADP 8
#define NACF_ADAP_BGN_THR  0.38 /* threshold signifying frame does            */
                               /* not have any voiced speech in it           */
                               /* so we might start to adapt thresholds      */
#define NACF_SOLID_VOICED  0.5  /* threshold above which we are pretty sure   */
                               /* speech is present and thus SNRs can be     */
                               /* adjusted accordingly                       */

#define FULLRATE_BPS 14.4
#define HALFRATE_BPS 7.2
#define QUARTER_BPS 3.6
#define HIGH_BND_SNR  25.0    /*14.0*/  /* in db */

#define LOW_BND_SNR 6.0      /* in db */
#define THR_NACF_BUMPUP 0.4   /* if nacf is lower than this and not declared*/
                             /* unvoiced then bump up rate                 */
#define THR_PRED_GAIN 5     /* in db, cc:08-23-94 */

#define HIGH_THRESH_LIM  5059644

#define DECIMATE_F 2
#define DEC_ORDER 17
#define FIR_UNV_LEN 21

static int hangover[TLEVELS] = {7,7,7,3,0,0,0,0};

/* linear discriminant coefficients for the voiced/unvoiced decision      */
/* 25-Oct-94 Weights */
/*static float dis_coef[5] = {2.795007,-0.031928,0.0,0.973946,0.0}; */

/* 08-March-95  Weights, Revision 5.17,  A5178f2.sln  */
static float dis_coef[5] = {5.190283,-0.092413,0.0,3.091836,0.0};


static float THRESH_SNR[FREQBANDS][TLEVELS][2]={
/* low band thresholds */
  { {7.0, 9.0} ,
   {7.0, 12.6} ,
   {8.0, 17.0} ,
   {8.6, 18.5} ,
   {8.9, 19.4} ,
   {9.4, 20.9} ,
   {11.0, 25.5} ,
   {15.8, 39.8} },
/* high band thresholds, these are same as above */
  { {7.0, 9.0} ,
   {7.0, 12.6} ,
   {8.0, 17.0} ,
   {8.6, 18.5} ,
   {8.9, 19.4} ,
   {9.4, 20.9} ,
   {11.0, 25.5} ,
   {15.8, 39.8} }
};

static float LOWEST_LEVEL[FREQBANDS]={10.0,5.0};


static float rate_filt[FREQBANDS][FILTERORDER]={
{   /* .2-2K bandpass filter, cut low end for noise immunity */
  -5.557699E-002,
  -7.216371E-002,
  -1.036934E-002,
   2.344730E-002,
  -6.071820E-002,
  -1.398958E-001,
  -1.225667E-002,
   2.799153E-001,
   4.375000E-001,
   2.799153E-001,
  -1.225667E-002,
  -1.398958E-001,
  -6.071820E-002,
   2.344730E-002,
  -1.036934E-002,
  -7.216371E-002,
  -5.557699E-002,
},
{
	/* 2-4k HPF */
  -1.229538E-002,
   4.376551E-002,
   1.238467E-002,
  -6.243877E-002,
  -1.244865E-002,
   1.053678E-001,
   1.248720E-002,
  -3.180645E-001,
   4.875000E-001,
  -3.180645E-001,
   1.248720E-002,
   1.053678E-001,
  -1.244865E-002,
  -6.243877E-002,
   1.238467E-002,
   4.376551E-002,
  -1.229538E-002
}
};


   static float decimate_filter[DEC_ORDER] = {
   2.725341E-003,
   1.028254E-002,
   5.973260E-003,
  -2.308975E-002,
  -5.009796E-002,
  -1.323563E-002,
   1.166278E-001,
   2.767512E-001,
   3.500000E-001,
   2.767512E-001,
   1.166278E-001,
  -1.323563E-002,
  -5.009796E-002,
  -2.308975E-002,
   5.973260E-003,
   1.028254E-002,
   2.725341E-003
};

   static float unv_filter[FIR_UNV_LEN] = {
  -1.344519E-001,
   1.735384E-002,
  -6.905826E-002,
   2.434368E-002,
  -8.210701E-002,
   3.041388E-002,
  -9.251384E-002,
   3.501983E-002,
  -9.918777E-002,
   3.749518E-002,
   8.985137E-001,
   3.749518E-002,
  -9.918777E-002,
   3.501983E-002,
  -9.251384E-002,
   3.041388E-002,
  -8.210701E-002,
   2.434368E-002,
  -6.905826E-002,
   1.735384E-002,
  -1.344519E-001

};
