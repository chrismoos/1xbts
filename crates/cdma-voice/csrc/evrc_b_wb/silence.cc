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
    "$Id: silence.cc,v 1.38.6.5 2007/06/24 06:43:18 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/* This is the original EVRC eighth rate coder */
/* separated from the main routine encode.c */

#include  <stdio.h>
#include  <stdlib.h>

#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "struct.h"





void
FGV_MEM::silence_encoder ()
{
    register int i, j;
    short subframesize;
    float sum1;

    // Gain Quantization Setups
    FCBGainSize = 16;		/*...use half-rate gain quantizer... */
    gnvq = gnvq_4;

    // LSP Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (write_accshift) {
	//dummy print out in silence encoder, doesnt really do RCELP, but is called
	//so I need to print out accshift here to maintian frame alignment

	fprintf (stdout, "%f\n", accshift);
	fprintf (stdout, "%f\n", accshift);
	fprintf (stdout, "%f\n", accshift);
	frame_shift[0] = accshift;
	frame_shift[1] = accshift;
	frame_shift[2] = accshift;

    }



    if (data_packet.WB_MODE_BIT == 1) {

	enc_lsp_vq_10 (lsp_nq, SScratch, lsp);

    }
    else {			//narroawband case

	lspmaq (lsp_nq, ORDER, 1, 2, nsub8, nsize8, 0.5, lsp, SScratch,
		bit_rate, lsptab8);

    }

    for (i = 0; i < ORDER; i++)
	cbprevprev_E[i] = cbprev_E[i] = lsp[i];

    for (i = 0; i < 2; i++) {
	data_packet.LSP_IDX[i] = SScratch[i];
    }



    float Htemp[54], lpc_gain;
    static float prev_lpc_gain = 0;

    lsp2a (pci, lsp_nq, ORDER);
    for (j = 0; j < 54; j++)
	Htemp[j] = 0.0;
    ImpulseRzp (Htemp, pci, pci, 1.0, 1.0, ORDER, 54);
    for (j = 0, lpc_gain = 0; j < Hlength; j++)
	lpc_gain += Htemp[j] * Htemp[j];
    noSID = 0;
    if ((lastrateE == 1) && ((10 * log10 (lpc_gain) - prev_lpc_gain) > 0.65))
	noSID = 1;
    lpcgflag = 0;
    if ((lastrateE == 1) && ((10 * log10 (lpc_gain) - prev_lpc_gain) > 0.72))
	lpcgflag = 1;
    prev_lpc_gain = 10 * log10 (lpc_gain);









    /* copied quantized LSPs */
    // extern float global_lsp[ORDER];
    for (j = 0; j < ORDER; j++)
	global_lsp[j] = lsp[j];

    if (args->unquantized_lsp) {
	for (i = 0; i < ORDER; i++)
	    global_lsp[i] = lsp_nq[i];
	for (i = 0; i < ORDER; i++)
	    lsp[i] = lsp_nq[i];
    }

    if (data_packet.WB_MODE_BIT == 1) {
	/* Compute residual with quantized LSPs */
	/* Get residual signal */
	for (j = 0; j < ORDER; j++)
	    Scratch[j] = 0;	/* Scratch is used as filter memory */

	lsp2a (pci, OldlspE, ORDER);

	GetResidual (residual, HPspeech, pci, Scratch, ORDER, GUARD);

	for (i = 0; i < NoOfSubFrames; i++) {

	    Interpol (lspi, OldlspE, lsp, i, ORDER);
	    lsp2a (pci, lspi, ORDER);
	    if (i < 2)
		GetResidual (residual + i * (SubFrameSize - 1) + GUARD,
			     HPspeech + i * (SubFrameSize - 1) + GUARD, pci,
			     Scratch, ORDER, SubFrameSize - 1);
	    else
		GetResidual (residual + i * (SubFrameSize - 1) + GUARD,
			     HPspeech + i * (SubFrameSize - 1) + GUARD, pci,
			     Scratch, ORDER, SubFrameSize);
	}

	lsp2a (pci, lsp, ORDER);
	GetResidual (residual + FrameSize + GUARD,
		     HPspeech + FrameSize + GUARD, pci, Scratch, ORDER,
		     GUARD);

    }
    // Start Silence Processing

    for (i = 0; i < FrameSize; i++)
	copy[i] = residual[i + GUARD];

    if (args->mform_res_out)
	write_samples (1, residual + GUARD, FrameSize);

    /* Reset accumulated shift */
    accshift = 0;
    dpm = 0;

    for (i = 0; i < NoOfSubFrames; i++) {

	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	/* interpolate lsp */

	Interpol (lspi, OldlspE, lsp, i, ORDER);
	Interpol (lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);

	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);
	lsp2a (pci_nq, lspi_nq, ORDER);

	/* JH -- Update filter memory */
	SynthesisFilter (origm, residual + GUARD + i * (SubFrameSize - 1),
			 pci, SynMemoryM, ORDER, subframesize);

	if (args->target_speech_out)
	    write_samples (1, origm, subframesize);

	/* Weighting filter */
	if (data_packet.WB_MODE_BIT == 1)
	    weight (wpci, pci_nq, GAMMA1, ORDER);
	else
	    weight (wpci, pci_nq, GAMMA1_NB, ORDER);

	fir (Scratch, origm, wpci, WFmemFIR, ORDER, subframesize);
	weight (wpci, pci_nq, GAMMA2, ORDER);
	iir (worigm, Scratch, wpci, WFmemIIR, ORDER, subframesize);

	if (data_packet.WB_MODE_BIT == 1) {


#   define MAX_LOG_ENERGY_WB	(2.0*6.0)	//(4.0)  // put these in defines.h
#   define MIN_LOG_ENERGY_WB	(2.0*1.0)	//(-0.1)
#   define LOG_ZERO_ENERGY_WB	(2.0*-1.0)	//(-2.0)

#   define WB_DUMP_EIGHTH_RATE_ENERGY_PARAMS 0

	    if (i == 0) {
		float sum;
		float Eres = 0.0, D, tmp;
		int k;

		for (j = 0; j < FrameSize; j++) {
		    //Eres += residual[GUARD+j] * residual[GUARD+j];
		    Eres += HPspeech[GUARD + j] * HPspeech[GUARD + j];
		}
		//Eres = (1./sqrt(2.0)) * Eres;  // Quantize energy biased 1.5 dB down
		//Eres = 0.5 * Eres;  // Quantize energy biased 3 dB down
		Eres = 0.25 * Eres;	// Quantize energy biased 6 dB down
		//Eres = log(sqrt((Eres+1.0)/FrameSize));
		Eres = log ((Eres + 1.0) / FrameSize);

		/* Quantize frame energy */
		for (k = 0, sum = 1e30; k < NUM_Q_LEVELS; k++) {
		    D = Eres - (MIN_LOG_ENERGY_WB +
				(float) k * (MAX_LOG_ENERGY_WB -
					     MIN_LOG_ENERGY_WB) /
				NUM_Q_LEVELS);
		    tmp = D * D;
		    if (tmp < sum) {
			sum = tmp;
			idxcbg = k;
		    }
		}

		for (k = 0; k < ACBMemSize + SubFrameSize; k++) {
		    Excitation[k] = 0.0;
		}

		if (data_packet.WB_MODE_BIT == 1)
		    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize,
			       GAMMA1, GAMMA2, ORDER, subframesize, 1);
		else
		    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize,
			       GAMMA1_NB, GAMMA2, ORDER, subframesize, 1);
	    }


	}
	else {			//narrowband case

	    /* Get lpc gain */
	    /* Calculate impulse response of 1/A(z) */
	    ImpulseRzp (H, pci, pci, 1.0, 1.0, ORDER, Hlength);

	    sum1 = pow (10.0, FIXED_EIGHTH_RATE_ATTEN_AMT / 20.0);

	    if (lastrateE != 1 && i == 0 && encode_fcnt == 0)
		j = 0;		/* Reset seed */
	    else
		j = 1;

	    GetExc800bps (Excitation, &idxcbg, sum1,
			  residual + GUARD + i * (SubFrameSize - 1),
			  subframesize, j, i);

	    /*...another puff fix... */
	    ZeroInput (zir, pci_nq, pci,
		       Excitation + ACBMemSize - subframesize, GAMMA1_NB,
		       GAMMA2, ORDER, subframesize, 1);

	}


	if (data_packet.WB_MODE_BIT == 1)
	    for (j = 0; j < subframesize; j++)
		P_LB.whtnd_la[i * (SubFrameSize - 1) + j] = 0;
    }

    /* TJ Added 3/8/96 -- Trap for all ones rate 1/8 packet */
    if ((SScratch[0] & SScratch[1] & 0xf) == 0xf && (idxcbg == 0xff)) {
	/* Clear Frame Energy Gain MSB if output packet == all ones */
	idxcbg = 0x7f;
    }

    data_packet.SILENCE_GAIN = idxcbg;

    data_packet.PACKET_RATE = 1;
    data_packet.MODE_BIT = 0;


}

