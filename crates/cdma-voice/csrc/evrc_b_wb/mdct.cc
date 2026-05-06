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
static char const rcsid[] =
    "$Id: mdct.cc,v 1.2.6.4 2007/06/24 06:43:14 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/* Fast MDCT algorithm from:

   H.S. Malvar, "Lapped transforms for efficient transform/subband coding",
   Acoustics, Speech, and Signal Processing, IEEE Transactions on
   Volume 38, Issue 6, June 1990 Page(s):969 - 978 

   P. Yip and K.R. Rao, "Fast decimation-in-time algorithms for a
   family of discrete sine and cosine transforms." Circuits, Systems, Signal
   Processing. vol. 3, pp. 387-408, 1984.

*/

#include <math.h>
#include <stdio.h>
#include <stdlib.h>
#include <assert.h>

#include "defines.h"

#include "mdct.h"

#define PI 3.1415926535897931
#define pi PI

#define SIN_N 1280
static float sin_table[SIN_N / 4];	// put in class
static float sin_table_5x5[5][5];	// PUT IN CLASS OR ROM.CC


void
init_sin_tables ()		// put in class init routine
{
    static int firsttime = 1;
    int m, i, j;

    if (firsttime == 1) {
	firsttime = 0;
	for (m = 0; m < SIN_N / 4; m++) {
	    sin_table[m] = sin (2.0 * PI * m / SIN_N);
	}
	for (i = 0; i < 5; i++) {
	    for (j = 0; j < 5; j++) {
		sin_table_5x5[i][j] = sin ((j + 0.5) * (i + 0.5) * pi / 5);
	    }
	}
    }
}



#define OP_COUNT(X)
#define RESET_OP_COUNT
#define PRINT_OP_COUNT(X)

void
def_dst4 (float *x, float *y)
{
    int i, j;
    double tmp;

    for (i = 0; i < 5; i++) {
	tmp = x[0] * sin_table_5x5[i][0];
	for (j = 1; j < 5; j++) {
	    tmp += x[j] * sin_table_5x5[i][j];
	}
	y[i] = tmp;
	OP_COUNT (5 + 3);
    }


}

void
dit_dst4 (float *x, float *y, int MM)
{
    int i;
    float *y1 = new float[MM], *y2 = new float[MM], *y3 = new float[MM], *y4 =
	new float[MM];
    for (i = 0; i < MM / 2; i += 2) {
	y1[i] = x[2 * i] + x[2 * i + 1];
	y2[i] = x[2 * i] - x[2 * i + 1];
	y1[i + 1] = x[2 * (i + 1)] + x[2 * (i + 1) + 1];
	y2[i + 1] = -x[2 * (i + 1)] + x[2 * (i + 1) + 1];
	OP_COUNT (4 + 4);
    }
    OP_COUNT (1);
    if (MM == 10) {
	def_dst4 (y1, y3);
	OP_COUNT (4);
	def_dst4 (y2, y4);
	OP_COUNT (4);
    }
    else {
	dit_dst4 (y1, y3, MM / 2);
	OP_COUNT (5);
	dit_dst4 (y2, y4, MM / 2);
	OP_COUNT (5);
    }


    int is = (SIN_N / 8) / MM;	// sine part
    int ic = SIN_N / 4 - is;	// cosine part
    int id = 2 * is;		// step size: ((SIN_COS_N/8)*3/M) - is;

    OP_COUNT (6);
    for (i = 0; i < MM / 2; i++, ic -= id, is += id) {
	y[i] = y3[i] * sin_table[ic] - y4[MM / 2 - i - 1] * sin_table[is];
	OP_COUNT (3);
    }

    is = (SIN_N * (1 + MM) / (8 * MM));	// sine part
    ic = SIN_N / 4 - is;	// cosine part
    OP_COUNT (4);
    for (i = 0; i < MM / 2; i++, ic -= id, is += id) {
	y[i + MM / 2] =
	    y3[MM / 2 - i - 1] * sin_table[ic] + y4[i] * sin_table[is];
	OP_COUNT (3);
    }

    delete[]y1;
    delete[]y2;
    delete[]y3;
    delete[]y4;
}


void
mdct_dst4 (float *x, float *y, int MM)
{
    int i;
    float *y1 = new float[MM / 2];

    for (i = 0; i < MM / 4; i++) {
	y1[i] = x[i + MM / 4] - x[MM / 4 - i - 1];
	OP_COUNT (2);
    }
    for (i = MM / 4; i < MM / 2; i++) {
	y1[i] = x[i + MM / 4] + x[5 * MM / 4 - i - 1];
	OP_COUNT (2);
    }
    dit_dst4 (y1, y, MM / 2);
    for (i = 0; i < MM / 2; i += 2) {
	y[i] = -y[i];
	OP_COUNT (2);
    }

    delete[]y1;
}


