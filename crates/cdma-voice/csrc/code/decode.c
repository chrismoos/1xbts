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
/*  EVRC Decoder -- main module                                         */
/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/
#include  <stdio.h>
#include  <string.h>
#include  <stdlib.h>

#include "mathadv.h"
//#include "mathevrc.h"
#include "dsp_math.h"
#include "mathdp31.h"

#include  "decode.h"
#include  "d_globs.h"
#include  "fer.h"
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"

//#include "tty.h"


/*======================================================================*/
/*         ..Reset RCELP decode rate.                                   */
/*----------------------------------------------------------------------*/
void SetDecodeRate(short rate)
{

	/*....execute.... */
	bit_rate = rate;

	switch (rate)
	{
	case 1:
		FCBGainSize = 16;		/*...use half-rate... */
		gnvq = gnvq_4;
		break;
	case 3:
		nsub = nsub22;
		nsize = nsize22;
		lognsize = lognsize22;
		lsptab = lsptab22;
		knum = 3;
		FCBGainSize = 16;
		gnvq = gnvq_4;
		break;
	case 4:
		nsub = nsub28;
		nsize = nsize28;
		lognsize = lognsize28;
		lsptab = lsptab28;
		knum = 4;
		FCBGainSize = 32;
		gnvq = gnvq_8;
		break;
	}
}

/*======================================================================*/
/*         ..Reset RCELP decoder parameters.                            */
/*----------------------------------------------------------------------*/
void InitDecoder()
{
	/*....(local) variables.... */
	int j;

	/*....execute.... */
	SetDecodeRate(4);
	last_valid_rate = 1;		/* reset last_valid_rate */
	last_fer_flag = 0;			/* reset last_fer_flag */
	decode_fcnt = 0;

	for (j = 0; j < ORDER; j++)
		SynMemory[j] = 0;

	OldlspD[0] = 1573;
	OldlspD[1] = 3146;
	OldlspD[2] = 4719;
	OldlspD[3] = 6291;
	OldlspD[4] = 7864;
	OldlspD[5] = 9437;
	OldlspD[6] = 11010;
	OldlspD[7] = 12583;
	OldlspD[8] = 14156;
	OldlspD[9] = 15729;

	for (j = 0; j < ACBMemSize; j++)
		PitchMemoryD[j] = PitchMemoryD_back[j] = 0;
	pdelayD = 40;
	ave_acb_gain = ave_fcb_gain = 0;
	FadeScale = 32767;
	fer_counter = 0;

	erasureFlag = 0;
	errorFlag = 0;
}

/*======================================================================*/
/*         ..Decode bitstream data.                                     */
/*----------------------------------------------------------------------*/
void decode(
			   short *codeBuf,
			   short rate,
			   short post_filter,
			   short *outFbuf
)
{
	/*....(local) variables.... */
	short local_rate;
    short acbgain_flag;
    short *oP;
    short subframesize;
    short j;


	/*....execute.... */
    acbgain_flag = 1; /* initialize to get rid of compiler warning */
	local_rate = rate;
	FrameErrorHandler(&local_rate, codeBuf);  /* check frame for some errors */
	SetDecodeRate(local_rate);

	fer_flag = erasureFlag | errorFlag;

#if 0
	if( tty_option == TTY_NO_GAIN )
    {
        if( local_rate == 4 )
        {
            /* Get pitch delay */
			/* pitch_index = (((codeBuf[1] & 0x7) << 4) | ((codeBuf[2] >> 12) & 0xf)); */
            tty_dec_header = shr((codeBuf[1] & 0x6), 1);
            tty_dec_char = shl((codeBuf[1] & 0x01), 4);
            tty_dec_char = add(tty_dec_char,(shr(codeBuf[2],12) & 0xF));
            tty_dec_baud_rate = (codeBuf[10] & 0x0020) >> 5; /* Get baud rate from bit 171 */

			/* Check for non-zero ACB gain in all three subframes */
            acbgain_flag = 0;
			acbgain_flag |= (codeBuf[2] & 0x0070);
			acbgain_flag |= (codeBuf[5] & 0x0e00);
			acbgain_flag |= (codeBuf[7] & 0x0001);
			acbgain_flag |= (codeBuf[8] & 0xc000);
		}
        else if( local_rate == 3 )
        {
            /* Get pitch delay */
			/* pitch_index = ((codeBuf[1] & 0x3f8) >> 3); */
            tty_dec_header = shr((codeBuf[1] & 0x300), 8);
            tty_dec_char = shr((codeBuf[1] & 0xF8), 3);

			/* Check for non-zero ACB gain in all three subframes */
            acbgain_flag = 0;
			acbgain_flag |= (codeBuf[1] & 0x0007);
			acbgain_flag |= (codeBuf[2] & 0x0003);
			acbgain_flag |= (codeBuf[3] & 0x8001);
			acbgain_flag |= (codeBuf[4] & 0xc000);
		}
        else
        {
            tty_dec_header = 0;
            tty_dec_char = 0;
            acbgain_flag = 1;
        }
		acbgain_flag = abs(acbgain_flag);

		oP = outFbuf;

		/* TTY subframe loop */
        for( j=0 ; j < NoOfSubFrames ; j++)  /* loop over all subframes */
        {
			if (j < 2)
            {
				subframesize = SubFrameSize - 1;
			}
            else
            {
                subframesize = SubFrameSize;
            }

            tty_dec_flag = tty_dec( oP,
				        acbgain_flag,
				        tty_dec_header,
				        tty_dec_char,
				        tty_dec_baud_rate,
                        fer_flag,
				        j,
				        NoOfSubFrames,
				        subframesize);
             
			oP += subframesize;
        } /* end for(j) */

        if( tty_dec_flag != 0 )
        {
            InitDecoder();
            return;
        }

	} /* end if( tty_option == TTY_NO_GAIN ) */
#endif


	if (!fer_flag){
		decode_no_fer(codeBuf, post_filter, outFbuf); /* no FER */
		fer_flag = erasureFlag | errorFlag;
		}

	if (fer_flag)
		decode_fer(post_filter, outFbuf); /* FER has occured */
}