void
FGV_MEM::silence_decoder (float *outFbuf, short post_filter)
{
    register int i, j;
    register float *foutP;
    float delayi[3];
    short subframesize;
    float lsp_store_temp[ORDER];

    // Gain Quantization Setups
    FCBGainSize = 16;		/*...use half-rate gain quantizer... */
    gnvq = gnvq_4;

    // LSP Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (data_packet.WB_MODE_BIT == 1) {

	dec_lsp_vq_10 ((short int *) data_packet.LSP_IDX, lsp);

    }
    else {			//narrowband case

	lspmaq_dec (ORDER, 1, 2, nsub8, nsize8, lsp,
		    (short int *) data_packet.LSP_IDX, 1, lsptab8);

    }

    /* Check for monontonic LSP */
    for (j = 1; j < ORDER; j++)
	if (lsp[j] <= lsp[j - 1])
	    errorFlag = 1;

    for (i = 0; i < ORDER; i++)
	cbprevprev_D[i] = cbprev_D[i] = lsp[i];

    if (data_packet.WB_MODE_BIT == 0) {

	lsp_spread (lsp);

    }


    if (args->vad_out) {	//overloading vad_out for acb write out
	//dummy print out, no ACB in nelp and silence
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
    }

    PitchGain = 0.0;		//Populate the external variable PitchGain for the Wideband analysis. 0 in this case
    // extern float global_lsp[ORDER];
    if (args->unquantized_lsp)
	for (i = 0; i < ORDER; i++)
	    lsp[i] = global_lsp[i];

    // Do Silence Decoding

    delayi[0] = (float) (DMIN);
    delayi[1] = (float) (DMIN);
    delayi[2] = (float) (DMIN);

    idxcbg = data_packet.SILENCE_GAIN;

    foutP = outFbuf;


    float dmp[160];

    if (data_packet.WB_MODE_BIT == 1)
	ER_exct_nrg_avg = 0.0;


    for (i = 0; i < NoOfSubFrames; i++) {

	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	if (data_packet.WB_MODE_BIT == 1) {

	    float sum;

	    float sum_bbg;

	    static long decode_fcnt_last;


	    if (idxcbg == 0) {
		sum = LOG_ZERO_ENERGY_WB;
	    }
	    else {
		sum =
		    MIN_LOG_ENERGY_WB + (float) idxcbg *(MAX_LOG_ENERGY_WB -
							 MIN_LOG_ENERGY_WB) /
		    NUM_Q_LEVELS;
	    }

	    sum = exp (sum);

	    if (i == 0 && (decode_fcnt - decode_fcnt_last > 6)
		|| (decode_fcnt < 6) || sum_sm > sum) {

		sum_sm = sum;
	    }
	    else {

		sum_sm = 0.97 * sum_sm + 0.03 * (0.4 * 0.4 * sum);
	    }

	    sum_bbg = sum;

	    sum = sum_sm;


	    sum = sqrt (sum);

	    sum_bbg = sqrt (sum_bbg);


	    for (j = 0; j < subframesize; j++) {
		PitchMemoryD[ACBMemSize + j] = ran_g (&silence_Seed);

		PitchMemoryD_bbg[ACBMemSize + j] =
		    PitchMemoryD[ACBMemSize + j];

	    }

	    /* Linear interpolation of lsp */

	    Interpol (lspi, OldlspD, lsp, i, ORDER);


	    {
		static int first = 1;

		if (first) {
		    //if (lastrateD != 1 && i == 0) {
		    first = 0;
		    for (j = 0; j < ORDER; j++) {
			lspi_sm[j] = lspi[j];
		    }
		}
		else {
		    //printf("%d ",decode_fcnt);
		    for (j = 0; j < ORDER; j++) {
			lspi_sm[j] = 0.9 * lspi_sm[j] + 0.1 * lspi[j];
			lspi[j] = lspi_sm[j];
		    }
		}
	    }

	    /* Convert lsp to PC */
	    lsp2a (pci, lspi, ORDER);


	    float h[Hlength];

	    ImpulseRzp1 (h, pci, pci, 0.0f, 0.0f, ORDER, Hlength);
	    float gain = 1.0;

	    for (j = 1; j < Hlength; j++) {
		gain += h[j] * h[j];
	    }
	    sum /= sqrt (gain);

	    sum_bbg /= sqrt (gain);


	    for (j = 0; j < subframesize; j++) {
		PitchMemoryD[ACBMemSize + j] *= sum;

		PitchMemoryD_bbg[ACBMemSize + j] *= sum_bbg;

	    }

	    for (j = 0; j < ACBMemSize; j++) {
		PitchMemoryD[j] = 0.0;
	    }
	    decode_fcnt_last = decode_fcnt;


	}
	else {

	    if (lastrateD != 1 && i == 0 && decode_fcnt == 0)
		j = 0;		/* Reset seed */
	    else
		j = 1;

	    GetExc800bps_dec (PitchMemoryD + ACBMemSize, subframesize, idxcbg,
			      j, i);

	    if (args->unquantized_zero_rate)
		for (j = 0; j < subframesize; j++)
		    PitchMemoryD[ACBMemSize + j] =
			copy[i * (SubFrameSize - 1) + j];

	    for (j = 0; j < ACBMemSize; j++)
		PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	    for (j = 0; j < ACBMemSize; j++)
		PitchPreFiltMemoryD[j] = PitchMemoryD[j];


	}

	if (data_packet.WB_MODE_BIT == 1) {




	}
	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);

	if (data_packet.WB_MODE_BIT == 1) {




	    for (j = 0; j < subframesize; j++)
		LB_Excitation[i * (SubFrameSize - 1) + j] =
		    PitchMemoryD[j + ACBMemSize];



	}




	if (data_packet.WB_MODE_BIT == 1) {

	    for (j = 0; j < subframesize; j++)
		dmp[i * (SubFrameSize - 1) + j] =
		    PitchMemoryD[j + ACBMemSize];



	    /* Synthesis of decoder output signal and postfilter output signal */

	    SynthesisFilter (DECspeech_bbg, PitchMemoryD_bbg + ACBMemSize,
			     pci, SynMemory_bbg, ORDER, subframesize);

	    if (update_bbg_flag) {

		float decoutenergy = 0;

		for (j = 0; j < subframesize; j++) {
		    /* Avoid synthesizing DECspeech_bbg by using excitation energy
		     * parameter combined with h[n]^2 (h[n] is impulse response of
		     * lpc filter) */
		    decoutenergy += DECspeech_bbg[j] * DECspeech_bbg[j];
		}
		update_bbg_estimate2 (pci, decoutenergy);

	    }


	}
	else {			//narrowband case

	    /* Linear interpolation of lsp */
	    Interpol (lspi, OldlspD, lsp, i, ORDER);

	    /* Convert lsp to PC */
	    lsp2a (pci, lspi, ORDER);


	}

	if (!args->dtx || !update_bbg_reenc_flag) {


	    SynthesisFilter (DECspeech, PitchMemoryD + ACBMemSize, pci,
			     SynMemory, ORDER, subframesize);
	    if (post_filter) {
		Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			     (delayi[0] + delayi[1]) / 2.0, 0.0, 0.0,
			     subframesize);
	    }
	    else
		V_copy (DECspeech, DECspeechPF, subframesize);

	    /* Write decoder output and variables to files */
	    for (j = 0; j < subframesize; j++) {
		if (DECspeechPF[j] > 32767.0)
		    *foutP++ = 32767.0;
		else {
		    if (DECspeechPF[j] < -32768)
			*foutP++ = -32768.0;
		    else
			*foutP++ = DECspeechPF[j];
		}
	    }


	}

    }

    if (data_packet.WB_MODE_BIT == 1) {

	for (j = 0, ER_exct_nrg_avg = 0.0; j < 160; j++)
	    ER_exct_nrg_avg += dmp[j] * dmp[j];

    }





}