void
imdct_dst4 (float *x, float *y, int MM)
{
    int i;
    float *x1 = new float[MM], *y1 = new float[MM];

    for (i = 0; i < MM / 2; i += 2) {
	x1[i] = -x[i];
	OP_COUNT (2);
    }
    for (i = 1; i < MM / 2; i += 2) {
	x1[i] = x[i];
	OP_COUNT (2);
    }
    dit_dst4 (x1, y1, MM / 2);

    for (i = 0; i < MM / 4; i++) {
	y[i + MM / 4] = y1[i];
	OP_COUNT (2);
	y[MM / 4 - i - 1] = -y1[i];
	OP_COUNT (2);
    }
    for (i = MM / 4; i < MM / 2; i++) {
	y[i + MM / 4] = y1[i];
	OP_COUNT (2);
	y[5 * MM / 4 - i - 1] = y1[i];
	OP_COUNT (2);
    }
    delete[]x1;
    delete[]y1;
}




void
mdct (float *x, float *y, short winflag)
{
    static int firsttime = 1;
    float tmp_buf[320];

    int j;

    if (firsttime == 1) {
	firsttime = 0;
	init_sin_tables ();	// put in class init routine   
    }

    RESET_OP_COUNT;

    /*copy the input signal into a buffer that has 40 leading zeros and 40 trailing zeros */

    for (j = 0; j < 40; j++) {
	tmp_buf[j] = 0.0;
	OP_COUNT (2);
    }

    if (winflag == 0) {
	for (j = 0; j < 80; j++) {
	    tmp_buf[j + 40] = sin_table[2 + 4 * j] * x[j];
	    OP_COUNT (2);
	}
	for (j = 80; j < 160; j++) {
	    tmp_buf[j + 40] = x[j];
	    OP_COUNT (1);
	}
	for (j = 160; j < 240; j++) {
	    tmp_buf[j + 40] = sin_table[320 - 2 - 4 * (j - 160)] * x[j];
	    OP_COUNT (2);
	}
    }
    else {
	for (j = 0; j < 160; j++) {
	    tmp_buf[j + 40] = x[j];
	    OP_COUNT (1);
	}
	for (j = 160; j < 240; j++) {
	    tmp_buf[j + 40] = sin_table[320 - 2 - 4 * (j - 160)] * x[j];
	    OP_COUNT (2);
	}
    }

    for (j = 0; j < 40; j++) {
	tmp_buf[j + 280] = 0.0;
	OP_COUNT (1);
    }



    mdct_dst4 (tmp_buf, y, 320);

    for (j = 0; j < 160; j++) {
	y[j] *= 2.0 * 0.05590169943749;
	OP_COUNT (2);
    }				// 2/sqrt(320) 

    PRINT_OP_COUNT ("MDCT");

}


void
imdct (float *x, float *y, short winflag)
{
    static int firsttime = 1;

    int j;

    if (firsttime == 1) {
	firsttime = 0;
	init_sin_tables ();	// put in class init routine
    }

    RESET_OP_COUNT;

    imdct_dst4 (x, y, 320);

    for (j = 0; j < 320; j++) {
	y[j] *= 2.0 * 0.05590169943749;
	OP_COUNT (2);
    }				// 1/sqrt(320)

    /*copy the input signal into a buffer that has 40 leading zeros and 40 trailing zeros */
    for (j = 0; j < 40; j++) {
	y[j] = 0.0;
	OP_COUNT (1);
    }

    if (winflag == 0) {
	for (j = 0; j < 80; j++) {
	    y[j + 40] *= sin_table[2 + 4 * j];
	    OP_COUNT (2);
	}
	for (j = 160; j < 240; j++) {
	    y[j + 40] *= sin_table[320 - 2 - 4 * (j - 160)];
	    OP_COUNT (2);
	}
    }
    else {
	for (j = 160; j < 240; j++) {
	    y[j + 40] *= sin_table[320 - 2 - 4 * (j - 160)];
	    OP_COUNT (2);
	}
    }
    for (j = 0; j < 40; j++) {
	y[j + 280] = 0.0;
	OP_COUNT (1);
    }

    PRINT_OP_COUNT ("IMDCT");
}
