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

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include "filterbank.h"


/* coefficients for low band analysis filter */
float coefs_ana_filt_lb[2][ANA_FILT_LEN_LB] = {
    {4.64243812e-003, -8.20745101e-003, 1.34441876e-002, -2.13208829e-002,
     3.41918706e-002, -5.98583629e-002, 1.48104776e-001, 4.49506086e-001,
     -8.68124575e-002, 4.43922465e-002, -2.68710133e-002, 1.69574704e-002,
     -1.05729745e-002, 6.25339997e-003},
    {6.25339997e-003, -1.05729745e-002, 1.69574704e-002, -2.68710133e-002,
     4.43922465e-002, -8.68124575e-002, 4.49506086e-001, 1.48104776e-001,
     -5.98583629e-002, 3.41918706e-002, -2.13208829e-002, 1.34441876e-002,
     -8.20745101e-003, 4.64243812e-003}
};

/* coefficients for low band synthesis filter */
float coefs_syn_filt_lb[2][SYN_FILT_LEN_LB] = {
    {-4.54575223e-003, 1.12287220e-002, -2.00599576e-002, 3.25351453e-002,
     -5.15341410e-002, 8.53696291e-002, -1.68733537e-001, 8.92598257e-001,
     3.04016299e-001, -1.28550250e-001, 7.77310154e-002, -5.18131018e-002,
     3.51649970e-002, -2.29975097e-002, 1.35456148e-002, -5.72353363e-003},
    {-5.72353363e-003, 1.35456148e-002, -2.29975097e-002, 3.51649970e-002,
     -5.18131018e-002, 7.77310154e-002, -1.28550250e-001, 3.04016299e-001,
     8.92598257e-001, -1.68733537e-001, 8.53696291e-002, -5.15341410e-002,
     3.25351453e-002, -2.00599576e-002, 1.12287220e-002, -4.54575223e-003}
};
float coefs_syn_filt_lb_a[3] = { 1.0, 0.6, 0.26 };
float coefs_syn_filt_lb_b[3] = { 0.4770, 0.9063, 0.4770 };

//freqz(0.477*[1, 1.9, 1], [1, 0.6, 0.26], 2^11, 16e3); axis([0, 8e3, -2, .5]);

/* coefficients for high band analysis filter */
float coefs_down_HB[FRAC_DN_HB][LEN_DN_HB] = {
    {3.41912907e-004, -2.69503234e-003, 1.19769577e-002, -4.56908882e-002,
     9.77711819e-001, 8.03984401e-002, -3.11043229e-002, 1.20198888e-002,
     -3.53544829e-003, 5.86167535e-004},
    {1.23211218e-003, -8.62410562e-003, 3.47366625e-002, -1.17506954e-001,
     9.01024049e-001, 2.50516846e-001, -8.47225835e-002, 3.05201954e-002,
     -8.47829304e-003, 1.32890510e-003},
    {1.81777835e-003, -1.23518612e-002, 4.80598154e-002, -1.52764025e-001,
     7.75797477e-001, 4.34194878e-001, -1.29059613e-001, 4.45406397e-002,
     -1.20398838e-002, 1.84337614e-003},
    {2.02437256e-003, -1.34769676e-002, 5.10793217e-002, -1.54547032e-001,
     6.14941672e-001, 6.14941671e-001, -1.54547031e-001, 5.10793212e-002,
     -1.34769674e-002, 2.02437252e-003},
    {1.84337617e-003, -1.20398839e-002, 4.45406402e-002, -1.29059613e-001,
     4.34194879e-001, 7.75797476e-001, -1.52764024e-001, 4.80598150e-002,
     -1.23518610e-002, 1.81777832e-003},
    {1.32890513e-003, -8.47829321e-003, 3.05201958e-002, -8.47225843e-002,
     2.50516847e-001, 9.01024048e-001, -1.17506953e-001, 3.47366621e-002,
     -8.62410545e-003, 1.23211214e-003},
    {5.86167565e-004, -3.53544844e-003, 1.20198892e-002, -3.11043236e-002,
     8.03984409e-002, 9.77711818e-001, -4.56908876e-002, 1.19769573e-002,
     -2.69503218e-003, 3.41912876e-004}
};
float coefs_ana_filt_hb_a[2] = { 1.0, 0.9 };
float coefs_ana_filt_hb_b[2] = { 0.95, 0.95 };

/* coefficients for high band synthsis filter */
float coefs_up_HB[FRAC_UP_HB][LEN_UP_HB] = {
    {1.20318669e-003, -7.63051281e-003, 2.72917685e-002, -7.50806010e-002,
     2.17114817e-001, 9.19427451e-001, -1.06860103e-001, 3.11334638e-002,
     -7.66063210e-003, 1.08509157e-003},
    {1.99103625e-003, -1.31460240e-002, 4.92989146e-002, -1.46294949e-001,
     5.37321710e-001, 6.88738481e-001, -1.57550510e-001, 5.10128599e-002,
     -1.33122905e-002, 1.98270018e-003},
    {1.67326973e-003, -1.14565524e-002, 4.49962065e-002, -1.45555950e-001,
     8.19434767e-001, 3.76310623e-001, -1.16791891e-001, 4.08360252e-002,
     -1.11251931e-002, 1.71435282e-003},
    {2.78957903e-004, -2.26822102e-003, 1.02912159e-002, -3.99823584e-002,
     9.80668152e-001, 7.05611352e-002, -2.76674071e-002, 1.07928329e-002,
     -3.20123678e-003, 5.35218462e-004}
};

//float coefs_syn_filt_hb_a1[3] = {1.0, 1.74835555397183, 0.85544957491863};
//float coefs_syn_filt_hb_b1[3] = {0.92482579255755, 1.75415354377535, 0.92482579255755};
float coefs_syn_filt_hb_a1[3] = { 1.0, 1.84755462947281, 0.97110052295510 };
float coefs_syn_filt_hb_b1[3] = { 0.9, 1.68548204358251, 0.9 };
float coefs_syn_filt_hb_a2[3] = { 1.0, 1.74219434405041, 0.85804273005855 };
float coefs_syn_filt_hb_b2[3] = { 1.0, 1.89908877043819, 1.0 };
