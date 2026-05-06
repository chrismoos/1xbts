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
/*  Module:     bqiir.c                                                 */
/*----------------------------------------------------------------------*/
/*         ..Complexity (not including main() buffers).                 */
/*           total data ROM        48 bytes                             */
/*           total static RAM     144 bytes                             */
/*           total dynamic RAM   1516 bytes                             */
/*----------------------------------------------------------------------*/
/*======================================================================*/
/*         ..Include files.                                             */
/*----------------------------------------------------------------------*/
#include  <stdio.h>
//#include  <math.h>
//#include  "mathevrc.h"
#include "dsp_math.h"
#include  "mathdp31.h"
#include  "mathadv.h"
#include  "evrc_state.h"

/*======================================================================*/
/*        ..Defines.                                                    */
/*             data ROM       12 bytes                                  */
/*----------------------------------------------------------------------*/
#define  BQ_N_DATA     160
#define  BQ_N_FILTERS    3
#define  BQ_N_ORDER      2
#define  BQ_N_SAVE       (BQ_N_FILTERS * BQ_N_ORDER)
#define  BQ_N_BUFFER     (BQ_N_SAVE + BQ_N_DATA)
#define  BQ_N_W          (2 * BQ_N_FILTERS * (BQ_N_ORDER + 1))

/*======================================================================*/
/*        ..Globals.                                                    */
/*             data ROM       36 bytes                                  */
/*----------------------------------------------------------------------*/
static const Shortword  bq_w[BQ_N_W] = {
/*    n2     n1    n0     d2     d1     d0   */
	8193, -16385, 8192,  7802, -15923, 8192,
	8191, -16383, 8192,  7170, -15294, 8192,
	6828, -13656, 6828,  6827, -14952, 8192
};

/*======================================================================*/
/*        ..Fixed_point vector copy functions.                          */
/*----------------------------------------------------------------------*/

void V_copy_SS (Shortword* a,Shortword* b,Shortword n)
{
	/*...execute....*/
	while (n--) *b++ = *a++;
}
void  V_copy_LL (Longword* a,Longword* b,Shortword n)
{
	/*....execute....*/
	while (n--)  *b++ = *a++;
}
void  V_copy_SL (Shortword* a,Longword* b,Shortword n)
{
	/*....execute....*/
	while (n--)  *b++ = L_shl((Longword)(*a++),12);
}
void  V_copy_LS (Longword* a,Shortword* b,Shortword n)
{
	/*....execute....*/
	while (n--)  *b++ = round32(L_shl(*a++,4));
}

/*======================================================================*/
/*        ..Three-stage bi-quad hi-pass IIR filter.                     */
/*          This function processes 160 point (20 ms) blocks).          */
/*             static RAM     144 bytes                                 */
/*             dynamic RAM   1516 bytes                                 */
/*----------------------------------------------------------------------*/
void  bqiir (Shortword* buf)
{
	Shortword  *bq_xsave = evrc_current_context()->encoder.bqiir_bq_xsave;
	Longword  *bq_ysave = evrc_current_context()->encoder.bqiir_bq_ysave;

	/*....(local) variables....*/
	register Shortword   i;
	register Shortword   j;
	register Shortword   k;

	register Shortword*  wP;
	register Shortword*  wpP;

	register Longword    sum;
	register Longword    tmp_sum;

	register Shortword*  tmpPx;
	register Longword*   tmpPy;
	register Shortword*   xP;
	register Shortword*   xpP;
	register Shortword*   xsP;
	register Longword*   yP;
	register Longword*   ypP;
	register Longword*   ysP;

	Shortword  bq_x[BQ_N_BUFFER];
	Longword  bq_y[BQ_N_BUFFER];

	/*....execute....*/
	V_copy_SS(buf,bq_x+BQ_N_SAVE,BQ_N_DATA);

	xsP = bq_xsave;
	ysP = bq_ysave;
	xP = bq_x;
	yP = bq_y + BQ_N_ORDER;
	wP = bq_w;
	for (i=BQ_N_FILTERS-1; i>=0; i--) {
		V_copy_SS(xsP,bq_x,BQ_N_SAVE);
		V_copy_LL(ysP,bq_y,BQ_N_SAVE);
	
		xpP = xP;
		ypP = yP;
		for (j=0; j<BQ_N_DATA+i*BQ_N_ORDER; j++) {
			sum = 0;
			wpP = wP;
			tmpPx = xpP++;
			for (k=0; k<BQ_N_ORDER+1; k++) {
				sum = L_mac(sum, *tmpPx++, *wpP++);
			}
			sum = L_shr(sum,4);
			tmpPy = ypP - BQ_N_ORDER;
			for (k=0; k<BQ_N_ORDER; k++) {
				tmp_sum = L_mpy_ls(*tmpPy++,*wpP++);
				sum = L_sub(sum, tmp_sum);
			}
			*ypP++ = (L_shl(sum, 2));
		}
		V_copy_SS(bq_x+BQ_N_BUFFER-BQ_N_SAVE,xsP,BQ_N_SAVE);
		V_copy_LL(bq_y+BQ_N_BUFFER-BQ_N_SAVE,ysP,BQ_N_SAVE);
		xsP += BQ_N_SAVE;
		ysP += BQ_N_SAVE;
		xP += BQ_N_ORDER;
		yP += BQ_N_ORDER;
		wP += (2*(BQ_N_ORDER+1));

		V_copy_LS(bq_y,bq_x,BQ_N_BUFFER);
	}
	V_copy_LS(bq_y+BQ_N_SAVE,buf,BQ_N_DATA);
}