void
FGV_MEM::silence_erasure_decoder (float *outFbuf, short post_filter)
{
    register int i, j;
    register float *foutP;
    float delayi[3];
    short subframesize;


    if (args->vad_out) {	//overloading vad_out for acb write out
	//dummy print out, no ACB in nelp and silence
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
    }



    PitchGain = 0.0;		//Populate the external variable PitchGain for the Wideband analysis. 0 in this case
    delayi[0] = (float) (DMIN);
    delayi[1] = (float) (DMIN);
    delayi[2] = (float) (DMIN);

    foutP = outFbuf;

    //static long silence_erasure_seed=0;
    int N = SubFrameSize;
    double G0, G1, G2, Gain;

    for (i = 0, G2 = 1; i < N; i++)
	G2 += SQR (PitchMemoryD[ACBMemSize - i]);
    G2 = sqrt (G2 / N);
    for (i = 0, G1 = 1; i < N - 1; i++)
	G1 += SQR (PitchMemoryD[ACBMemSize - N - i]);
    G1 = sqrt (G1 / (N - 1));
    for (i = 0, G0 = 1; i < ACBMemSize - (2 * N - 1); i++)
	G0 += SQR (PitchMemoryD[i]);
    G0 = sqrt (G0 / (ACBMemSize - (2 * N - 1)));
    Gain = pow (10., (log10 (G0) + log10 (G1) + log10 (G2)) / 3.);

    /* Convert lsp to PC */
    if (data_packet.WB_MODE_BIT == 1) {

	lsp2a (pci, lspi_sm, ORDER);



    }
    else {
	lsp2a (pci, OldlspD, ORDER);
    }


    for (i = 0; i < NoOfSubFrames; i++) {

	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	Interpol (lspi, OldlspD, OldlspD, i, ORDER);
	if (data_packet.WB_MODE_BIT == 1) {

	    float h[Hlength];

	    ImpulseRzp1 (h, pci, pci, 0.0f, 0.0f, ORDER, Hlength);
	    float gain = 1.0;

	    for (j = 1; j < Hlength; j++) {
		gain += h[j] * h[j];
	    }
	    gain = sqrt (sum_sm / gain);

	    for (j = 0; j < ACBMemSize; j++) {
		PitchMemoryD[j] = 0.0;
	    }
	    for (j = 0; j < subframesize; j++) {
		PitchMemoryD[ACBMemSize + j] = gain * ran_g (&silence_Seed);
	    }


	}
	else {
	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[j + ACBMemSize] =
		    Gain * ran_g (&silence_erasure_seed);

	    if (args->unquantized_zero_rate) {	/*Do Nothing */
	    }

	    for (j = 0; j < ACBMemSize; j++)
		PitchMemoryD[j] = PitchMemoryD[j + subframesize];


	}


	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);
	if (data_packet.WB_MODE_BIT == 1)
	    for (j = 0; j < subframesize; j++)
		LB_Excitation[i * (SubFrameSize - 1) + j] = 0.0;


	/* Synthesis of decoder output signal and postfilter output signal */
	SynthesisFilter (DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory,
			 ORDER, subframesize);
	if (post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 (delayi[0] + delayi[1]) / 2.0, 0.0, 0.0,
			 subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);

	/* Write decoder output and variables to files */
	for (j = 0; j < subframesize; j++) {
	    if (DECspeechPF[j] > 32767.0)
		*foutP++ = 32767.0;
	    else {
		if (DECspeechPF[j] < -32768)
		    *foutP++ = -32768.0;
		else
		    *foutP++ = DECspeechPF[j];
	    }
	}
    }
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     getext1k.c                                              */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     12/01/94  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/****************************************************************************
* Routine name: GetExc800bps.                                               *
* Function: Energy quantization of the residual signal.                     *
* Inputs: input - signal array.                                             *
*         length - size of signal array.                                    *
* Output: output - quantized signal.                                        *
****************************************************************************/

