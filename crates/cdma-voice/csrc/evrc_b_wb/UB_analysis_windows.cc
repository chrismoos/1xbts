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

float win[154] =
    { 0.056070447, 0.167506223, 0.276835511, 0.382683432, 0.483718887,
0.578671296, 0.666346578,
    0.745642165, 0.815560869, 0.875223422, 0.923879533, 0.960917322,
	0.985871019, 0.998426815,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    1.000000000, 1.000000000, 1.000000000, 1.000000000, 1.000000000,
	1.000000000, 1.000000000,
    0.998426815, 0.985871019, 0.960917322, 0.923879533, 0.875223422,
	0.815560869, 0.745642165,
    0.666346578, 0.578671296, 0.483718887, 0.382683432, 0.276835511,
	0.167506223, 0.056070447
};

float win_lpc[154] =
    { 0.000411, 0.001642, 0.003693, 0.006559, 0.010235, 0.014716, 0.019995,
0.026062, 0.032908, 0.040521, 0.048889, 0.057999,
    0.067834, 0.078380, 0.089618, 0.101531, 0.114098, 0.127300, 0.141113,
	0.155517, 0.170486, 0.185997, 0.202023, 0.218539, 0.235518, 0.252931,
	0.270750,
    0.288946, 0.307489, 0.326347, 0.345492, 0.364889, 0.384509, 0.404319,
	0.424286, 0.444377, 0.464560, 0.484801, 0.505067, 0.525325, 0.545541,
	0.565682,
    0.585715, 0.605607, 0.625326, 0.644839, 0.664114, 0.683120, 0.701824,
	0.720197, 0.738208, 0.755828, 0.773027, 0.789778, 0.806053, 0.821825,
	0.837068,
    0.851757, 0.865869, 0.879379, 0.892266, 0.904508, 0.916086, 0.926981,
	0.937173, 0.946648, 0.955388, 0.963381, 0.970612, 0.977070, 0.982744,
	0.987625,
    0.991704, 0.994976, 0.997435, 0.999076, 0.999897, 0.999897, 0.999076,
	0.997435, 0.994976, 0.991704, 0.987625, 0.982744, 0.977070, 0.970612,
	0.963381,
    0.955388, 0.946648, 0.937173, 0.926981, 0.916086, 0.904508, 0.892266,
	0.879379, 0.865869, 0.851757, 0.837068, 0.821825, 0.806053, 0.789778,
	0.773027,
    0.755828, 0.738208, 0.720197, 0.701824, 0.683120, 0.664114, 0.644839,
	0.625326, 0.605607, 0.585715, 0.565682, 0.545541, 0.525325, 0.505067,
	0.484801,
    0.464560, 0.444377, 0.424286, 0.404319, 0.384509, 0.364889, 0.345492,
	0.326347, 0.307489, 0.288946, 0.270750, 0.252931, 0.235518, 0.218539,
	0.202023,
    0.185997, 0.170486, 0.155517, 0.141113, 0.127300, 0.114098, 0.101531,
	0.089618, 0.078380, 0.067834, 0.057999, 0.048889, 0.040521, 0.032908,
	0.026062,
    0.019995, 0.014716, 0.010235, 0.006559, 0.003693, 0.001642, 0.000411
};

float subwin[42] =
    { 0.056070, 0.167506, 0.276836, 0.382683, 0.483719, 0.578671, 0.666347,
0.745642, 0.815561, 0.875223, 0.923880, 0.960917, 0.985871,
    0.998427, 1.000000, 1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
	1.000000, 1.000000, 1.000000, 1.000000, 1.000000, 1.000000, 1.000000,
	1.000000,
    0.998427, 0.985871, 0.960917, 0.923880, 0.875223, 0.815561, 0.745642,
	0.666347, 0.578671, 0.483719, 0.382683, 0.276836, 0.167506, 0.056070
};
