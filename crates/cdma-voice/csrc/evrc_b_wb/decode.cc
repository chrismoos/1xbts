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
    "$Id: decode.cc,v 1.65.2.5 2007/06/24 06:43:06 apadmana Exp $";

//*======================================================================*/
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
/*  Module:     decode.c                                                */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*----------------------------------------------------------------------*/
/*  EVRC Decoder                                                        */
/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/

#include  <string.h>
#include  "macro.h"
#include  "globs.h"
#include  "proto.h"
#include  "rom.h"
#include  "acelp.h"		/* for ACELP */
#include  "struct.h"


//extern float global_lsp[ORDER];



void decode_da_fp (int *daindex, unsigned short fcb[][4]);

/*======================================================================*/
/*         ..Reset RCELP decoder parameters.                            */
/*----------------------------------------------------------------------*/
void
FGV_MEM::InitDecoder ()
{
    /*....(local) variables.... */
    int j;
    float ftmp;

    bit_rate = 4;
    last_valid_rate = 1;	/* reset last_valid_rate */
    decode_fcnt = 0;


    for (j = 0; j < ORDER; j++)
	SynMemory[j] = 0.0;
    if (data_packet.WB_MODE_BIT == 1) {
	for (j = 0; j < ORDER; j++) {
	    SynMemory_bbg[j] = 0.0;
	}
	for (j = 0; j < ACBMemSize; j++) {
	    PitchMemoryD_bbg[j] = 0.0;
	}
    }

    ftmp = 0.48 / (float) ORDER;
    OldOldlspD[0] = OldlspD[0] = ftmp;
    for (j = 1; j < ORDER; j++) {
	OldOldlspD[j] = OldOldlspD[j - 1] + ftmp;
	OldlspD[j] = OldlspD[j - 1] + ftmp;
    }
    for (j = 0; j < ACBMemSize; j++)
	PitchMemoryD[j] = PitchPreFiltMemoryD[j] = PitchMemoryD2[j] =
	    PitchMemoryD3[j] = 0.0;
    pdelayD = 40.0;
    ave_acb_gain = ave_fcb_gain = ave_acb_gain_back = 0.0;
    FadeScale = 1.0;

    prev_frame_error = 0;
    prev_prev_frame_error = 0;
    total_fer_counter = 0;

    erasureFlag = 0;
    errorFlag = 0;

    init_bbg ();
    consec_eighth_rates = 3;
}

void
FGV_MEM::InitDecoder_1 ()
{
    /*....(local) variables.... */
    int j;
    float ftmp;

    //bit_rate=4;
    last_valid_rate = 1;	/* reset last_valid_rate */

    for (j = 0; j < ORDER; j++)
	SynMemory[j] = 0.0;

    for (j = 0; j < ACBMemSize; j++)
	PitchMemoryD[j] = PitchPreFiltMemoryD[j] = PitchMemoryD2[j] =
	    PitchMemoryD3[j] = 0.0;
    pdelayD = 40.0;
    ave_acb_gain = ave_fcb_gain = ave_acb_gain_back = 0.0;
    FadeScale = 1.0;

    prev_frame_error = 0;
    prev_prev_frame_error = 0;
    erasureFlag = 0;
    errorFlag = 0;
    total_fer_counter = 0;

    for (j = 0; j < 14; j++)
	Synstate[j] = 0;

}


/*======================================================================*/
/*         ..Decode bitstream data.                                     */
/*----------------------------------------------------------------------*/

//float prevpD[ACBMemSize];
//char PPP_MODE_D;
//int rcelp_half_rateD;
//int prev_rcelp_half=0;