#define IA 16807
#define IM 2147483647
#define AM (1.0/IM)
#define IQ 127773
#define IR 2836
#define MASK1 23148373

float
ran0 (long *seed0)
{
    long k;
    float ans;

    *seed0 ^= MASK1;
    k = (*seed0) / IQ;
    *seed0 = IA * (*seed0 - k * IQ) - IR * k;
    if (*seed0 < 0)
	*seed0 += IM;
    ans = AM * (*seed0);
    *seed0 ^= MASK1;
    return (ans);
}

float
FGV_MEM::ran_g (long *seed0)
{
    //static int iset = 0;
    //static float gset;
    float fac, rsq, v1, v2;

//        printf(" ISET is %d\n",iset);
    if (iset == 0) {
	do {
	    v1 = 2.0 * ran0 (seed0) - 1.0;
	    v2 = 2.0 * ran0 (seed0) - 1.0;
	    rsq = v1 * v1 + v2 * v2;
	}
	while (rsq >= 1.0 || rsq == 0.0);
	fac = sqrt (-2.0 * log (rsq) / rsq);
	gset = v1 * fac;
	iset = 1;
	return (v2 * fac);
    }
    else {
	iset = 0;
	return (gset);
    }
}




#define	MAX_LOG_ENERGY	(2.6)	/* set to about 2.1 for is-127 equivalent */
#define	MIN_LOG_ENERGY	(0.2)
#define   LOG_ZERO_ENERGY	(-2.0)

