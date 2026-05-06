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
/*  Memory Usage:                           */
/*      ROM:                            */
/*      Static/Global RAM:                      */
/*      Stack/Local RAM:                    */
/*----------------------------------------------------------------------*/
/**********************************************************************
* Routine name: durbin                                                *
* Function: Calculates LPC prediction coefficients                    *
*           from autocorrelation coefficients using                   *
*           Durbin's recursive procedure.                             *
*********************************************************************** */
#include <stdio.h>
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"
#include "mathadv.h"
#include "macro.h" 


void    durbin(
		      INT32 *R,
		      INT16 *rc,
		      INT16 *spc,
		      INT16 order
)                           
{
        register INT16 i, j, k, scale;
	INT32 pc[ORDER];
	INT32 pc_scratch[ORDER];
	INT16 shift;
        INT32 sum, Ltemp, E_long, A;
        INT32 E_min;

    A = 0; /* eliminate compiler warning */

	/* clearOverflow(); */

	if (*R <= 1)
	{

		/* if R(0)<=0, set LPC coeff. to zero */
		if (R[0] != 0)
		{
			fprintf(stderr, "Ill condition in durbin.\n");
		}

		for (i = 0; i < order; i++)
		{
			spc[i]=0;
			rc[i]=0;
		}

	}
	else
	{
		E_long = R[0]; /* maintain 32-bit normalized pred. error */
                E_min = L_shr(E_long,10); /* 2**-10 = -30dB */

		for(i=0; i < order; i++)
                {
                    if( E_long < E_min )
                    {
                        pc[i] = 0;   /* reflection coef = 0 */
                    }

                    else
                    {
                        sum = 0.0;
			scale = 1;
                        for(j = 0; j < i; j++){
                                /* sum += pc[j] * R[i-j];  */
				Ltemp = L_mpy_ll(R[i-j], pc[j]);
				Ltemp = L_shr(Ltemp,scale); /* Ltemp Q28 */
				sum = L_add(sum, Ltemp);
				if ((sum > 0x40000000) || (sum < -0x40000000))
					{
					sum = L_shr(sum,1);
					scale++;
					}
				}
			sum = L_shl(sum,3+scale);  /* back to Q28 */

                        /* pc[i] = (R[i+1] - sum) / E; */
			A = L_sub(R[i+1], sum);
		        	
			shift = norm_l(E_long);	
			if (A < 0)
				pc[i] = L_shl(L_negate(((L_divide(L_abs(A), L_shl(E_long,shift))))),shift);
			else
				pc[i] = L_shl((L_divide(A, L_shl(E_long,shift))),shift);
                    } /* end else */

                    /* pc[i] is now Q31 */

                    for(j = 0; j < i; j++)
                    {
                        /* pc_scratch[j] = pc[j] - pc[i] * pc[i-j-1]; */
                        Ltemp = L_mpy_ll(pc[i], pc[i-j-1]);  /* Ltemp Q27 */
                        pc_scratch[j] = L_sub(pc[j], Ltemp);
                    }

                    if (i != order - 1)
                    {
                        for(k = 0; k < j; k++)
                            pc[k] = pc_scratch[k];

                        /* E = (1 - pc[i]*pc[i]) * E; */
                        Ltemp = L_mpy_ll(A, pc[i]); /* Ltemp Q28 */
                        E_long = L_sub(E_long, Ltemp);
                    }  /* end if */

                    /* now convert pc[i] to Q28 */
                    pc[i] = L_add(pc[i], 0x4);   /* round before shift */
                    pc[i] = L_shr(pc[i],3); 

		/*
		if (isOverflow()){
			printf("\ndurbin: OVERFLOW for i = %d",i);
			clearOverflow();
			}

		*/

                } /*end for i */

		/* Round predictor coefficients to 16 bits (Q12) and
		   negate them for compatibility with other modules */
		spc[i-1] = negate(round32(pc[i-1]));
		for(k=0; k < order-1; k++)
			spc[k] = negate(round32(pc_scratch[k])); 

        }  /* end else */

}
