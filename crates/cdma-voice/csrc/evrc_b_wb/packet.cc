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
    "$Id: packet.cc,v 1.13.12.2 2007/06/24 06:43:16 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     Bitpack                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

#include "macro.h"
#include "struct.h"

#define       MAX_RATES       25

void
FGV_MEM::clearBitTotal ()
{
    total_bits_packed = 0;
}

void
FGV_MEM::printBitTotal ()
{
    int i;
    int flag = 1;

    for (i = 0; i < bit_rates; i++) {
	if (total_bits_packed == bit_rate_totals[i]) {
	    flag = 0;
	    break;
	}
    }

    if (flag) {
	/* new bit rate */
	bit_rate_totals[bit_rates] = total_bits_packed;
	bit_rates++;
	fprintf (stderr, "Packet Size = %3d, Bit Rate = %5d bps\n",
		 total_bits_packed, 50 * total_bits_packed);
    }
}

/************************************************
* Routine name: Bitpack                         *
* Function: pack input data into bitstream.     *
* Inputs:                                       *
*    in - input data.                           *
*    TrWords - pointer to transmit words memory.*
*    NoOfBits - number of bits in input data.   *
*    ptr - bit and word pointers.               *
*                                               *
************************************************/

void
FGV_MEM::Bitpack (short in, unsigned short *TrWords, short NoOfBits,
		  short *ptr)
{
    short temp;
    unsigned short *WordPtr;

    total_bits_packed += NoOfBits;
    if (total_bits_packed > PACKWDSNUM * 16) {
	fprintf (stderr,
		 "\n\nERROR: %s line %d, %s()\nToo many bits per packet, try increasing BITSTREAM_BUFFER_LEN in macro.h\n",
		 __FILE__, __LINE__, __FUNCTION__);
	fprintf (stderr, "Max bits currently allowed: %d (%d bps)\n\n",
		 PACKWDSNUM * 16, PACKWDSNUM * 16 * 50);
	exit (1);
    }

    WordPtr = TrWords + ptr[1];

    *ptr -= NoOfBits;
    if (*ptr >= 0) {
	*WordPtr = *WordPtr | (in << *ptr);
    }
    else {
	temp = (unsigned short) in >> (-*ptr);	//BUG FIX 
	*WordPtr = *WordPtr | temp;
	WordPtr++;
	*ptr = 16 + *ptr;
	*WordPtr = (short) ((long) ((long) in << *ptr) & 0xffff);
    }
    ptr[1] = (short) (WordPtr - TrWords);
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:    bitupack.c                                               */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     1/01/95  Born                                                    */
/*----------------------------------------------------------------------*/

/***********************************************
* Routine name: BitUnpack                      *
* Function: pack input data into bitstream.    *
* Inputs:                                      *
*    RecWords - Location of receive words.     *
*    NoOfBits - number of bits for output data.*
*    ptr - bit and word pointers.              *
* Output:                                      *
*    out - output bits.                        *
*                                              *
* Written by: Dror Nahumi.                     *
***********************************************/

void
FGV_MEM::BitUnpack (short *out, unsigned short *RecWords, short NoOfBits,
		    short *ptr)
{
    unsigned short *WordPtr;
    long temp;

    WordPtr = RecWords + ptr[1];

    *ptr -= NoOfBits;
    if (*ptr >= 0) {
	temp = (long) (*WordPtr) << NoOfBits;
    }
    else {
	temp = (long) (*WordPtr) << (NoOfBits + *ptr);
	WordPtr++;
	temp = (temp << (-*ptr)) | ((long) *WordPtr << (-*ptr));
	*ptr = 16 + *ptr;
    }

    *WordPtr = (short) (temp & 0xffff);
    *out = (short) ((long) (temp & 0xffff0000) >> 16);

    ptr[1] = (short) (WordPtr - RecWords);
}