void
FGV_MEM::decode (short *codeBuf, short rate, short post_filter,
		 float *outFbuf, float time_warp_fraction, short *oBuf_len)
{

    /*....(local) variables.... */
    register int i;
    short tmp, dmp[1];
    short local_rate;

    //void fer_processing(float*,short,short);
    short spl_hcelp_wb = 0x7B;	//WB special lag identifier (123) for spl_hcelp frame
    short spl_hcelp_nb = 0x6E;
    short poss_spl_hcelp_wb;
    short poss_qnelp;

//  short update_bbg_flag;
    SPL_HCELP_WB = 0;
    BAD_RATE = 0;
    SPL_HCELP = 0;
    SPL_HPPP = 0;
    SPL_HNELP = 0;

    // NOTE: "rate" is a true rate and not like that in ENC's VAD
    for (i = 0; i < PACKWDSNUM; i++) {
	RxPkt[i] = codeBuf[i];
    }
    data_packet.PACKET_RATE = rate;



    /* Re-initialize PackWdsPtr */
    PackWdsPtr[0] = 16;
    PackWdsPtr[1] = 0;
    errorFlag = 0;
    //gautam
    PktPtr[0] = 16;
    PktPtr[1] = 0;

    PACKET_RATE = data_packet.PACKET_RATE;

    rcelp_half_rateD = 0;
    PPP_MODE_D = ' ';		//Reset the two global variables  



    // Time_Warping Setting. Bug-fix: This line is moved here for not only clean frame decoding but also erasure concealment.
    *oBuf_len = 160;
    int do_phase_match = 0;

    if (data_packet.WB_MODE_BIT == 0) {	//to maintain sync with  NB code
	//Phase Matching variable


	if (phase_offset != 10)	//Phase Offset == 10 denotes Phase Matching disabled
	    do_phase_match = 1;	//check for Phase Matching
	if ((rate == 4) || (rate == 3) || (rate == 2))
	    decode_DTX_STATE = 0;
	else if (rate == 1)
	    decode_DTX_STATE = 1;

	if ((rate == 0) || (rate == 0xE)) {
	    if ((lastrateD == 4) || (lastrateD == 3) || (lastrateD == 2))
		rate = 0xE;
	    else if ((lastrateD == 1) || (lastrateD == 0))
		rate = 0;
	    else {
		if (decode_DTX_STATE == 1)
		    rate = 0;
		else
		    rate = 0xE;
	    }
	}
	data_packet.PACKET_RATE = rate;
	PACKET_RATE = data_packet.PACKET_RATE;

    }

  Error_Label:
    // Check for Erasure - from codeBuf or from packet.cc or from external ctrl
    if (rate == 0xE) {
	if (data_packet.WB_MODE_BIT == 1) {
	    erasureFlag = 1;
	    if (fer_counter == 3)
		ave_acb_gain *= 0.75;
	    fer_counter = 3;
	    if (lastrateD > 2) {
		ave_acb_gain_back = ave_acb_gain;
		pdelayD2 = pdelayD;
		for (i = 0; i < ACBMemSize; i++)
		    PitchMemoryD2[i] = PitchMemoryD[i];
	    }




	    if (lastrateD != 3)
		prev_rcelp_half = 0;
	    if (prev_rcelp_half == 1) {
		lastrateD = 4;
		prev_rcelp_half = 0;
	    }
	}
	else {
	    if (!do_phase_match) {

		total_fer_counter++;
		if (fer_counter == 3)
		    ave_acb_gain *= 0.75;
		fer_counter = 3;
		if (lastrateD > 2) {
		    ave_acb_gain_back = ave_acb_gain;
		    pdelayD2 = pdelayD;
		    for (i = 0; i < ACBMemSize; i++)
			PitchMemoryD2[i] = PitchMemoryD[i];
		}

		if (lastrateD != 3)
		    prev_rcelp_half = 0;
		if (prev_rcelp_half == 1) {
		    lastrateD = 4;
		    prev_rcelp_half = 0;
		}
	    }
	}


	fer_processing (outFbuf, post_filter, bit_rate = lastrateD);
	if (data_packet.WB_MODE_BIT == 1) {

	    {
		float outenergy = 0;

		for (i = 0; i < SubFrameSize; i++) {
		    outenergy +=
			outFbuf[2 * (SubFrameSize - 1) +
				i] * outFbuf[2 * (SubFrameSize - 1) + i];
		}
		/* If fer_processing() output within 5dB of estimated bg level,
		 * force dtx_likely_flag to inject bg noise on next consecutive
		 * erased frame */
		if (outenergy / bbg_dtx_likely_bgnenergy <
		    pow (10.0, (36.1236 + 5.0) / 20.0)) {
		    bbg_dtx_likely_flag = 1;
		}
	    }
	}
    }

    else {			// no erasure: clean frame case

	norm_fade_fac = 1;
	total_fer_counter = 0;
	if (data_packet.WB_MODE_BIT == 1) {
	    fer_counter--;
	    if (fer_counter < 0)
		fer_counter = 0;
	}


	if (data_packet.WB_MODE_BIT == 1) {

	    if (args->dtx)	// Only run if dtx mode is on
	    {
		if (PACKET_RATE == 1) {
		    if (consec_eighth_rates < 3)	// Most likely in DTX
		    {
			consec_eighth_rates++;
			/* Force next call to silence_decoder to run update_bbg_estimate()
			 * internally and to skip updating the filter states of the main
			 * branch */
			update_bbg_reenc_flag = 1;
			// Decode packet to update bgn estimate before re-encoding
			BitUnpack ((short *) &data_packet.LSP_IDX[0],
				   (unsigned short *) RxPkt, 5, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[1],
				   (unsigned short *) RxPkt, 5, PktPtr);
			BitUnpack ((short *) &data_packet.SILENCE_GAIN,
				   (unsigned short *) RxPkt, NUM_EQ_BITS_WB,
				   PktPtr);
			BitUnpack (zrbit, (unsigned short *) RxPkt, 1, PktPtr);	//15th bit
			//do bad-rate check
			BAD_RATE = bad_rate_check (0, codeBuf);

			silence_decoder (outFbuf, 1);
			/* Make sure next call to silence_decoder() DOES NOT
			 * call update_bbg_estimate() */
			update_bbg_reenc_flag = 0;
			// update_bbg_estimate() already called inside silence_decoder.
			update_bbg_flag = 0;
			PACKET_RATE = 0;
		    }
		}
		else if (PACKET_RATE == 0) {
		    consec_eighth_rates = 0;
		}

		update_bbg_flag = 0;
		if (PACKET_RATE == 0)	//suppressed eighth rate
		{

		    gen_encode_bbg (codeBuf);
		    // After this, process as eighth rate.
		    PACKET_RATE = data_packet.PACKET_RATE = 1;

		    // Reset PktPtr
		    for (i = 0; i < PACKWDSNUM; i++) {
			RxPkt[i] = codeBuf[i];
		    }
		    PktPtr[0] = 16;
		    PktPtr[1] = 0;
		}
		else if (PACKET_RATE == 1) {
		    update_bbg_flag = 1;
		}
	    }
	}

	if (data_packet.WB_MODE_BIT == 1)	//WB decoding
	{
	    data_packet.MODE_BIT = 0;
	}
	else {
	    if ((PACKET_RATE == 2) || (PACKET_RATE == 4))
		BitUnpack ((short *) &data_packet.MODE_BIT,
			   (unsigned short *) RxPkt, 1, PktPtr);

	}



	if (PACKET_RATE == 3)	//SPL packet detector 
	{
	    //chk for spl. 1/2 rate CELP and PPP
	    BitUnpack (&poss_spl_hcelp_wb, (unsigned short *) RxPkt, 7,
		       PktPtr);
	    //BitUnpack(&poss_qnelp,(unsigned short *)RxPkt,2,PktPtr);
	    if ((poss_spl_hcelp_wb == spl_hcelp_wb)
		&& data_packet.WB_MODE_BIT == 1) {

		data_packet.WB_MODE_BIT = 1;
		BitUnpack ((short *) &data_packet.LSP_IDX[0],
			   (unsigned short *) RxPkt, 6, PktPtr);
		BitUnpack ((short *) &data_packet.LSP_IDX[1],
			   (unsigned short *) RxPkt, 6, PktPtr);
		BitUnpack ((short *) &data_packet.LSP_IDX[2],
			   (unsigned short *) RxPkt, 9, PktPtr);
		BitUnpack ((short *) &data_packet.LSP_IDX[3],
			   (unsigned short *) RxPkt, 7, PktPtr);

		for (i = 0; i < NoOfSubFrames; i++)
		    BitUnpack ((short *) &data_packet.ACBG_IDX[i],
			       (unsigned short *) RxPkt, 3, PktPtr);

		BitUnpack ((short *) &data_packet.DELAY_IDX,
			   (unsigned short *) RxPkt, 7, PktPtr);
		data_packet.PACKET_RATE = 3;
		PACKET_RATE = data_packet.PACKET_RATE;
		SPL_HCELP_WB = 1;
	    }
	    else		//NB decoding
	    {
		//BitUnpack(&poss_spl_hcelp,(unsigned short *)RxPkt,7,PktPtr);
		BitUnpack (&poss_qnelp, (unsigned short *) RxPkt, 2, PktPtr);
		if (poss_spl_hcelp_wb == spl_hcelp_nb) {
		    if (poss_qnelp == 0) {
			BitUnpack ((short *) &data_packet.MODE_BIT,
				   (unsigned short *) RxPkt, 1, PktPtr);
			if (data_packet.MODE_BIT == 0)	//unpacking for SPL_HCELP
			{
			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[2],
				       (unsigned short *) RxPkt, 9, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[3],
				       (unsigned short *) RxPkt, 7, PktPtr);
			    for (i = 0; i < NoOfSubFrames; i++)
				BitUnpack ((short *) &data_packet.ACBG_IDX[i],
					   (unsigned short *) RxPkt, 3,
					   PktPtr);
			    BitUnpack ((short *) &data_packet.DELAY_IDX,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    data_packet.PACKET_RATE = 4;
			    PACKET_RATE = data_packet.PACKET_RATE;
			    SPL_HCELP = 1;
			}
			else	//unpacking for SPL_HPPP
			{

			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[2],
				       (unsigned short *) RxPkt, 9, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[3],
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.F_ROT_IDX,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[2],
				       (unsigned short *) RxPkt, 8, PktPtr);
			    BitUnpack ((short *) &data_packet.A_POWER_IDX,
				       (unsigned short *) RxPkt, 8, PktPtr);
			    //BitUnpack((short *) &data_packet.DELAY_IDX,(unsigned short *)RxPkt,7,PktPtr);
			    BitUnpack ((short *) &data_packet.delayindex,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    for (i = 0; i < NUM_FIXED_BAND; i++) {
				if (i < 3)
				    data_packet.F_ALIGN_IDX[i] = 0x10;
				else
				    data_packet.F_ALIGN_IDX[i] = 0x20;
			    }
			    data_packet.PACKET_RATE = 4;
			    PACKET_RATE = data_packet.PACKET_RATE;
			    SPL_HPPP = 1;
			}
		    }
		    else	// for poss_qnelp, unpacking for SPL_HNELP
		    {
			BitUnpack ((short *) &data_packet.LSP_IDX[0],
				   (unsigned short *) RxPkt, 8, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[1],
				   (unsigned short *) RxPkt, 8, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[0][0],
				   (unsigned short *) RxPkt, 5, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[1][0],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[1][1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_FID,
				   (unsigned short *) RxPkt, 2, PktPtr);
			data_packet.PACKET_RATE = 2;
			PACKET_RATE = data_packet.PACKET_RATE;
			SPL_HNELP = 1;
		    }
		}
		else {		// not a spl. 1/2 rate , restore RxPkt
		    for (i = 0; i < PACKWDSNUM; i++) {
			RxPkt[i] = codeBuf[i];
		    }
		    PktPtr[0] = 16;
		    PktPtr[1] = 0;
		}

	    }
	}
	if (((LAST_PACKET_RATE_D == 1) || ((LAST_PACKET_RATE_D == 2) && (LAST_MODE_BIT_D == 0))) && (SPL_HCELP == 1)) {	//prev was QNELP/Sil
	    ones_dec_cnt = 0;
	    dec_lsp_vq_28 ((short *) data_packet.LSP_IDX, OldlspD);

	    PACKET_RATE = data_packet.PACKET_RATE = 0xE;	//erasure , bug-fix in the way the next frame is processed
	    rate = 0xE;
	    if (lastrateD != 3)
		prev_rcelp_half = 0;
	    if (prev_rcelp_half == 1) {
		lastrateD = 4;
		prev_rcelp_half = 0;
	    }
	    fer_processing (outFbuf, post_filter, bit_rate = lastrateD);

	    /* Use curr frames good LSP if curr frame is spl hcelp and prev frame was QNELP/Sil */
	}
	else {
	    //bad rate check for speech, decoding and other updates

	    //unpacking routine
	    switch (data_packet.PACKET_RATE) {
	    case 1:
		//silence

		if (data_packet.WB_MODE_BIT == 1) {
		    BitUnpack ((short *) &data_packet.LSP_IDX[0],
			       (unsigned short *) RxPkt, 5, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[1],
			       (unsigned short *) RxPkt, 5, PktPtr);
		    BitUnpack ((short *) &data_packet.SILENCE_GAIN,
			       (unsigned short *) RxPkt, NUM_EQ_BITS_WB,
			       PktPtr);
		    //fprintf(stderr,"unpacking eighth rate frames\n");



		}
		else		//NB decoding
		{
		    BitUnpack ((short *) &data_packet.LSP_IDX[0],
			       (unsigned short *) RxPkt, 4, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[1],
			       (unsigned short *) RxPkt, 4, PktPtr);
		    BitUnpack ((short *) &data_packet.SILENCE_GAIN,
			       (unsigned short *) RxPkt, NUM_EQ_BITS, PktPtr);
		    BitUnpack (zrbit, (unsigned short *) RxPkt, 1, PktPtr);	//15th bit

		}

		BAD_RATE = bad_rate_check (0, codeBuf);


		break;
	    case 2:
		if (data_packet.WB_MODE_BIT == 1) {

		    if (data_packet.MODE_BIT == 0) {
			BitUnpack (dmp, (unsigned short *) RxPkt, 6, PktPtr);
			//NELP
			BitUnpack ((short *) &data_packet.LSP_IDX[0],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[2],
				   (unsigned short *) RxPkt, 9, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[3],
				   (unsigned short *) RxPkt, 7, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[0][0],
				   (unsigned short *) RxPkt, 5, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[1][0],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_GAIN_IDX[1][1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.NELP_FID,
				   (unsigned short *) RxPkt, 2, PktPtr);
			if (data_packet.WB_MODE_BIT == 0)	//NB decoding       
			    BitUnpack (zrbit, (unsigned short *) RxPkt, 4, PktPtr);	// no free bits



			//do bad-rate check
			BAD_RATE = bad_rate_check (1, codeBuf);

		    }
		    else if (data_packet.MODE_BIT == 1) {
			//QPPP

			BitUnpack ((short *) &data_packet.LSP_IDX[0],
				   (unsigned short *) RxPkt, 10, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.Q_delta_lag,
				   (unsigned short *) RxPkt, 4, PktPtr);
			BitUnpack ((short *) &data_packet.AMP_IDX[0],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.AMP_IDX[1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.POWER_IDX,
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack (zrbit, (unsigned short *) RxPkt, 1, PktPtr);	//40th bit
			//do bad-rate check
			BAD_RATE = bad_rate_check (2, codeBuf);
		    }
		    else {
			cerr << "MODE_BIT neither 0 nor 1 in Frame #" <<
			    decode_fcnt << "\n";
		    }
		}
		else {		//narrowband case
		    if (SPL_HNELP == 0) {	//if SPL_HNELP is 1 unpacking done in spl_half-rate section above
			if (data_packet.MODE_BIT == 0) {
			    //NELP
			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 8, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 8, PktPtr);
			    BitUnpack ((short *) &data_packet.
				       NELP_GAIN_IDX[0][0],
				       (unsigned short *) RxPkt, 5, PktPtr);
			    BitUnpack ((short *) &data_packet.
				       NELP_GAIN_IDX[1][0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.
				       NELP_GAIN_IDX[1][1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.NELP_FID,
				       (unsigned short *) RxPkt, 2, PktPtr);
			    BitUnpack (zrbit, (unsigned short *) RxPkt, 4, PktPtr);	// no free bits
			    //do bad-rate check
			    BAD_RATE = bad_rate_check (1, codeBuf);
			}
			else if (data_packet.MODE_BIT == 1) {
			    //QPPP
			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 10, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.Q_delta_lag,
				       (unsigned short *) RxPkt, 4, PktPtr);
			    BitUnpack ((short *) &data_packet.AMP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.AMP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.POWER_IDX,
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack (zrbit, (unsigned short *) RxPkt, 1, PktPtr);	//40th bit
			    //do bad-rate check
			    BAD_RATE = bad_rate_check (2, codeBuf);
			}
		    }

		}

		break;
	    case 3:
		//Half Rate Celp
		if (SPL_HCELP_WB != 1) {	//for special half rate celp, unpacking already performed.
		    BitUnpack ((short *) &data_packet.DELAY_IDX,
			       (unsigned short *) RxPkt, 7, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[0],
			       (unsigned short *) RxPkt, 7, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[1],
			       (unsigned short *) RxPkt, 7, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[2],
			       (unsigned short *) RxPkt, 8, PktPtr);
		    for (i = 0; i < NoOfSubFrames; i++)
			BitUnpack ((short *) &data_packet.ACBG_IDX[i],
				   (unsigned short *) RxPkt, 3, PktPtr);
		    for (i = 0; i < NoOfSubFrames; i++)
			BitUnpack ((short *) &data_packet.FCB_PULSE_IDX[i][0],
				   (unsigned short *) RxPkt, 10, PktPtr);
		    for (i = 0; i < NoOfSubFrames; i++)
			BitUnpack ((short *) &data_packet.FCBG_IDX[i],
				   (unsigned short *) RxPkt, 4, PktPtr);
		    //do bad-rate check
		    BAD_RATE = bad_rate_check (3, codeBuf);

		}

		if (SPL_HCELP_WB == 1) {
		    // the above section of code skips bad-rate check for SPL_HCELP_WB , the following lines below check for bad LSPs for a SPL_HCELP_WB
		    dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);
		    float m0, m1;

		    m0 = MAX (lsp[0], lsp[1]);
		    m1 = MIN (MIN (lsp[4], lsp[5]), lsp[6]);
		    if (m0 >= m1) {
			//printf("m0 is > m1\n");
			BAD_RATE = 1;
		    }
		    if (data_packet.DELAY_IDX > 120) {
			//lags of 124 and 125 will not be packed since this SPL_HCELP_WB packet comes from a FCELP_WB packet
			BAD_RATE = 1;

		    }
		}
		break;
	    case 4:

		if (data_packet.WB_MODE_BIT == 1) {

		    if (data_packet.Celp_Mdct_Flag == 0) {

			if (data_packet.MODE_BIT == 0) {
			    if (SPL_HCELP == 0) {

				//FCELP
				BitUnpack ((short *) &data_packet.DELAY_IDX,
					   (unsigned short *) RxPkt, 7,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[0],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[1],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[2],
					   (unsigned short *) RxPkt, 9,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[3],
					   (unsigned short *) RxPkt, 7,
					   PktPtr);

				//RS1
				for (i = 0; i < NoOfSubFrames; i++)
				    BitUnpack ((short *) &data_packet.
					       ACBG_IDX[i],
					       (unsigned short *) RxPkt, 3,
					       PktPtr);

				short fxindices[NoOfSubFrames][3];


				for (i = 0; i < NoOfSubFrames; i++) {
				    BitUnpack ((short *) &fxindices[i][0],
					       (unsigned short *) RxPkt, 16,
					       PktPtr);
				    BitUnpack ((short *) &fxindices[i][1],
					       (unsigned short *) RxPkt, 8,
					       PktPtr);
				}
				BitUnpack ((short *) &fxindices[0][2],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &fxindices[1][2],
					   (unsigned short *) RxPkt, 7,
					   PktPtr);
				BitUnpack ((short *) &fxindices[2][2],
					   (unsigned short *) RxPkt, 7,
					   PktPtr);

				for (i = 0; i < NoOfSubFrames; i++) {
				    data_packet.FCB_PULSE_IDX[i][0] =
					(fxindices[i][0] & 0x00FF);
				    data_packet.FCB_PULSE_IDX[i][1] =
					((fxindices[i][0] >> 8) & 0x00FF);

				    data_packet.FCB_PULSE_IDX[i][2] =
					fxindices[i][1];
				    data_packet.FCB_PULSE_IDX[i][3] =
					fxindices[i][2];

				}


				BitUnpack ((short *) &idxe,
					   (unsigned short *) RxPkt, 3,
					   PktPtr);
				for (i = 0; i < NoOfSubFrames; i++)
				    BitUnpack ((short *) &data_packet.
					       FCBG_IDX[i],
					       (unsigned short *) RxPkt, 4,
					       PktPtr);


			    }	//for SPL_HCELP

			    //do bad-rate check
			    BAD_RATE = bad_rate_check (5, codeBuf);	//do this for spl. packet also


			}
			else if (data_packet.MODE_BIT == 1) {
			    if (SPL_HPPP == 0) {
				//FPPP
				BitUnpack ((short *) &data_packet.delayindex,
					   (unsigned short *) RxPkt, 7,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[0],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[1],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[2],
					   (unsigned short *) RxPkt, 9,
					   PktPtr);
				BitUnpack ((short *) &data_packet.LSP_IDX[3],
					   (unsigned short *) RxPkt, 7,
					   PktPtr);
				BitUnpack ((short *) &data_packet.F_ROT_IDX,
					   (unsigned short *) RxPkt, 7,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   A_AMP_IDX[0],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   A_AMP_IDX[1],
					   (unsigned short *) RxPkt, 6,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   A_AMP_IDX[2],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &data_packet.A_POWER_IDX,
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				for (i = 0; i < NUM_FIXED_BAND; i++) {
				    if (i < 3)
					BitUnpack ((short *) &data_packet.
						   F_ALIGN_IDX[i],
						   (unsigned short *) RxPkt,
						   5, PktPtr);
				    else
					BitUnpack ((short *) &data_packet.
						   F_ALIGN_IDX[i],
						   (unsigned short *) RxPkt,
						   6, PktPtr);
				}

				//Find out the first empty bin to unpack the VC

			    }	// for SPL_HPPP
			    //do bad-rate check
			    BAD_RATE = bad_rate_check (4, codeBuf);
			}
			else {
			    cerr << "MODE_BIT neither 0 nor 1 in Frame #" <<
				decode_fcnt << "\n";

			}
			break;
		    }
		    else {	// music mode

			BitUnpack ((short *) &data_packet.LSP_IDX[0],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[1],
				   (unsigned short *) RxPkt, 6, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[2],
				   (unsigned short *) RxPkt, 9, PktPtr);
			BitUnpack ((short *) &data_packet.LSP_IDX[3],
				   (unsigned short *) RxPkt, 7, PktPtr);

			for (i = 0; i < MDCT_NWORDS - 1; i++) {
			    BitUnpack ((short *) &data_packet.MDCT_IDX[i],
				       (unsigned short *) RxPkt, 15, PktPtr);
			}
			BitUnpack ((short *) &data_packet.
				   MDCT_IDX[MDCT_NWORDS - 1],
				   (unsigned short *) RxPkt, MDCT_REMAINDER,
				   PktPtr);

			BitUnpack ((short *) &data_packet.FrameGain_IDX,
				   (unsigned short *) RxPkt, 7, PktPtr);
			BitUnpack ((short *) &data_packet.NoiseGain_IDX,
				   (unsigned short *) RxPkt, 2, PktPtr);

			break;
		    }
		}
		else {		//narrowband full rate celp decoding

		    if (data_packet.MODE_BIT == 0) {
			if (SPL_HCELP == 0) {
			    //FCELP
			    BitUnpack ((short *) &data_packet.DELAY_IDX,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[2],
				       (unsigned short *) RxPkt, 9, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[3],
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.DELTA_DELAY_IDX,
				       (unsigned short *) RxPkt, 5, PktPtr);
			    for (i = 0; i < NoOfSubFrames; i++)
				BitUnpack ((short *) &data_packet.ACBG_IDX[i],
					   (unsigned short *) RxPkt, 3,
					   PktPtr);
			    for (i = 0; i < NoOfSubFrames; i++) {
				BitUnpack ((short *) &data_packet.
					   FCB_PULSE_IDX[i][1],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   FCB_PULSE_IDX[i][0],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   FCB_PULSE_IDX[i][3],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &data_packet.
					   FCB_PULSE_IDX[i][2],
					   (unsigned short *) RxPkt, 8,
					   PktPtr);
				BitUnpack ((short *) &tmp,
					   (unsigned short *) RxPkt, 3,
					   PktPtr);
				data_packet.FCB_PULSE_IDX[i][3] += (tmp << 8);
			    }
			    for (i = 0; i < NoOfSubFrames; i++)
				BitUnpack ((short *) &data_packet.FCBG_IDX[i],
					   (unsigned short *) RxPkt, 5,
					   PktPtr);
			}
			//do bad-rate check
			BAD_RATE = bad_rate_check (5, codeBuf);	//do this for spl. packet also

		    }
		    else if (data_packet.MODE_BIT == 1) {
			if (SPL_HPPP == 0) {
			    //FPPP
			    BitUnpack ((short *) &data_packet.delayindex,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[2],
				       (unsigned short *) RxPkt, 9, PktPtr);
			    BitUnpack ((short *) &data_packet.LSP_IDX[3],
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.F_ROT_IDX,
				       (unsigned short *) RxPkt, 7, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[0],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[1],
				       (unsigned short *) RxPkt, 6, PktPtr);
			    BitUnpack ((short *) &data_packet.A_AMP_IDX[2],
				       (unsigned short *) RxPkt, 8, PktPtr);
			    BitUnpack ((short *) &data_packet.A_POWER_IDX,
				       (unsigned short *) RxPkt, 8, PktPtr);
			    for (i = 0; i < NUM_FIXED_BAND; i++) {
				if (i < 3)
				    BitUnpack ((short *) &data_packet.
					       F_ALIGN_IDX[i],
					       (unsigned short *) RxPkt, 5,
					       PktPtr);
				else
				    BitUnpack ((short *) &data_packet.
					       F_ALIGN_IDX[i],
					       (unsigned short *) RxPkt, 6,
					       PktPtr);
			    }	//for
			}	//for SPL_HPPP
			//do bad-rate check
			BAD_RATE = bad_rate_check (4, codeBuf);
		    }
		    else {
			cerr << "MODE_BIT neither 0 nor 1 in Frame #" <<
			    decode_fcnt << "\n";
		    }
		    break;

		}

	    }


	    if (data_packet.WB_MODE_BIT == 0) {

		if (PACKET_RATE == 1 && !BAD_RATE) {
		    if (consec_eighth_rates < 3)	// Most likely in DTX
		    {
			consec_eighth_rates++;
			// Decode packet to update bgn estimate before re-encoding
			silence_decoder (outFbuf, 1);
			// Update bgn estimate
			update_bbg_estimate (outFbuf, PACKET_RATE, 1.0);
			PACKET_RATE = 0;
		    }
		}
		else if (PACKET_RATE == 0) {
		    consec_eighth_rates = 0;
		}

		if (PACKET_RATE == 0)	//suppressed eighth rate
		{

		    gen_encode_bbg (codeBuf);
		    // After this, process as eighth rate.
		    PACKET_RATE = data_packet.PACKET_RATE = 1;
		    update_bbg_flag = 0;

		    // Reset PktPtr
		    for (i = 0; i < PACKWDSNUM; i++) {
			RxPkt[i] = codeBuf[i];
		    }
		    PktPtr[0] = 16;
		    PktPtr[1] = 0;
		    BitUnpack ((short *) &data_packet.LSP_IDX[0],
			       (unsigned short *) RxPkt, 4, PktPtr);
		    BitUnpack ((short *) &data_packet.LSP_IDX[1],
			       (unsigned short *) RxPkt, 4, PktPtr);
		    BitUnpack ((short *) &data_packet.SILENCE_GAIN,
			       (unsigned short *) RxPkt, NUM_EQ_BITS, PktPtr);
		}
		else {
		    update_bbg_flag = 1;
		}

	    }

	    if (!BAD_RATE) {
		if (data_packet.WB_MODE_BIT == 0) {
		    N_consec_ers = 0;
		    fer_counter--;
		    if (fer_counter < 0)
			fer_counter = 0;
		    //Translate the PACKET_RATE to the appropriate bit_rate and PPP_MODE
		}

		//Translate the PACKET_RATE to the appropriate bit_rate and PPP_MODE
		switch (PACKET_RATE) {
		case 0:
		    if (!args->dtx) {
			break;
		    }
		case 1:
		    bit_rate = local_rate = 1;
		    break;
		case 2:
		    if (data_packet.WB_MODE_BIT == 1) {
			data_packet.MODE_BIT = 0;
		    }
		    MODE_BIT = data_packet.MODE_BIT;
		    if (MODE_BIT == 0)
			bit_rate = local_rate = 2;
		    else {
			bit_rate = local_rate = 3;
			PPP_MODE_D = 'Q';
			data_packet.MODE_BIT = 1;
			data_packet.PACKET_RATE = 2;
		    }
		    break;
		case 3:
		    bit_rate = local_rate = 3;
		    rcelp_half_rateD = 1;	//Half-rate CELP
		    break;
		case 4:

		    if (data_packet.WB_MODE_BIT == 1) {

			if (data_packet.Celp_Mdct_Flag == 0) {

			    data_packet.MODE_BIT = 0;
			    MODE_BIT = data_packet.MODE_BIT;
			    if (MODE_BIT == 0)
				bit_rate = local_rate = 4;
			    else {
				bit_rate = local_rate = 3;
				PPP_MODE_D = 'F';
				data_packet.MODE_BIT = 1;
				data_packet.PACKET_RATE = 4;
			    }
			}
			else
			    bit_rate = local_rate = 4;
		    }
		    else {
			MODE_BIT = data_packet.MODE_BIT;
			if (MODE_BIT == 0)
			    bit_rate = local_rate = 4;
			else {
			    bit_rate = local_rate = 3;
			    PPP_MODE_D = 'F';
			    data_packet.MODE_BIT = 1;
			    data_packet.PACKET_RATE = 4;
			}

		    }
		    break;
		}

	    }
	    //Time-Warping: Setting oBuf_len to 160 in case NELP or CELP is not called
	    if (data_packet.WB_MODE_BIT == 1) {

		/* Check for errors in FPMP code */
		if (bit_rate == 4 && data_packet.Celp_Mdct_Flag == 0) {
		    int64 code;
		    int fpmp_error = 0;

		    int numIdx = 4;

		    decode_da_fp (da_index, data_packet.FCB_PULSE_IDX);

		    for (i = 0; i < NoOfSubFrames; i++) {
			for (int j = 0; j < numIdx; j++) {
			    fcbIndexVector[j] =
				(int) data_packet.FCB_PULSE_IDX[i][j];
			}
			fpmp_error |= check_fpmp_code (fcbIndexVector, i);
			//fprintf(stderr, "DECODE.CC:  %d %04hx %04hx %04hx %04hx \n", fpmp_error, fcbIndexVector[0], fcbIndexVector[1], fcbIndexVector[2], fcbIndexVector[3]);
		    }
		    if (fpmp_error) {
			errorFlag = 1;
			rate = 0xE;
			data_packet.PACKET_RATE = rate;
			fprintf (stderr,
				 "Error detected in FCB_PULSE_IDX!\n");
			goto Error_Label;
		    }
		}
	    }
	    else {

		if (!BAD_RATE) {

		    //If not full-rate, disable Phase Matching for HCELP and QNELP
		    if (((bit_rate != 4 && rcelp_half_rateD == 1)
			 || (bit_rate < 3)) && do_phase_match == 1) {
			do_phase_match = 0;
			if (phase_offset == -1) {
			    rate = 0xE;
			    printf ("\n going back to Lable 1 %d", bit_rate);
			    go_back_input = 1;
			    goto Error_Label;
			}
			printf ("\n 1setting phase_offset to 10 %d",
				bit_rate);
			phase_offset = 10;
		    }
		    //Phase Matching: if delay change is big, disable Phase Matching
		    if (((bit_rate == 3 && PPP_MODE_D == 'F'
			  && fabs (data_packet.delayindex + DMIN - pdelayD) >=
			  15) || (bit_rate == 4
				  && fabs (data_packet.DELAY_IDX + DMIN -
					   pdelayD) >= 15))
			&& do_phase_match == 1) {
			do_phase_match = 0;
			if (phase_offset == -1) {
			    rate = 0xE;
			    printf ("\n going back to Lable 2 %d %d %f",
				    bit_rate, data_packet.delayindex + DMIN,
				    pdelayD);
			    go_back_input = 1;
			    goto Error_Label;
			}

			if (bit_rate == 3)
			    printf
				("\n 2setting phase_offset to 10 %d %d %f %d",
				 bit_rate, data_packet.delayindex + DMIN,
				 pdelayD, data_packet.DELAY_IDX + DMIN);
			else
			    printf ("\n 2setting phase_offset to 10 %d %d %f",
				    bit_rate, data_packet.DELAY_IDX + DMIN,
				    pdelayD);
			phase_offset = 10;
		    }
		    //If full-rate CELP and delta delay == 0, disable Phase Matching
		    if (bit_rate == 4 && data_packet.DELTA_DELAY_IDX == 0
			&& do_phase_match == 1) {
			do_phase_match = 0;
			if (phase_offset == -1) {
			    rate = 0xE;
			    printf ("\n going back to Lable 3 %d %d",
				    bit_rate, data_packet.DELTA_DELAY_IDX);
			    go_back_input = 1;
			    goto Error_Label;
			}
			printf ("\n 3setting phase_offset to 10 %d",
				bit_rate);
			phase_offset = 10;
		    }
		}


	    }

	    if (!BAD_RATE) {

		// Do regular decoding
		switch (bit_rate) {

		case 1:

		    silence_decoder (outFbuf, post_filter);
		    if (data_packet.PACKET_RATE != 1)
			cerr <<
			    "Data packet wrongly called SILENCE in Frame #:"
			    << decode_fcnt << "\n" << "rate is actually:" <<
			    data_packet.PACKET_RATE << "\n";
		    break;

		case 2:
		    //Time-Warping: Changed to nelp_decoder call below
		    nelp_decoder (outFbuf, post_filter, time_warp_fraction,
				  oBuf_len);
		    if (data_packet.PACKET_RATE != 2)
			cerr << "Data packet wrongly called NELP in Frame #:"
			    << decode_fcnt << "\n" << "rate is actually:" <<
			    data_packet.PACKET_RATE << "\n";
		    if (data_packet.MODE_BIT != 0)
			cerr << "Rate not set for NELP in Frame #:" <<
			    decode_fcnt << "\n";
		    break;

		case 3:

		    if (rcelp_half_rateD) {
			//Time-Warping: Changed to celp_decoder call below
			celp_decoder (outFbuf, post_filter, bit_rate, 1,
				      oBuf_len);
			if (data_packet.PACKET_RATE != 3)
			    cerr <<
				"Data packet wrongly called RCELP in Frame #:"
				<< decode_fcnt << "\n" << "rate is actually:"
				<< data_packet.PACKET_RATE << "\n";
		    }
		    else {
			voiced_decoder (outFbuf, post_filter, run_length,
					phase_offset, time_warp_fraction,
					oBuf_len);
			//printf ("\n voiced decoding %d", phase_offset);
		    }
		    break;

		case 4:
		    // if(data_packet.WB_MODE_BIT==0 || (data_packet.Celp_Mdct_Flag==0 && data_packet.WB_MODE_BIT==1)){
		    if (data_packet.Celp_Mdct_Flag == 0) {
			//Time-Warping: Changed to celp_decoder call below
			celp_decoder (outFbuf, post_filter, bit_rate,
				      time_warp_fraction, oBuf_len);
			if (data_packet.PACKET_RATE != 4)
			    cerr <<
				"Data packet wrongly called FCELP in Frame #:"
				<< decode_fcnt << "\n" << "rate is actually:"
				<< data_packet.PACKET_RATE << "\n";
			break;
		    }
		    else {
			music_decoder (outFbuf);
			PitchGain = 1;
			delay = 60;
			break;
		    }

		case 12:
		    cerr << "come in case 12 frame #:" << decode_fcnt << "\n";
		    break;


		}


		if (bit_rate == 4)
		    prev_celp_mdct_dec = data_packet.Celp_Mdct_Flag;
		else
		    prev_celp_mdct_dec = 0;

		if (bit_rate != 4 || data_packet.Celp_Mdct_Flag == 0) {
		    for (int j = 0; j < 160; j++)
			prev_qmdct[j] = 0.0;
		}


		if (data_packet.WB_MODE_BIT == 1) {

		    if (bit_rate == 1)
			ER_counter++;
		    else
			ER_counter = 0;

		    if (bit_rate != 2) {
			attn_fac = 1.0;
		    }
		}

		/* Add fading when there is no erasure */
		FadeScale = MIN (1.0, FadeScale + 0.6);	//CELP erasure routine is using this FadeScale

		//Back up this frame for potential pitch correction in case of erasures
		if (fer_counter == 2) {
		    pdelayD3 = pdelayD2;
		    for (i = 0; i < ACBMemSize; i++)
			PitchMemoryD3[i] = PitchMemoryD2[i];
		}
		if (data_packet.WB_MODE_BIT == 1) {

		}
		else {

		    if (update_bbg_flag == 1)	//not suppressed eighth rate
		    {
			update_bbg_estimate (outFbuf, PACKET_RATE, 1.0);
		    }

		}
	    }			//!BAD_RATE
	    else {
		if (data_packet.WB_MODE_BIT == 0) {

		    // bad-rate detecte
		    N_consec_ers++;
		    PACKET_RATE = data_packet.PACKET_RATE = 0xE;	//erasure , bug-fix in the way the next frame is processed
		    if (lastrateD != 3)
			prev_rcelp_half = 0;
		    if (prev_rcelp_half == 1) {
			lastrateD = 4;
			prev_rcelp_half = 0;
		    }
		}
		fer_processing (outFbuf, post_filter, bit_rate = lastrateD);
	    }			//BAD_RATE
	}			//bad rate check for speech, decoding and other updates
    }				//end of no erasure case

    /* update decoder varaibles */
    for (i = 0; i < ACBMemSize; i++)
	prevpD[i] = PitchMemoryD[i];
    for (i = 0; i < LPCORDER; i++) {
	OldOldlspD[i] = OldlspD[i];
	OldlspD[i] = lsp[i];
    }
    lastlastrateD = lastrateD;




    prev_rcelp_half = rcelp_half_rateD;
    if (!BAD_RATE)
	LAST_PACKET_RATE_D = PACKET_RATE;
    else {
	data_packet.PACKET_RATE = PACKET_RATE = 14;
	LAST_PACKET_RATE_D = 14;	//similar to the rate=0xE case
    }



    LAST_MODE_BIT_D = MODE_BIT;
//  decode_fcnt++;
    prev_prev_frame_error = prev_frame_error;
    prev_frame_error = (rate == 0xE || errorFlag == 1 || (BAD_RATE == 1));

//printf("%d\n",data_packet.PACKET_RATE);
    if (data_packet.WB_MODE_BIT == 1) {

    }




}