void
FGV_MEM::GetExc800bps (float *output, short *best, float scale, float *input,
		       short length, short flag, short n)
{

    short i, j;
    float sum, tmp, D, *ptr;

    //static long GetExc800bps_Seed;
    //static float GetExc800bps_Sum[NoOfSubFrames];

    float maxSFEnergy, maxSFEnergyQ;

    //static float maxSFEnergyQLast;
    if (data_packet.WB_MODE_BIT == 1) {

    }

    if (!flag)
	GetExc800bps_Seed = 1234;


    /* Get energy of next sub frame */
    for (i = 0, sum = 0; i < length; i++)
	sum += fabs (input[i]);

    sum = sum / (float) SubFrameSize;
    if (sum < 1)
	sum = 1;

    sum = log10 (sum / scale);	/*...fix for noise puffs... */

    GetExc800bps_Sum[n] = sum;

    /* Quantize if last frame */
    if (n == NoOfSubFrames - 1) {
	maxSFEnergy = GetExc800bps_Sum[0];
	for (j = 1; j < 3; j++)
	    if (GetExc800bps_Sum[j] > maxSFEnergy)
		maxSFEnergy = GetExc800bps_Sum[j];


	if (data_packet.WB_MODE_BIT == 1) {
	    /* Quantize to 8 bits */
	    for (i = 0, sum = 1e30; i < NUM_Q_LEVELS; i++) {
		D = maxSFEnergy - (MIN_LOG_ENERGY +
				   (float) i * (MAX_LOG_ENERGY -
						MIN_LOG_ENERGY) /
				   NUM_Q_LEVELS);
		tmp = D * D;
		if (tmp < sum) {
		    sum = tmp;
		    *best = i;
		}
	    }
	    maxSFEnergyQ =
		MIN_LOG_ENERGY + *best * (MAX_LOG_ENERGY -
					  MIN_LOG_ENERGY) / NUM_Q_LEVELS;
	}
	else {
	    // Quantize to 8 bits 
	    for (i = 0, sum = 1e30; i < NUM_Q_LEVELS_NB; i++) {
		D = maxSFEnergy - (MIN_LOG_ENERGY +
				   (float) i * (MAX_LOG_ENERGY -
						MIN_LOG_ENERGY) /
				   NUM_Q_LEVELS_NB);
		tmp = D * D;
		if (tmp < sum) {
		    sum = tmp;
		    *best = i;
		}
	    }
	    maxSFEnergyQ =
		MIN_LOG_ENERGY + *best * (MAX_LOG_ENERGY -
					  MIN_LOG_ENERGY) / NUM_Q_LEVELS_NB;

	}



	if (*best == 0)
	    maxSFEnergyQ = LOG_ZERO_ENERGY;
	//fprintf (stderr, "\r   best = %d, maxSFEnergyQ = %4.2f\n", (int)*best, maxSFEnergyQ);

	/* Interpolate only if last frame was eighth rate */
	if (LAST_PACKET_RATE_E != 1)
	    maxSFEnergyQLast = maxSFEnergyQ;

	/* Interpolate using quantized max energy as endpoint */
	GetExc800bps_Sum[0] = 0.667 * maxSFEnergyQLast + 0.333 * maxSFEnergyQ;
	GetExc800bps_Sum[1] = 0.333 * maxSFEnergyQLast + 0.667 * maxSFEnergyQ;
	GetExc800bps_Sum[2] = maxSFEnergyQ;
	maxSFEnergyQLast = maxSFEnergyQ;

	/* Convert to linear domain */
	for (j = 0; j < 3; j++)
	    GetExc800bps_Sum[j] = pow (10.0, double (GetExc800bps_Sum[j]));

	/* Get excitation */
	j = FrameSize - ACBMemSize;


	for (i = 0; i < FrameSize - 1; i++) {
	    tmp = ran_g (&GetExc800bps_Seed);
	    if (i >= j)
		output[i - j] = GetExc800bps_Sum[i / (length - 1)] * tmp;
	}
	output[i - j] = GetExc800bps_Sum[2] * ran_g (&GetExc800bps_Seed);	/* last excitation */


    }


}

