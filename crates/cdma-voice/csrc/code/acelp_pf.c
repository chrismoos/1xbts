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
#include "acelp_pf.h"
//#include "mathevrc.h"
#include "dsp_math.h"

#define NB_TRACK  5
#define STEP      5
#define L_SUBFR   55
#define L_SUBFR2  55

/*Note: Vector xn[], res2[], code[], y2[] and h1[] should be of lenght 55 !!! */

/*---------------------------------------------------------------------------*
 *  Function  ACELP_code()                                                   *
 *  ~~~~~~~~~~~~~~~~~~~~~~                                                   *
 *   Find Algebraic codebook.                                                *
 *--------------------------------------------------------------------------*/

void ACELP_Code(
					  Shortword xn[],	/* (i)     :Target signal for codebook.       */
					  Shortword res2[],		/* (i)     :Residual after pitch contribution */
					  Shortword h1[],	/* (i)     :Impulse response of filters.      */
					  Shortword T0,		/* (i)     :Pitch lag.                        */
					  Shortword pitch_gain,		/* (i)     :Pitch gain.                       */
					  Shortword l_subfr,	/* (i)     :Subframe lenght.                  */
					  Shortword code[],		/* (o)     :Innovative vector.                */
					  Shortword * gain_code,	/* (o)     :Innovative vector gain.           */
					  Shortword y2[],	/* (o)     :Filtered innovative vector.       */
					  Shortword * index,	/* (o)     :Index of codebook + signs         */
					  Shortword choice	/* (i)     :Choice of innovative codebook     */
			/*          0 -> 10 bits                      */
				/*          1 -> 35 bits                      */
)
{
	Shortword i, pit_sharp;
	Shortword dn[L_SUBFR2];

 /*-----------------------------------------------------------------*
  * - Find pitch sharpening.                                        *
  *-----------------------------------------------------------------*/
	/**===========================================================\
         Since the pitch gain is scaled by 1/2, therefore, the pitch_gain 
         should enlarge 2 to compensate the scale factor 
        ==============================================================*/

	pit_sharp = pitch_gain;
	/*  if(pit_sharp > 0.9) pit_sharp = 0.9;  */
	if (pit_sharp > 14746)
		pit_sharp = 14746;
	/*  if(pit_sharp < 0.2) pit_sharp = 0.2;  */
	if (pit_sharp < 3277)
		pit_sharp = 3277;
       pit_sharp = shl(pit_sharp,1);


 /*-----------------------------------------------------------------*
  * - Include fixed-gain pitch contribution into impulse resp. h1[] *
  *-----------------------------------------------------------------*/

	for (i = T0; i < l_subfr; i++)
	{							/* h[i] += pitch_sharp*h[i-T0] */
		h1[i] = add(h1[i], mult(h1[i - T0], pit_sharp));
	}

 /*-----------------------------------------------------------------*
  * Compute correlation between target "xn[]" and "h1[]".           *
  *-----------------------------------------------------------------*/

	/* Put xn[i] = res2[i] = 0; for i>l_subfr */

	for (i = l_subfr; i < L_SUBFR; i++)
		xn[i] = res2[i] = 0;

	/* Compute correlation between target x[] and H[] */

	cor_h_x(h1, xn, dn);

 /*-----------------------------------------------------------------*
  * - Innovative codebook search                                    *
  *-----------------------------------------------------------------*/

	if (choice == 0)
	{
		code_3i54_10bits(dn, l_subfr, h1, code, y2, index);
	}
	else
	{
		code_8i55_35bits(dn, res2, l_subfr, h1, code, y2, index);
	}

 /*-----------------------------------------------------------------*
  * Include fixed-gain pitch contribution into code[].              *
  *-----------------------------------------------------------------*/

	for (i = T0; i < l_subfr; i++)
	{							/* code[i] += pitch_sharp*code[i-T0] */
		code[i] = add(code[i], mult(code[i - T0], pit_sharp));
	}

 /*-----------------------------------------------------------------*
  * find gain of code.                                              *
  *-----------------------------------------------------------------*/

	*gain_code = G_code(xn, y2, l_subfr);

	return;
}

/*-------------------------------------------------------------------*
 * Function  cor_h_x()                                               *
 * ~~~~~~~~~~~~~~~~~~~                                               *
 * Compute correlation between target "x[]" and "h[]".               *
 *-------------------------------------------------------------------*
 *-------------------------------------------------------------------*/
void cor_h_x(
					   Shortword h[],	/* (i) Q12 : impulse response of weighted synthesis filter */
					   Shortword x[],	/* (i) Q0  : correlation between target and h[]            */
					   Shortword dn[]	/* (o) Q0  : correlation between target and h[]            */
)
{
	Shortword i, j, k;
	Longword s, y32[L_SUBFR], max, tot;

	/* first keep the result on 32 bits and find absolute maximum */

	tot = 5;

	for (k = 0; k < NB_TRACK; k++)
	{
		max = 0;
		for (i = k; i < L_SUBFR; i += STEP)
		{
			s = 0;
			for (j = i; j < L_SUBFR; j++)
				s = L_mac(s, x[j], h[j - i]);

			y32[i] = s;

			s = L_abs(s);

			if (L_sub(s, max) > (Longword) 0)
				max = s;
		}
		tot = L_add(tot, L_shr(max, 1));	/* tot += (2.0 x max) / 4.0 */
	}

	/* Find the number of right shifts to do on y32[] so that */
	/* 2.0 x sumation of all max of dn[] in each track not saturate. */

	j = sub(norm_l(tot), 2);	/* multiply tot by 4 */

	for (i = 0; i < L_SUBFR; i++)
	{
		dn[i] = round32(L_shl(y32[i], j));
	}
}

/*-------------------------------------------------------------------*
 * Function  G_code()                                                *
 * ~~~~~~~~~~~~~~~~~~                                                *
 * Compute gain of code.                                             *
 *-------------------------------------------------------------------*/

Shortword G_code(				
					Shortword xn2[],    /* out: Q0 Gain of innovation code. */
					Shortword y2[],		/* in:  Q10 Filtered inovation vector */
					Shortword l_subfr)	
{
	Shortword i;
	Shortword xy, yy, exp_xy, exp_yy, gain;
	Shortword scal_y2[L_SUBFR];
	Longword s;

/* Scale down Y[] by 2 to avoid overflow */

	for (i = 0; i < l_subfr; i++)
	{
		scal_y2[i] = shr(y2[i], 1);		/* Q10 --> Q9 */
	}

/* Compute scalar product <X[],Y[]> */

	s = 1;						/* Avoid case of all zeros */
	for (i = 0; i < l_subfr; i++)
		s = L_mac(s, xn2[i], scal_y2[i]);

	exp_xy = norm_l(s);
	xy = extract_h(L_shl(s, exp_xy));

/* If (xy < 0) gain = 0  */

	if (xy <= 0)
		return ((Shortword) 0);

/* Compute scalar product <Y[],Y[]> */

	s = 0;
	for (i = 0; i < l_subfr; i++)
		s = L_mac(s, scal_y2[i], scal_y2[i]);

	exp_yy = norm_l(s);
	yy = extract_h(L_shl(s, exp_yy));

/* compute gain = xy/yy */

	xy = shr(xy, 1);			/* Be sure xy < yy */
	gain = divide_s(xy, yy);

/* Denormalization of division */
	i = add(exp_xy, 5);
	i = sub(i, exp_yy);
        i=add(i,1); 

	gain = shr(gain, i);

	return (gain);
}
