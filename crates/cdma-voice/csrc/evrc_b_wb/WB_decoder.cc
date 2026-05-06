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
/*  4GV - Fourth Generation Vocoder Speech Service Option for           */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include "struct.h"
#include "WB_analysis_windows_8thRate.h"
void
FGV_MEM::WB_decoder (float time_warp_fraction)
{
    int k;


    if (WB_decoder_first_time == 1) {
	filterbank_init_syn_lb (&S_syn_lb);
	filterbank_init_syn_hb (&S_syn_hb);
	WB_decoder_first_time = 0;
    }

    //***************************************************
    //BEGIN : Some Frame level initializations 
    //***************************************************
    BAD_RATE = 0;

    //WB HR detector
    data_packet.WB_MODE_BIT = 0;	//default is NB decoding
    short rate_bck;

    rate_bck = rate;


    if (((buf16[10] & 0x0020) == 0x20) && (rate == 4))
	//implies the 171st bit is set => EVRC-WB mode
    {
	if ((buf16[10] & 0x0040) == 0x40) {
	    //implies the 170th bit is set => MDCT mode
	    data_packet.Celp_Mdct_Flag = 1;	//MDCT
	    if ((buf16[10] & 0x0080) == 0x80) {
		//implies 169 th bit is set => WB-MDCT mode
		data_packet.WB_MODE_BIT = 1;

		MDCT_NPULSES = 23;	/* Used for WB Coding */
		MDCT_NWORDS = 8;	/* 23 pulses on 144 position = 114 bits => 7*15 bits + 9 bits */
		MDCT_REMAINDER = 9;
	    }
	    else {
		//implies 169 th bit is not set => NB-MDCT mode
		data_packet.WB_MODE_BIT = 0;

		// use this flag to distinguish between 23/28 pulse MDCT
		MDCT_NPULSES = 28;	/* Used for NB Coding */
		MDCT_NWORDS = 9;	/* 28 pulses on 144 position = 131 bits => 8*15 bits + 11 bits */
		MDCT_REMAINDER = 11;

	    }
	}
	else {
	    //implies the 170th bit is not set => CELP mode
	    data_packet.Celp_Mdct_Flag = 0;	//CELP
	    data_packet.WB_MODE_BIT = 1;

	}
    }
    else {
	//implies the 171st bit is not set => EVRC-B mode
	data_packet.WB_MODE_BIT = 0;
	data_packet.Celp_Mdct_Flag = 0;	//EVRC-B does not support MDCT mode
    }




    //printf("%d %d\n",data_packet.Celp_Mdct_Flag,decode_fcnt);







    if ((rate == 2) && (WB_COUNT > 10) && (rate_last != 2)) {
	data_packet.WB_MODE_BIT = 1;
	rate = 0xE;
    }

    //the idea here is that if we are in the WB decoding mode then rate=2 is not possible and hence bad-rate
    //however since rate=2 is possible when a switch to NB mode happens , we erase only the 1st rate =2 frame
       /********************* FIGURE OUT IF INCOMING PACKET IS WB/NB AND SET THE WB_MODE_BIT ************/
    if ((((buf16[0] & 0xFC00) >> 10) == 0x3F) && (rate == 3)) {
	rate = 2;
	data_packet.WB_MODE_BIT = 1;
    }				//WB_HR

    if (((buf16[0] & 0x1) == 1) && (rate == 1))
	data_packet.WB_MODE_BIT = 1;

    /* WB_MODE_BIT HANDLING FOR ERASURES */

    if ((last_wb_mode_bit == 1) && (rate == 0xE))
	data_packet.WB_MODE_BIT = 1;
	/***********************************************************************************************/


    if (rate == 0)
	data_packet.WB_MODE_BIT = last_wb_mode_bit;	//Added by Jagadeesh 08/21/06

    //if D&B signalling created a HCELP frame then set WB_MODE_BIT to 1 and DO NOT erase

    if ((rate == 3) && (last_wb_mode_bit != 0))
	data_packet.WB_MODE_BIT = 1;


    if (data_packet.WB_MODE_BIT == 1)
	args->operating_point = 3;
    else
	args->operating_point = 0;


    if ((data_packet.WB_MODE_BIT == 0) && (WB_COUNT > 10)
	&& (last_wb_mode_bit != 0) && (rate != 3)) {
	data_packet.WB_MODE_BIT = 1;
	rate = 0xE;
    }


    if ((data_packet.WB_MODE_BIT == 1) && (NB_COUNT > 10)
	&& (last_wb_mode_bit == 0)) {
	data_packet.WB_MODE_BIT = 0;
    }



    if ((last_wb_mode_bit == 0) && (rate == 3))
	initD = 0;		// ensure that for the current HCELP the upper band erasure proc. starts off with
    //initialized parameters IF the previous frame was NB


    rate_last = rate_bck;
    last_wb_mode_bit = data_packet.WB_MODE_BIT;


    data_packet.PACKET_RATE = rate;


    //***************************************************
    //END : Some Frame level initializations 
    //***************************************************
    FrameErrorHandler (buf16);

    int moto_downgrading = 0;	// this is to indicate whether 0xE is "downgraded" to 0 in the following code.

    if (args->dtx) {
	if (data_packet.PACKET_RATE == 0xE
	    && ((lastrateD == 1) || bbg_dtx_likely_flag)) {
	    data_packet.PACKET_RATE = 0;
	    moto_downgrading = 1;
	}
    }




    // run the following conversion only when the above downgrading is NOT executed. 
    //  The purpose of useing mot_downgrading flag is to guarantee the bit-exactness.
    if (args->dtx && !moto_downgrading) {
	rate = data_packet.PACKET_RATE;
	// adjust the DTX(rate 0) and Erasure(rate 0xE), according to lastrateD
	int DTX_STATE = 0;

	if ((rate == 4) || (rate == 3) || (rate == 2))
	    DTX_STATE = 0;
	else if (rate == 1)
	    DTX_STATE = 1;

	if ((rate == 0) || (rate == 0xE)) {
	    if ((lastrateD == 4) || (lastrateD == 3) || (lastrateD == 2))
		rate = 0xE;
	    else if ((lastrateD == 1) || (lastrateD == 0))
		rate = 0;
	    else {
		if (DTX_STATE == 1)
		    rate = 0;
		else
		    rate = 0xE;
	    }
	}
	data_packet.PACKET_RATE = rate;
	PACKET_RATE = data_packet.PACKET_RATE;
    }


    for (k = 0; k < 2; k++)
	this->FrameRateMemory[k] = this->FrameRateMemory[k + 1];
    this->FrameRateMemory[2] = data_packet.PACKET_RATE;

    short tmpbuf[PACKWDSNUM];

    for (k = 0; k < PACKWDSNUM; k++)
	tmpbuf[k] = buf16[k];

    PREV_LSP_FLAG = 0;
    //Time-Warping: Call to decode changed below
    decode (buf16, data_packet.PACKET_RATE, args->post_filter, buf_out,
	    time_warp_fraction, &obuf_len);
    decode_fcnt++;		//moved outside decode




    if (data_packet.WB_MODE_BIT == 1) {


	if (data_packet.PACKET_RATE == 1) {
	}
	else if (data_packet.PACKET_RATE == 2) {
	    BitUnpack ((short *) &dec_indices.indices_LSFs[0],
		       (unsigned short *) RxPkt, 8, PktPtr);
	    BitUnpack ((short *) &dec_indices.indices_LSFs[1],
		       (unsigned short *) RxPkt, 4, PktPtr);
	    BitUnpack ((short *) &dec_indices.indices_gains[0],
		       (unsigned short *) RxPkt, 4, PktPtr);
	    BitUnpack ((short *) &dec_indices.indices_gains[1],
		       (unsigned short *) RxPkt, 4, PktPtr);
	    BitUnpack ((short *) &dec_indices.indices_gainFrame,
		       (unsigned short *) RxPkt, 7, PktPtr);
	}
	else if (data_packet.PACKET_RATE == 4) {
	    BitUnpack ((short *) &dec_indices.indices_LSFs[0],
		       (unsigned short *) RxPkt, 8, PktPtr);



	    BitUnpack ((short *) &dec_indices.indices_gains[0],
		       (unsigned short *) RxPkt, 4, PktPtr);
	    BitUnpack ((short *) &dec_indices.indices_gainFrame,
		       (unsigned short *) RxPkt, 4, PktPtr);









	}

    }
    if (data_packet.WB_MODE_BIT == 1) {


	if (lastrateD == 1 && data_packet.PACKET_RATE == 14) {	// 8th rate erasure processing 
	    erasure_8R = 1;
	    initD = 1;
	}
	else
	    erasure_8R = 0;
	if (data_packet.PACKET_RATE == 1)
	    initD = 1;

	if (erasure_8R != 1)	// do full/quarter rate UB processing

	{


	    dequantize_WBparams (&(Dec_Qp_HB), dec_indices,
				 data_packet.PACKET_RATE);



	    /* boost HB by 2.5 dB during full rate; reduce by 2.5 dB during NELP */


	    if (autocorrelationFirstTime) {
		autocorrelationFirstTime = 0;
		for (k = 0; k < ORDER + 7; k++)
		    w[k] =
			exp (-0.5 *
			     pow (2 * 3.1416 * F0 * (float) k / 8000.0, 2.0));
		w[0] = 1.00003;
	    }


	    if (data_packet.PACKET_RATE == 3) {
		data_packet.PACKET_RATE = 14;	// if dim and burst erase upper band

		if (initD == 0) {
		    float ftmp;

		    ftmp = 0.48 / (float) LPC_ORD_WB;
		    //since no unpacking took place for HCELP frame , initialize the struc.
		    //Note : initD=0 only if 1st frame or prev frame was NB -- this is set inside main.cc
		    Dec_Qp_HB.LSFs[0] = ftmp;
		    for (k = 1; k < LPC_ORD_WB; k++)
			Dec_Qp_HB.LSFs[k] = Dec_Qp_HB.LSFs[k - 1] + ftmp;

		    for (k = 0; k < 5; k++)
			Dec_Qp_HB.Gain[k] = 0;

		    Dec_Qp_HB.GainFrame = 0;

		}
	    }


	    noise_synthesis (Dec_Qp_HB, buf_HB_out, initD);
	    for (k = 0; k < 7 * obuf_len / 8; k++)
		buf_HB_out[k] *= attn_fac;


	    for (k = 0; k < 7 * obuf_len / 4; k++)
		buf_WB14_out[k] = 0;

	    if (lastrateD == 1)	//ER->FR/QR
	    {
		/* reading from 14K state */
		for (k = 0; k < 28 + 48; k++)
		    buf_WB14_out[k] = Synstate14[k] * win_8R[280 - 48 + k];

		/* resetting 14K state */
		for (k = 0; k < 28 + 48; k++)
		    Synstate14[k] = 0;
	    }
	}






	else			//eighth-rate 
	{



	    for (k = 0; k < 7 * obuf_len / 4; k++)
		buf_WB14_out[k] = 0;


	    for (k = 0; k < 7 * obuf_len / 8; k++)
		buf_HB_out[k] = 0;

	    if (lastrateD != 1)	//FR/QR->ER
	    {
		/* reading 7K state */
		for (k = 0; k < 14; k++)
		    buf_HB_out[k] = Synstate[k];

		/* resetting 7K state */
		for (k = 0; k < 14; k++)
		    Synstate[k] = 0;
	    }
	}


	initD = 1;


	if (!args->lowband_decout) {

	    /* low-band synthesis filter */
	    filterbank_syn_lb (buf_float, buf_out, obuf_len, &S_syn_lb);

	    /* high-band synthesis filter */

	    //Time-Warping: Always Generate 160 here, Warp later below
	    filterbank_syn_hb (buf_tmp2, buf_HB_out, buf_WB14_out, 140,
			       &S_syn_hb);
	    //End: Time-Warping: Always Generate 160 here, Warp later below


	}
    }

    //Time-Warping of Upper Band

    float delay;
    int b, i;
    int num_residual = 320, num_samples;
    int merge_start, merge_end, merge_stop;
    float UB_warp_temp[4 * 160];
    float UB_warp[4 * 160];

    if (data_packet.WB_MODE_BIT == 1) {

	delay = LB_delay * 2;

	for (i = 0; i < (2 * 160); i++)
	    UB_warp[i] = buf_tmp2[i];

	if (delay > 0 && (time_warp_fraction > 1 || time_warp_fraction < 1))	//Expansion or Compression
	{
	    merge_start = 0;

	    if ((LB_delay) - floor (LB_delay) > 0.5)
		merge_end = (int) LB_delay + 1;
	    else
		merge_end = (int) LB_delay;

	    merge_end *= 2;
	    delay = merge_end;

	    if (merge_end - merge_start > num_residual / 2)
		merge_stop = num_residual - (merge_end - merge_start);
	    else
		merge_stop = merge_end - merge_start;


	    if (num_residual <= delay)
		merge_stop = 0;

	    num_samples = num_residual;

	    for (b = 0; b < num_residual; b++)
		UB_warp_temp[b] = UB_warp[b];

	    if (merge_stop != 0) {
		if (time_warp_fraction > 1)	//Expansion
		{
		    for (b = merge_start; b < merge_end; b++)
			UB_warp[b] = UB_warp_temp[b];

		    for (b = merge_start; b < merge_stop; b++)
			UB_warp[b + merge_end] =
			    UB_warp_temp[b +
					 (merge_end -
					  merge_start)] * (1.0 * (merge_stop -
								  merge_start
								  -
								  b) /
							   (merge_stop -
							    merge_start)) +
			    UB_warp_temp[b] * (1.0 * (b) /
					       (merge_stop - merge_start));

		    //Else, regular expansion as requested by de-jitter: produce one extra pitch period
		    {
			for (b = merge_start + merge_end + merge_stop;
			     b < num_residual + merge_end - merge_start; b++)
			    UB_warp[b] =
				UB_warp_temp[b - (merge_end - merge_start)];

			num_samples =
			    num_residual + (merge_end - merge_start);
		    }
		}
		//Regular compression as requested by de-jitter: produce one less pitch period
		//Compress only if compressed samples >= 40
		else if (time_warp_fraction < 1
			 && (num_residual - (merge_end - merge_start)) >=
			 40) {
		    for (b = merge_start; b < merge_stop; b++)
			UB_warp[b] =
			    UB_warp[b] * (1.0 *
					  (merge_stop - merge_start -
					   b) / (merge_stop - merge_start)) +
			    UB_warp[b +
				    (merge_end -
				     merge_start)] * (1.0 * (b) /
						      (merge_stop -
						       merge_start));

		    for (b = merge_stop;
			 b + merge_end - merge_start < num_residual; b++)
			UB_warp[b] = UB_warp[b + merge_end - merge_start];

		    num_samples = num_residual - (merge_end - merge_start);
		}
	    }

	    for (i = 0; i < (num_samples); i++)
		buf_tmp2[i] = UB_warp[i];
	}
	LB_delay = 0;

	//End: Time-Warping of Upper Band


	if (!args->lowband_decout) {
	    /* combine bands */
	    for (k = 0; k < 2 * obuf_len; k++)
		buf_float[k] =
		    (1.05f * buf_float[k] + 0.95f * buf_tmp2[k]) / 1.05;
	}

	/* saturation */
	if (!args->lowband_decout)
	    for (k = 0; k < 2 * obuf_len; k++) {
		if (buf_float[k] > 32767.0)
		    buf16[k] = (int16) 32767;
		else if (buf_float[k] < -32767.0)
		    buf16[k] = (int16) - 32767;
		else
		    buf16[k] = (int16) buf_float[k];
	    }
	{
	    /* Fixes ocassional problem with FER processing, dtx_likely_flag did not
	     * take into account energy in high band. */
	    float hbenergy;

	    hbenergy = 0.0;
	    for (k = 0; k < 2 * obuf_len; k++) {
		hbenergy += buf16[k] * buf16[k];
	    }
	    hbenergy = hbenergy / 2.0;
	    if (bbg_dtx_likely_flag) {
		/* Check if high band energy forces override of dtx_likely_flag */
		if ((hbenergy / bbg_dtx_likely_bgnenergy) >
		    pow (10.0, (36.1236 + 10.0) / 20.0)) {
		    bbg_dtx_likely_flag = 0;
		}
	    }
	}
	if (args->lowband_decout) {
	    for (k = 0; k < obuf_len; k++) {
		if (buf_out[k] > 32767.0)
		    buf16[k] = (int16) 32767;
		else if (buf_out[k] < -32767.0)
		    buf16[k] = (int16) - 32767;
		else
		    buf16[k] = (int16) buf_out[k];
	    }
	}
    }

    if (data_packet.WB_MODE_BIT == 0) {
	for (k = 0; k < obuf_len; k++) {
	    if (buf_out[k] > 32767.0)
		buf16[k] = (int16) 32767;
	    else if (buf_out[k] < -32767.0)
		buf16[k] = (int16) - 32767;
	    else
		buf16[k] = (int16) buf_out[k];
	}

    }


    if (erasure_8R != 1)
	lastrateD = bit_rate;	//moved this outside decode.cc
    else
	lastrateD = 1;


    if ((!BAD_RATE) && (data_packet.PACKET_RATE != 14)) {

	if (data_packet.WB_MODE_BIT == 1) {
	    WB_COUNT++;
	    NB_COUNT = 0;
	}
	else {
	    NB_COUNT++;
	    WB_COUNT = 0;
	}
    }

}