void
FGV_MEM::GetExc800bps_dec (float *output, short length, short best,
			   short flag, short n)
{
    short i, j;
    float sum;

    int fer_flag = 0;		// This was passed in
    float maxSFEnergyQ;

    //static float maxSFEnergyQLast_dec;

    if (!flag && !n)
	GetExc800bps_dec_Seed = 1234;

    if (n == 0) {

	/* De-quantize */
	if (fer_flag == 0) {

	    //best /= 2;
	    if (data_packet.WB_MODE_BIT == 1)
		maxSFEnergyQ =
		    MIN_LOG_ENERGY + (float) best *(MAX_LOG_ENERGY -
						    MIN_LOG_ENERGY) /
		NUM_Q_LEVELS;
	    else
	    maxSFEnergyQ =
		MIN_LOG_ENERGY + (float) best *(MAX_LOG_ENERGY -
						MIN_LOG_ENERGY) /
		NUM_Q_LEVELS_NB;
	    if (data_packet.WB_MODE_BIT == 1) {


		if (best == 0)
		    maxSFEnergyQ = LOG_ZERO_ENERGY;

	    }
	    else {
		if (best == 0)
		    maxSFEnergyQ = LOG_ZERO_ENERGY;

	    }

	    /* Interpolate only if last frame was eighth rate */
	    if (LAST_PACKET_RATE_D != 1)
		maxSFEnergyQLast_dec = maxSFEnergyQ;
	    if (data_packet.WB_MODE_BIT == 1) {

		GetExc800bps_dec_Sum[0] =
		    0.667 * maxSFEnergyQLast_dec + 0.333 * maxSFEnergyQ;
		GetExc800bps_dec_Sum[1] =
		    0.333 * maxSFEnergyQLast_dec + 0.667 * maxSFEnergyQ;
		GetExc800bps_dec_Sum[2] = maxSFEnergyQ;
		maxSFEnergyQLast_dec = maxSFEnergyQ;
		PrevBest = best;

	    }
	    else {		//narrowband case

		GetExc800bps_dec_Sum[0] =
		    0.667 * maxSFEnergyQLast_dec + 0.333 * maxSFEnergyQ;
		GetExc800bps_dec_Sum[1] =
		    0.333 * maxSFEnergyQLast_dec + 0.667 * maxSFEnergyQ;
		GetExc800bps_dec_Sum[2] = maxSFEnergyQ;
		maxSFEnergyQLast_dec = maxSFEnergyQ;
		PrevBest = best;

	    }


	    /* Interpolate using quantized max energy as endpoint */
	}
	else {
	    if (data_packet.WB_MODE_BIT == 1)
		for (j = 0; j < 3; j++)
		    GetExc800bps_dec_Sum[j] =
			MIN_LOG_ENERGY + (float) PrevBest *(MAX_LOG_ENERGY -
							    MIN_LOG_ENERGY) /
		NUM_Q_LEVELS;
	    else
	    for (j = 0; j < 3; j++)
		GetExc800bps_dec_Sum[j] =
		    MIN_LOG_ENERGY + (float) PrevBest *(MAX_LOG_ENERGY -
							MIN_LOG_ENERGY) /
		NUM_Q_LEVELS_NB;

	}

    }

    /* Convert to linear domain */
    sum = pow (10.0, double (GetExc800bps_dec_Sum[n]));

    if (data_packet.WB_MODE_BIT == 1) {

	for (i = 0; i < length; i++) {
	    output[i] = sum * ran_g (&GetExc800bps_dec_Seed);
	}


    }
    else {			//narrowband case
	for (i = 0; i < length; i++) {
	    output[i] = 0.656 * sum * ran_g (&GetExc800bps_dec_Seed);
	}
	//Note : The above reduction by 0.656 is done to compensate for the difference in energy between the FL and FX random number generators, this ensures the FL and FX decoders match in terms of energy of eighth-rate frames for the same packet input

    }
}
