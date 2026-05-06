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
#include "proto.h"
#include "defines.h"
#include "struct.h"
#include "globs.h"
extern float lsptab8[160];


short
FGV_MEM::bad_rate_check (short opt, short *cbuf)
{
    short i;
    float m0, m1, allzr = 0;
    short nsub8[2] = { 5, 5 };
    short nsize8[2] = { 16, 16 };

    //printf("In bad rate\n");
    switch (opt) {
    case 0:			//silence
	if (data_packet.WB_MODE_BIT == 1)	//WB decoding
	{
	    dec_lsp_vq_10 ((short int *) data_packet.LSP_IDX, lsp);
	}
	else
	    lspmaq_dec (ORDER, 1, 2, nsub8, nsize8, lsp,
			(short int *) data_packet.LSP_IDX, 1, lsptab8);

	if (data_packet.WB_MODE_BIT != 1)	//NB decoding
	{
	    if ((unsigned short) cbuf[0] == (unsigned short) 0xFFFF) {
		ones_dec_cnt++;
		if (ones_dec_cnt > 2) {
		    ones_dec_cnt = 3;
		    return (0);
		}
	    }
	    else
		ones_dec_cnt = 0;
	}

	for (i = 0; i < 1; i++)	//bug-fix 
	    if (cbuf[i] == 0)
		allzr++;

	if (allzr == 1)		//bug-fix
	{
	    return (1);
	}
	if (data_packet.WB_MODE_BIT != 1)	//NB decoding
	{
	    if (zrbit[0] != 0) {	//1 unused bit for bad-rate detection
		return (1);
	    }
	}

	m0 = MAX (MAX (MAX (MAX (lsp[0], lsp[1]), lsp[2]), lsp[3]), lsp[4]);
	m1 = MIN (MIN (MIN (MIN (lsp[5], lsp[6]), lsp[7]), lsp[8]), lsp[9]);
	if (m0 >= m1) {

	    return (1);
	}
	if ((LAST_PACKET_RATE_D == 4) && (prev_frame_error == 0)) {
	    return (1);
	}

	return (0);
	break;
    case 1:			//QNELP
	//printf("QNELP\n");

	if (data_packet.WB_MODE_BIT == 1)	//WB decoding
	{
	    dec_lsp_vq_28 ((short *) data_packet.LSP_IDX, lsp);

	    m0 = MAX (lsp[0], lsp[1]);
	    m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
	    if (m0 >= m1)
		return (1);

	}
	ones_dec_cnt = 0;
	for (i = 0; i < 2; i++)
	    if (cbuf[i] == 0)
		allzr++;

	if ((cbuf[2] & (0xFF00)) == 0)
	    allzr++;

	if (allzr == 3)
	    return (1);

	if (data_packet.WB_MODE_BIT != 1)	//NB decoding
	{
	    if (zrbit[0] != 0)
		return (1);
	}

	if (data_packet.NELP_FID == 3)
	    return (1);


	return (0);

	// maybe check prev frame cannot be FPPP/QPPP
	//info in LAST_PACKET_RATE_D, LAST_MODE_BIT_D
	break;
    case 2:			//QPPP
	//printf("QPPP\n");
	ones_dec_cnt = 0;
	for (i = 0; i < 2; i++)
	    if (cbuf[i] == 0)
		allzr++;

	if ((cbuf[2] & (0xFF00)) == 0)
	    allzr++;

	if (allzr == 3)
	    return (1);



	if (zrbit[0] != 0)
	    return (1);		//1 unused bit
	if ((LAST_PACKET_RATE_D == 1) || ((LAST_PACKET_RATE_D == 2) && (LAST_MODE_BIT_D == 0)) && ((prev_frame_error) == 0))	//last frame was silence or QNELP and not erased
	    return (1);


	return (0);
	break;
    case 3:			//HCELP
	//printf("HCELP\n");
	dec_lsp_vq_22 ((short int *) data_packet.LSP_IDX, lsp);
	ones_dec_cnt = 0;
	for (i = 0; i < 5; i++)
	    if (cbuf[i] == 0)
		allzr++;

	if (allzr == 5)
	    return (1);

	if (data_packet.WB_MODE_BIT == 1) {
	    if ((data_packet.DELAY_IDX != 120)
		&& (data_packet.DELAY_IDX != 115)
		&& (data_packet.DELAY_IDX != 124)
		&& (data_packet.DELAY_IDX != 125))
		/*  spl tty lag=120, do speech proc on frames following tty ( not erasure proc) */
		/*  memories in sync with encoder,  but output tty digit; 115= RESERVED LAG    */
	    {

		if (data_packet.DELAY_IDX > 120) {

		    return (1);
		}

		m0 = MAX (lsp[0], MAX (lsp[1], lsp[2]));
		m1 = MIN (lsp[6], MIN (MIN (lsp[7], lsp[8]), lsp[9]));
		if (m0 >= m1)
		    return (1);
	    }
	}
	else {			//narrowband case...update if updated in NB
	    if (1)		//temporarily including even tty/dtmf as bad packet
		/*  spl tty lag=120, do speech proc on frames following tty ( not erasure proc) */
		/*  memories in sync with encoder,  but output tty digit; 115= RESERVED LAG    */
	    {
		if (data_packet.DELAY_IDX > 100)
		    return (1);

		dec_lsp_vq_22 ((short int *) data_packet.LSP_IDX, lsp);
		m0 = MAX (lsp[0], MAX (lsp[1], lsp[2]));
		m1 = MIN (lsp[6], MIN (MIN (lsp[7], lsp[8]), lsp[9]));
		if (m0 >= m1)
		    return (1);
	    }

	}

	return (0);
	break;
    case 4:			//FPPP
	//
	//printf("FPPP\n");
	dec_lsp_vq_28 ((short *) data_packet.LSP_IDX, lsp);
	ones_dec_cnt = 0;
	for (i = 0; i < 10; i++)
	    if (cbuf[i] == 0)
		allzr++;

	if ((cbuf[10] & (0xFFE0)) == 0)
	    allzr++;
	if (allzr == 11)
	    return (1);


	if (data_packet.WB_MODE_BIT == 1) {

	    //if (SPL_HPPP == 0)
	    {
		if (data_packet.DELAY_IDX > 100)	// and not eq to special packet pattern
		    return (1);


		m0 = MAX (lsp[0], lsp[1]);
		m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
		if (m0 >= m1)
		    return (1);
		if ((LAST_PACKET_RATE_D == 1) || ((LAST_PACKET_RATE_D == 2) && (LAST_MODE_BIT_D == 0)) && ((prev_frame_error) == 0))	//last frame was silence or QNELP and not erased
		    return (1);


		if (((LAST_PACKET_RATE_D != 2) && (prev_frame_error == 0))
		    || ((LAST_PACKET_RATE_D == 2) && (fer_counter == 0))) {
		    delay = data_packet.DELAY_IDX + DMIN;
		    if (fabs ((delay - pdelayD)) > 15) {
			// zer out mems
			for (i = 0; i < ACBMemSize; i++)
			    PitchMemoryD[i] = 0;
			for (i = 0; i < 10; i++)
			    SynMemory[i] = 0;
			Init_Post_Filter ();
			return (1);
		    }
		}
	    }
	}
	else {			//narrowband case
	    if (SPL_HPPP == 0) {
		if (data_packet.delayindex > 100)	// and not eq to special packet pattern
		    return (1);


		m0 = MAX (lsp[0], lsp[1]);
		m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
		if (m0 >= m1)
		    return (1);

		if ((LAST_PACKET_RATE_D == 1) || ((LAST_PACKET_RATE_D == 2) && (LAST_MODE_BIT_D == 0)) && ((prev_frame_error) == 0))	//last frame was silence or QNELP and not erased
		    return (1);


		if (((LAST_PACKET_RATE_D != 2) && (prev_frame_error == 0))
		    || ((LAST_PACKET_RATE_D == 2) && (fer_counter == 0))) {
		    delay = data_packet.delayindex + DMIN;
		    if (fabs (delay - pdelayD) > 15) {
			// zer out mems
			for (i = 0; i < ACBMemSize; i++)
			    PitchMemoryD[i] = 0;
			for (i = 0; i < 10; i++)
			    SynMemory[i] = 0;
			Init_Post_Filter ();
			return (1);
		    }
		}
	    }

	}

	return (0);
	break;
    case 5:			//FCELP
	//printf("FCELP\n");

	dec_lsp_vq_28 ((short *) data_packet.LSP_IDX, lsp);	// do this for spl_hcelp also
	ones_dec_cnt = 0;
	for (i = 0; i < 10; i++)
	    if (cbuf[i] == 0)
		allzr++;

	if ((cbuf[10] & (0xFFE0)) == 0)
	    allzr++;
	if (allzr == 11) {
	    //printf("ALLzr is 11\n");
	    return (1);
	}

	if (data_packet.WB_MODE_BIT == 1) {

	    //if (SPL_HCELP_WB==0)//redundant check
	    {
		if (data_packet.DELAY_IDX > 120) {	// and not eq to special packet pattern
//printf("Delay is > 100\n");   
		    return (1);
		}


		m0 = MAX (lsp[0], lsp[1]);
		m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
		if (m0 >= m1) {
		    //printf("m0 is > m1\n");
		    return (1);
		}
	    }
	}
	else {			//narrowband case
	    if (SPL_HCELP == 0) {
		if (data_packet.DELAY_IDX > 100)	// and not eq to special packet pattern
		    return (1);


		m0 = MAX (lsp[0], lsp[1]);
		m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
		if (m0 >= m1)
		    return (1);
	    }

	    for (i = 0; i < NoOfSubFrames; i++) {
		long long code;

		code =
		    (((data_packet.
		       FCB_PULSE_IDX[i][3] >> 8) & 0x00000007L) << 8) +
		    (data_packet.FCB_PULSE_IDX[i][3] & 0x000000ffL);
		code =
		    (code << 8) +
		    (data_packet.FCB_PULSE_IDX[i][2] & 0x000000ffL);
		code =
		    (code << 8) +
		    (data_packet.FCB_PULSE_IDX[i][1] & 0x000000ffL);
		code =
		    (code << 8) +
		    (data_packet.FCB_PULSE_IDX[i][0] & 0x000000ffL);
		if (code > (long long) 34208719091LL)
		    return (1);
	    }

	}
	return (0);
	break;

    }
    return (0);
}
