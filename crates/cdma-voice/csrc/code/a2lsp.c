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
/*  Memory Usage:                                                       */
/*      ROM:                4                                           */
/*      Static/Global RAM:  0                                           */
/*      Stack/Local RAM:    21                                          */
/*----------------------------------------------------------------------*/

/*
 * pctolsp - convert pc   to lsp
 *
 * NOTES: 1. This routine is hardwired for 10th order
 *        2. The routine uses 3 stage uniform grid quantization of lsp.
 */
#include "macro.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"

#define STEPSNUM 4
INT32 interpolation_cos129(INT16);
void a2lsp(INT16 *freq, INT16 *a)
{
	static INT32 LSTEPS[4] =
	{13636520, 6818260, 3409130, 1704565};

	int lspnumber;
	int root, notlast;
	INT32 Ltemp;
	INT32 Lq[11], t[7], s[6];
	INT32 Lprev[2];
	int offset;
	int iswitch;
	INT32 Lfrequency, LastFreq;
	INT32 LSTEP;
	int STEPindex;
	INT16 num;
	INT16 j;
	INT16 scale;
	scale = 5;
	LastFreq = 0;
	t[0] = 0;
	t[1] = L_shr(L_deposit_h(0x1000), scale);
	Lq[0] = t[1];

	/* calculate q[z] and p[z] , they are all stored in q */
	for (j = 1; j < 6; j++)
	{
		Lq[j] = L_add(L_shr(L_deposit_h(a[j - 1]), scale),
					  L_shr(L_deposit_h(a[10 - j]), scale));
		Lq[j] = L_sub(Lq[j], Lq[j - 1]);
	}

	Lq[5] = L_shr(Lq[5], 1);
	offset = 5;
	Lq[1 + offset] = L_add(L_shr(L_deposit_h(sub(a[0], a[9])), scale), Lq[0]);

	for (j = 2; j < 6; j++)
	{
		Lq[j + offset] = L_sub(L_shr(L_deposit_h(a[j - 1]), scale),
							   L_shr(L_deposit_h(a[10 - j]), scale));
		Lq[j + offset] = L_add(Lq[j + offset], Lq[j - 1 + offset]);
	}

	Lq[5 + offset] = L_shr(Lq[5 + offset], 1);
	Lprev[0] = LW_MAX;
	Lprev[1] = LW_MAX;
	lspnumber = 0;
	notlast = TRUE;
	iswitch = 0;
	Lfrequency = 0x00000000L;

	while (notlast)
	{

		root = TRUE;
		if (iswitch)
			offset = 5;
		else
			offset = 0;

		STEPindex = 0;			/* Start with low resolution grid */
		LSTEP = LSTEPS[STEPindex];

		while (root)
		{
			Ltemp = L_shr(interpolation_cos129(round32(Lfrequency)), 1);
			for (j = 1; j < 5; j++)
				s[j] = Ltemp;
			s[5] = L_shr(Ltemp, 1);

			for (j = 2; j < 7; j++)
			{
				t[j] = L_mpy_ll(s[j - 1], t[j - 1]);
				t[j] = L_shl(t[j], 2);
				t[j] = L_sub(t[j], t[j - 2]);
				t[j] = L_add(t[j], Lq[j - 1 + offset]);
			}

			num = (((t[6] > 0) && (Lprev[iswitch] < 0)) ||
				   ((t[6] < 0) && (Lprev[iswitch] > 0)) || (t[6] == 0));
			if ((num == 1) || (Lfrequency >= 0x40000000L))
			{

				if (STEPindex == STEPSNUM - 1)
				{

					if (L_abs(t[6]) < L_abs(Lprev[iswitch]))

						freq[lspnumber] = round32(Lfrequency);
					else
						freq[lspnumber] = round32(L_sub(Lfrequency, LSTEP));

					if ((Lprev[iswitch]) < 0)

						Lprev[iswitch] = LW_MAX;	/*was 9e9 */
					else
						Lprev[iswitch] = LW_MIN;	/*was 9e9 */

					root = FALSE;
					Lfrequency = LastFreq;
					STEPindex = 0;
				}
				else
				{

					if (STEPindex == 0)
						LastFreq = Lfrequency;
					Lfrequency =
						L_sub(Lfrequency, LSTEPS[++STEPindex]);
					/* Go back one grid step */
					LSTEP = (LSTEPS[STEPindex]);
				}
			}
			else
			{
				Lprev[iswitch] = t[6];
				Lfrequency = L_add(Lfrequency, LSTEP);

			}
		}
		lspnumber++;

		if (lspnumber > ORDER - 1)
			notlast = FALSE;
		iswitch = 1 - iswitch;
	}
}
