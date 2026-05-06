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
    "$Id: nelp.cc,v 1.39.4.4 2007/06/24 06:43:15 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <iostream>
using namespace std;

#include "defines.h"
#include "filt.h"
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "struct.h"

float unq_Gains[10];



//freqz(0.835*[1, -1.999, 1], [1, -1.66, .72], 2^11, 8e3); zoom on;

static float bp1_num_coef_wb[13] =
    { 0.85, -1.69915, 0.85, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
0.0 };

static float bp1_den_coef_wb[13] =
    { 1, -1.715, 0.76, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0 };


static float bp1_num_coef[13] = {
    0.33935546875000, 0, -2.03564453125000, 0, 5.08886718750000, 0,
	-6.78515625000000,
    0, 5.08886718750000, 0, -2.03564453125000, 0, 0.33935546875000
};

static float bp1_den_coef[13] = { 1,
    -0.43505859375000, -3.78784179687500, 1.28784179687500, 6.33081054687500,
    -1.62207031250000, -5.86523437500000, 1.06396484375000, 3.14868164062500,
    -0.36035156250000, -0.92333984375000, 0.05004882812500, 0.11499023437500
};


static float shape1_num_coef[11] = {
    0.959381103515625, -0.074554443359375, -0.4161376953125, 0.1317138671875,
    -0.3109130859375, 0.00146484375, 0.080535888671875, -0.109100341796875,
    -0.023681640625, 0.03192138671875, 0.012176513671875
};

static float shape1_den_coef[11] = { 1,
    0.0897216796875, -0.373443603515625, 0.123046875, -0.293243408203125,
    -0.06097412109375, 0.071258544921875, -0.1190185546875,
	-0.048675537109375,
    0.026153564453125, 0.007720947265625
};

static float shape2_num_coef[11] = {
    0.938720703125, 0.000946044921875, -0.295989990234375, 0.2904052734375,
    -0.17938232421875, -0.221221923828125, -0.3194580078125, 0.01348876953125,
    0.10003662109375, -0.001922607421875, 0.034027099609375
};

static float shape2_den_coef[11] = { 1,
    0.488861083984375, -0.02716064453125, 0.390594482421875, 0.07159423828125,
    -0.213165283203125, -0.402587890625, -0.176849365234375,
	-0.028961181640625,
    -0.0455322265625, -0.00927734375
};

static float shape3_num_coef[11] = {
    0.936431884765625, -0.011688232421875, -0.303253173828125,
	-0.293121337890625,
    -0.183013916015625, 0.232269287109375, -0.317169189453125,
	-0.010833740234375,
    0.098846435546875, 0.0003662109375, 0.0364990234375
};

static float shape3_den_coef[11] = { 1,
    -0.50347900390625, -0.027801513671875, -0.395111083984375,
	0.06976318359375,
    0.22674560546875, -0.408477783203125, 0.18511962890625, -0.03607177734375,
    0.0482177734375, -0.008331298828125
};

static float txlpf1_num_coef[11] = {
    0.016845703125, 0.024169921875, 0.062744140625, 0.0831298828125,
	0.1124267578125,
    0.11767578125, 0.1124267578125, 0.0831298828125, 0.062744140625,
	0.024169921875,
    0.016845703125
};

static float txlpf1_den_coef[11] = { 1,
    -2.3126220703125, 3.8590087890625, -3.8023681640625, 2.989990234375,
    -1.5567626953125, 0.6748046875, -0.17529296875, 0.0423583984375,
	-0.0030517578125,
    0.00048828125
};

static float txhpf1_num_coef[11] = {
    0.016845703125, -0.024169921875, 0.062744140625, -0.0831298828125,
	0.1124267578125,
    -0.11767578125, 0.1124267578125, -0.0831298828125, 0.062744140625,
	-0.024169921875,
    0.016845703125
};

static float txhpf1_den_coef[11] = { 1,
    2.3126220703125, 3.8590087890625, 3.8023681640625, 2.989990234375,
	1.5567626953125,
    0.6748046875, 0.17529296875, 0.0423583984375, 0.0030517578125,
	0.00048828125
};





static float rxlpf_num_coef[7] =
    { 0.59994581615663, 3.53683594633538, 8.74948168164726, 11.62517663542182,
8.74948168164726, 3.53683594633538, 0.59994581615663 };
static float rxlpf_den_coef[7] =
    { 1.0, 4.88981078104793, 10.11919486460807, 11.32491170693722,
7.22004529057673, 2.48380589810452, 0.35993498242596 };

float quantize_uvg (float *G, int &iG1, int *iG2, float *quantG);

void
FGV_MEM::nelp_encoder (float *out)
{
    int i, j, k;
    short subframesize;
    float *ptr = out;
    float *in = residual + GUARD;

    int lag = 16;
    float Gains[10];
    float snr;
    int iG1, iG2[2], fid;
    short packet_data;

    float E1, E2, E3, R, EL1, EH1, EL2, EH2, RL, RH;
    float filtRes[FSIZE];
    float Gnorm[10], Grms;

    if (write_accshift) {
	//dummy print out in silence encoder, doesnt really do RCELP, but is called
	//so I need to print out accshift here to maintian frame alignment
	fprintf (stdout, "%f\n", accshift);
	fprintf (stdout, "%f\n", accshift);
	fprintf (stdout, "%f\n", accshift);
/* UB specific stuff */
	frame_shift[0] = accshift;
	frame_shift[1] = accshift;
	frame_shift[2] = accshift;
    }


    if (lastrateE != 2) {
	for (i = 0; i < 12; i++)
	    bp1_filt_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    shape1_filt_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    shape2_filt_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    shape3_filt_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    txlpf1_filt1_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    txlpf1_filt2_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    txhpf1_filt1_mem[i] = 0;
	for (i = 0; i < 10; i++)
	    txhpf1_filt2_mem[i] = 0;
    }

    POLEZERO_FILTER bp1_filter = { 12, 12, 12, 0, bp1_filt_mem };
    POLEZERO_FILTER shape1_filter = { 10, 10, 10, 0, shape1_filt_mem };
    POLEZERO_FILTER shape2_filter = { 10, 10, 10, 0, shape2_filt_mem };
    POLEZERO_FILTER shape3_filter = { 10, 10, 10, 0, shape3_filt_mem };
    POLEZERO_FILTER txlpf1_filter1 = { 10, 10, 10, 0, txlpf1_filt1_mem };
    POLEZERO_FILTER txlpf1_filter2 = { 10, 10, 10, 0, txlpf1_filt2_mem };
    POLEZERO_FILTER txhpf1_filter1 = { 10, 10, 10, 0, txhpf1_filt1_mem };
    POLEZERO_FILTER txhpf1_filter2 = { 10, 10, 10, 0, txhpf1_filt2_mem };

    // LSP Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (data_packet.WB_MODE_BIT == 1) {
	knum = 4;
	enc_lsp_vq_28 (lsp_nq, SScratch, lsp);
	for (i = 0; i < knum; i++)
	    data_packet.LSP_IDX[i] = SScratch[i];
    }
    else {			//narrowband case

	knum = 2;
	enc_lsp_vq_16 (lsp_nq, SScratch, lsp);
	for (i = 0; i < knum; i++)
	    data_packet.LSP_IDX[i] = SScratch[i];


    }
    for (i = 0; i < ORDER; i++)
	cbprevprev_E[i] = cbprev_E[i] = lsp[i];
    for (i = 0; i < knum; i++) {
	data_packet.LSP_IDX[i] = SScratch[i];
    }

    lsp_spread (lsp);



    /* copied quantized LSPs */
    for (j = 0; j < ORDER; j++)
	global_lsp[j] = lsp[j];

    if (args->unquantized_lsp) {
	for (i = 0; i < ORDER; i++)
	    global_lsp[i] = lsp_nq[i];
	for (i = 0; i < ORDER; i++)
	    lsp[i] = lsp_nq[i];
    }
    if (data_packet.WB_MODE_BIT == 1) {
	if (PREV_LSP_FLAG == 1) {
	    for (i = 0; i < LPCORDER; i++) {
		OldOldlspE[i] = OldlspE[i] = lsp[i];
	    }
	}
    }
    /* Compute residual with quantized LSPs */
    /* Get residual signal */
    for (j = 0; j < ORDER; j++)
	Scratch[j] = 0;		/* Scratch is used as filter memory */

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
		 HPspeech + FrameSize + GUARD, pci, Scratch, ORDER, GUARD);

    // Start Unvoiced/NELP Processing

    for (i = 0; i < FrameSize; i++)
	copy[i] = residual[i + GUARD];

    if (args->mform_res_out)
	write_samples (1, residual + GUARD, FrameSize);


    for (i = 0, E1 = 0; i < FSIZE; i++)
	E1 += SQR (in[i]);
    polezero_filter (in, filtRes, FSIZE, txlpf1_num_coef, txlpf1_den_coef,
		     txlpf1_filter1);
    for (i = 0, EL1 = 0; i < FSIZE; i++)
	EL1 += SQR (filtRes[i]);
    polezero_filter (in, filtRes, FSIZE, txhpf1_num_coef, txhpf1_den_coef,
		     txhpf1_filter1);
    for (i = 0, EH1 = 0; i < FSIZE; i++)
	EH1 += SQR (filtRes[i]);

    /* Reset accumulated shift */
    accshift = 0;
    dpm = 0;

    frame_shift[0] = accshift;
    frame_shift[1] = accshift;
    frame_shift[2] = accshift;

    for (i = 0; i < 10; i++) {
	for (j = i * lag, Gains[i] = 0.001; j < (i + 1) * lag; j++)
	    Gains[i] += SQR (in[j]);
	Gains[i] = sqrt (Gains[i] / lag);

    }



    if (data_packet.WB_MODE_BIT == 1) {




	float fdbck, var_dB, tmp;

	if (lastrateE != 2)
	    nelp_gain_mem = Gains[0];
	tmp = 20.0 * (log10 (Gains[0]) - log10 (nelp_gain_mem));
	var_dB = tmp * tmp;
	for (i = 1; i < 10; i++) {
	    tmp = 20.0 * (log10 (Gains[i]) - log10 (Gains[i - 1]));
	    var_dB += tmp * tmp;
	}
	if (lastrateE != 2)
	    var_dB *= 0.111;
	else
	    var_dB *= 0.1;
//plot(t, 1 ./ (1 + exp(0.3*(t-15))))
	fdbck = 0.82 / (1.0 + exp (0.25 * (var_dB - 20.0)));


	for (i = 0; i < 10; i++) {

	    Gains[i] = (1.0 - fdbck) * Gains[i] + fdbck * nelp_gain_mem;

	    nelp_gain_mem = Gains[i];
	}



    }

    snr = quantize_uvg (Gains, iG1, iG2, Gains);

    for (i = 0; i < 10; i++) {
	float tmp[16], tmp1[16], tmpf;
	int k1, k2, I[16], tmpi;

	for (j = 0; j < 16; j++) {
	    tmp[j] = (nelp_enc_seed = 521 * nelp_enc_seed + 259) / 32768.0;
	    tmp1[j] = ABS (tmp[j]);
	    I[j] = j;
	}
	for (k1 = 0; k1 < 15; k1++) {
	    for (k2 = k1 + 1; k2 < 16; k2++) {
		if (tmp1[k2] > tmp1[k1]) {
		    tmpi = I[k2];
		    tmpf = tmp1[k2];
		    tmp1[k2] = tmp1[k1];
		    I[k2] = I[k1];
		    tmp1[k1] = tmpf;
		    I[k1] = tmpi;
		}
	    }
	}
	for (j = 0; j < 4; j++)
	    ptr[i * 16 + I[j]] = Gains[i] * sqrt (3.0) * tmp[I[j]];
	for (; j < 16; j++)
	    ptr[i * 16 + I[j]] = 0;
    }
    if (data_packet.WB_MODE_BIT == 1) {
    }
    if (data_packet.WB_MODE_BIT == 1) {
	polezero_filter (ptr, ptr, FSIZE, bp1_num_coef_wb, bp1_den_coef_wb,
			 bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);
    }
    else {
	polezero_filter (ptr, ptr, FSIZE, bp1_num_coef, bp1_den_coef,
			 bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);

    }

    polezero_filter (ptr, ptr, FSIZE, shape1_num_coef, shape1_den_coef,
		     shape1_filter);

    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);
    R = sqrt (E1 / E2);
    for (i = 0; i < FSIZE; i++)
	filtRes[i] = R * ptr[i];
    polezero_filter (filtRes, filtRes, FSIZE, txlpf1_num_coef,
		     txlpf1_den_coef, txlpf1_filter2);
    for (i = 0, EL2 = 0; i < FSIZE; i++)
	EL2 += SQR (filtRes[i]);
    for (i = 0; i < FSIZE; i++)
	filtRes[i] = R * ptr[i];
    polezero_filter (filtRes, filtRes, FSIZE, txhpf1_num_coef,
		     txhpf1_den_coef, txhpf1_filter2);
    for (i = 0, EH2 = 0; i < FSIZE; i++)
	EH2 += SQR (filtRes[i]);
    RL = 10 * log10 (EL1 / EL2);
    RH = 10 * log10 (EH1 / EH2);

    fid = 0;
    if (RL < -3)
	fid = 1;
    else if (RH < -3)
	fid = 2;
    switch (fid) {
    case 1:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	// filter the residual to desired shape
	polezero_filter (ptr, ptr, FSIZE, shape2_num_coef, shape2_den_coef,
			 shape2_filter);
	break;
    case 2:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	// filter the residual to desired shape
	polezero_filter (ptr, ptr, FSIZE, shape3_num_coef, shape3_den_coef,
			 shape3_filter);
	break;
    default:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	polezero_filter (ptr, filtRes, FSIZE, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	break;
    }
    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);
    R = sqrt (E3 / E2);
    for (i = 0; i < FSIZE; i++)
	ptr[i] *= R;
    sanity_check = R;

    data_packet.NELP_GAIN_IDX[0][0] = iG1;

    data_packet.NELP_GAIN_IDX[1][0] = iG2[0];

    data_packet.NELP_GAIN_IDX[1][1] = iG2[1];

    data_packet.NELP_FID = fid;

    /* Update target filter memory and reconstruction filter memory */
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

	if (data_packet.WB_MODE_BIT == 1)
	    ZeroInput (zir, pci_nq, pci, ptr + i * (SubFrameSize - 1), GAMMA1,
		       GAMMA2, ORDER, subframesize, 1);
	else
	    ZeroInput (zir, pci_nq, pci, ptr + i * (SubFrameSize - 1),
		       GAMMA1_NB, GAMMA2, ORDER, subframesize, 1);


    }

    if (data_packet.WB_MODE_BIT == 1) {
	for (j = 0; j < FSIZE; j++)
	    P_LB.whtnd_la[j] = ptr[j];
    }


    for (j = 0; j < ACBMemSize; j++)
	Excitation[j] = ptr[FrameSize - ACBMemSize + j];

    data_packet.PACKET_RATE = 2;
    data_packet.MODE_BIT = 0;
    if (data_packet.WB_MODE_BIT == 1) {
	for (j = 0; j < ACBMemSize; j++)
	    bufferm[j] = 0.0;
    }

}

void dequantize_uvg (int iG1, int *iG2, float *G);

void
FGV_MEM::nelp_decoder (float *outFbuf, short post_filter,
		       float time_warp_fraction, short *oBuf_len)
{
    register float *foutP;
    float delayi[3];
    short subframesize;

    int i, j, k;
    float *ptr = outFbuf;

    int lag = 16;
    float Gains[10];
    int iG1, iG2[2], fid;
    short packet_data;

    float res_test[160];

    float E2, E3, R;
    float filtRes[FSIZE * 2];	//Increased size of filtRes to support Time-Warping

    //Additions to support Time-Warping
    int NUM_SAMPLES = 160;

    if (data_packet.WB_MODE_BIT == 1) {
	time_warp_fraction = 1;
    }

    if (time_warp_fraction > 1) {
	NUM_SAMPLES = 192;
	LB_delay = 32;
    }
    else if (time_warp_fraction < 1) {
	NUM_SAMPLES = 128;
	LB_delay = 32;
    }

    *oBuf_len = NUM_SAMPLES;

    //printf ("\n NELP warping %d", NUM_SAMPLES);
    //End: Additions to support Time-Warping


    if (args->vad_out) {	//overloading vad_out for acb write out
	//dummy print out, no ACB in nelp and silence
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
    }

    if (data_packet.WB_MODE_BIT == 1)
	PitchGain = 0.0;	//Populate the external variable PitchGain for the Wideband analysis. 0 in this case




    if (lastrateD != 2 || prev_frame_error) {
	for (i = 0; i < 12; i++)
	    bp1_filt_mem_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape1_filt_mem_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape2_filt_mem_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape3_filt_mem_dec[i] = 0;
    }


    POLEZERO_FILTER bp1_filter = { 12, 12, 12, 0, bp1_filt_mem_dec };
    POLEZERO_FILTER shape1_filter = { 10, 10, 10, 0, shape1_filt_mem_dec };
    POLEZERO_FILTER shape2_filter = { 10, 10, 10, 0, shape2_filt_mem_dec };
    POLEZERO_FILTER shape3_filter = { 10, 10, 10, 0, shape3_filt_mem_dec };

    if (data_packet.PACKET_RATE != 2)
	cerr << "Rate_Mode not set to 2 for NELP in Frame #:" << decode_fcnt
	    << "\n";
    if (data_packet.MODE_BIT != 0)
	cerr << "Here1:Rate not set for NELP in Frame #:" << decode_fcnt <<
	    "\n";
    // LSP De-Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (data_packet.WB_MODE_BIT == 1) {
	dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);
    }
    else {
	dec_lsp_vq_16 ((short int *) data_packet.LSP_IDX, lsp);

    }
    /* Check for monontonic LSP */
    for (j = 1; j < ORDER; j++)
	if (lsp[j] <= lsp[j - 1])
	    errorFlag = 1;
    for (i = 0; i < ORDER; i++)
	cbprevprev_D[i] = cbprev_D[i] = lsp[i];

    lsp_spread (lsp);

    //extern float global_lsp[ORDER];
    if (args->unquantized_lsp)
	for (i = 0; i < ORDER; i++)
	    lsp[i] = global_lsp[i];
    if (data_packet.WB_MODE_BIT == 1) {
	if (PREV_LSP_FLAG == 1) {
	    for (i = 0; i < LPCORDER; i++) {
		OldOldlspD[i] = OldlspD[i] = lsp[i];
	    }
	}
    }
    // Do Unvoiced/NELP Decoding

    iG1 = data_packet.NELP_GAIN_IDX[0][0];

    iG2[0] = data_packet.NELP_GAIN_IDX[1][0];

    iG2[1] = data_packet.NELP_GAIN_IDX[1][1];

    fid = data_packet.NELP_FID;

    dequantize_uvg (iG1, iG2, Gains);


    //Time-Warping: Have to generate NUM_SAMPLES/16, but at least 10 since
    //LB_Excitation is to be populated with 160
    int num_iterations;

    num_iterations = NUM_SAMPLES / 16;
    //End: Time-Warping: Have to generate NUM_SAMPLES/16, but at least 10 since
    //LB_Excitation is to be populated with 160

    //Time-Warping: Changed number of loops to NUM_SAMPLES/16 below
    for (i = 0; i < num_iterations; i++) {
	float tmp[16], tmp1[16], tmpf;
	int k1, k2, I[16], tmpi;

	for (j = 0; j < 16; j++) {
	    tmp[j] = (nelp_dec_seed = 521 * nelp_dec_seed + 259) / 32768.0;
	    tmp1[j] = ABS (tmp[j]);
	    I[j] = j;
	}
	if (data_packet.WB_MODE_BIT == 1) {
	}
	for (k1 = 0; k1 < 15; k1++) {
	    for (k2 = k1 + 1; k2 < 16; k2++) {
		if (tmp1[k2] > tmp1[k1]) {
		    tmpi = I[k2];
		    tmpf = tmp1[k2];
		    tmp1[k2] = tmp1[k1];
		    I[k2] = I[k1];
		    tmp1[k1] = tmpf;
		    I[k1] = tmpi;
		}
	    }
	}
	//Time-Warping: Changes to support when NUM_SAMPLES > 160
	if (i < 10) {
	    for (j = 0; j < 4; j++)
		ptr[i * 16 + I[j]] = Gains[i] * sqrt (3.0) * tmp[I[j]];
	}
	else {
	    for (j = 0; j < 4; j++)
		ptr[i * 16 + I[j]] = Gains[9] * sqrt (3.0) * tmp[I[j]];
	}
	//End: Time-Warping: Changes to support when NUM_SAMPLES > 160
	for (; j < 16; j++)
	    ptr[i * 16 + I[j]] = 0;
    }
    if (data_packet.WB_MODE_BIT == 1) {
    }

    //Small change to support Time-Warping: NUM_SAMPLES
    if (data_packet.WB_MODE_BIT == 1) {
	polezero_filter (ptr, ptr, NUM_SAMPLES, bp1_num_coef_wb,
			 bp1_den_coef_wb, bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);
    }
    else {
	polezero_filter (ptr, ptr, NUM_SAMPLES, bp1_num_coef, bp1_den_coef,
			 bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);
    }

    //Small change to support Time-Warping: NUM_SAMPLES
    polezero_filter (ptr, ptr, NUM_SAMPLES, shape1_num_coef, shape1_den_coef,
		     shape1_filter);
    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);

    switch (fid) {
    case 1:
	// Update other filter memory
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, filtRes, NUM_SAMPLES, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	// filter the residual to desired shape
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, ptr, NUM_SAMPLES, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	break;
    case 2:
	// Update other filter memory
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, filtRes, NUM_SAMPLES, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	// filter the residual to desired shape
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, ptr, NUM_SAMPLES, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	break;
    default:
	// Update other filter memory
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, filtRes, NUM_SAMPLES, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	//Small change to support Time-Warping: NUM_SAMPLES
	polezero_filter (ptr, filtRes, NUM_SAMPLES, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	break;
    }
    //Small change to support Time-Warping: NUM_SAMPLES
    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);
    R = sqrt (E3 / E2);
    //Small change to support Time-Warping: NUM_SAMPLES
    for (i = 0; i < FSIZE; i++)
	ptr[i] *= R;
    if (!args->decode_only && sanity_check != R
	&& args->erasure_file == NULL) {
	fprintf (stderr, "Encoder-Decoder Mismatch in NELP\n");
    }

    delayi[0] = (float) (DMIN);
    delayi[1] = (float) (DMIN);
    delayi[2] = (float) (DMIN);

    foutP = outFbuf;

    //More support for Time-Warping
    int temp_samples_count = 0;

    /* Populate the low band excitation to be used by the WB synthesis */
    if (data_packet.WB_MODE_BIT == 1)
	for (j = 0; j < 160; j++)
	    LB_Excitation[j] = ptr[j];

    //End: More support for Time-Warping

    if (data_packet.WB_MODE_BIT == 1) {
	float exct_nrg;

	NASS = 0;

	if ((lastrateD == 1) && (ER_counter > 5)) {

	    NASS = 1;
	    //Small change to support Time-Warping: NUM_SAMPLES
	    for (i = 0, exct_nrg = 0.0; i < NUM_SAMPLES; i++)
		exct_nrg += SQR (ptr[i]);
	    attn_fac = MIN (sqrt (ER_exct_nrg_avg / exct_nrg), 1.0);
	}
	else {
	    attn_fac = 1.0;

	}

    }
    for (i = 0; i < NoOfSubFrames; i++) {
	//Time-Warping: Change in size of subframesize, not always equal to 53 since residual size may not be = 160

	if (NUM_SAMPLES % 3 == 0)
	    subframesize = NUM_SAMPLES / 3;
	else if (NUM_SAMPLES % 3 == 1) {
	    if (i < 2)
		subframesize = NUM_SAMPLES / 3;
	    else
		subframesize = NUM_SAMPLES / 3 + 1;
	}
	else if (NUM_SAMPLES % 3 == 2) {
	    if (i < 1)
		subframesize = NUM_SAMPLES / 3;
	    else
		subframesize = NUM_SAMPLES / 3 + 1;
	}
	//End: Time-Warping: Change in size of subframesize, not always equal to 53

	//Time-Warping: Slight change in call to support smaller/larger number of samples
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] = ptr[temp_samples_count + j];
	//End: Time-Warping: Slight change in call to support smaller/larger number of samples

	/* Populate the low band excitation to be used by the WB synthesis */

	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];

	if (args->unquantized_quarter_rate)
	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[ACBMemSize + j] =
		    copy[i * (SubFrameSize - 1) + j];


	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);





	if (data_packet.WB_MODE_BIT == 1) {

	    if (NASS == 1) {

		for (j = 0; j < ORDER; j++)
		    lspi[j] = (0.9 * OldlspD[j]) + (0.1 * lsp[j]);



	    }
	    else
		Interpol (lspi, OldlspD, lsp, i, ORDER);
	    /* Linear interpolation of lsp */
	}
	else {

	    /* Linear interpolation of lsp */
	    Interpol (lspi, OldlspD, lsp, i, ORDER);

	}

	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);








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
	delay = 0.0;		//added for purify purposes
	if (data_packet.WB_MODE_BIT == 1) {
	    {
		float decoutenergy = 0;

		for (j = 0; j < subframesize; j++) {
		    decoutenergy += DECspeechPF[j] * DECspeechPF[j];
		}
		/* This new update routine works on subframes and uses lpc envelope and
		 * output energy instead of output signal to estimate noise. */
		update_bbg_estimate2 (pci, decoutenergy);
	    }
	    for (j = 0; j < subframesize; j++)
		DECspeechPF[j] = DECspeechPF[j] * attn_fac;

	}

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
	temp_samples_count += subframesize;	//Addition to support Time-Warping
    }

}


void
FGV_MEM::nelp_erasure_decoder (float *outFbuf, short post_filter)
{
    register float *foutP;
    float delayi[3];
    short subframesize;

    int i, j, k;

    //static short seed=0;
    float *ptr = outFbuf;

    int lag = 16;
    double Gain;
    int iG1, iG2[2], fid = 0;
    short packet_data;

    //  float tmp;
    float E2, E3, R;
    float filtRes[FSIZE];

    if (args->vad_out) {	//overloading vad_out for acb write out
	//dummy print out, no ACB in nelp and silence
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
	fprintf (stdout, "%f\n", 0.0);
    }


    PitchGain = 0.0;		//Populate the external variable PitchGain for the Wideband analysis. 0 in this case
    if (!prev_frame_error) {
	for (i = 0; i < 12; i++)
	    bp1_filt_mem_erasure_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape1_filt_mem_erasure_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape2_filt_mem_erasure_dec[i] = 0;
	for (i = 0; i < 10; i++)
	    shape3_filt_mem_erasure_dec[i] = 0;
    }

    POLEZERO_FILTER bp1_filter = { 12, 12, 12, 0, bp1_filt_mem_erasure_dec };
    POLEZERO_FILTER shape1_filter =
	{ 10, 10, 10, 0, shape1_filt_mem_erasure_dec };
    POLEZERO_FILTER shape2_filter =
	{ 10, 10, 10, 0, shape2_filt_mem_erasure_dec };
    POLEZERO_FILTER shape3_filter =
	{ 10, 10, 10, 0, shape3_filt_mem_erasure_dec };


    for (i = 0, Gain = 0; i < SubFrameSize; i++)
	Gain += SQR (PitchMemoryD[ACBMemSize - i]);
    Gain = sqrt (Gain / SubFrameSize);
    Gain *= 0.8;		// Some scale down of energy since it is an erasure

    for (i = 0; i < 10; i++) {

	float tmp[16], tmp1[16], tmpf;
	int k1, k2, I[16], tmpi;

	for (j = 0; j < 16; j++) {
	    tmp[j] = (nelp_erasure_seed =
		      521 * nelp_erasure_seed + 259) / 32768.0;
	    tmp1[j] = ABS (tmp[j]);
	    I[j] = j;
	}
	if (data_packet.WB_MODE_BIT == 1) {
	}
	for (k1 = 0; k1 < 15; k1++) {
	    for (k2 = k1 + 1; k2 < 16; k2++) {
		if (tmp1[k2] > tmp1[k1]) {
		    tmpi = I[k2];
		    tmpf = tmp1[k2];
		    tmp1[k2] = tmp1[k1];
		    I[k2] = I[k1];
		    tmp1[k1] = tmpf;
		    I[k1] = tmpi;
		}
	    }
	}
	for (j = 0; j < 4; j++)
	    ptr[i * 16 + I[j]] = Gain * sqrt (3.0) * tmp[I[j]];
	for (; j < 16; j++)
	    ptr[i * 16 + I[j]] = 0;
    }

    if (data_packet.WB_MODE_BIT == 1) {
	polezero_filter (ptr, ptr, FSIZE, bp1_num_coef_wb, bp1_den_coef_wb,
			 bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);
    }
    else {
	polezero_filter (ptr, ptr, FSIZE, bp1_num_coef, bp1_den_coef,
			 bp1_filter);
	for (i = 0, E3 = 0; i < FSIZE; i++)
	    E3 += SQR (ptr[i]);
    }

    polezero_filter (ptr, ptr, FSIZE, shape1_num_coef, shape1_den_coef,
		     shape1_filter);

    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);

    switch (fid) {
    case 1:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	// filter the residual to desired shape
	polezero_filter (ptr, ptr, FSIZE, shape2_num_coef, shape2_den_coef,
			 shape2_filter);
	break;
    case 2:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	// filter the residual to desired shape
	polezero_filter (ptr, ptr, FSIZE, shape3_num_coef, shape3_den_coef,
			 shape3_filter);
	break;
    default:
	// Update other filter memory
	polezero_filter (ptr, filtRes, FSIZE, shape2_num_coef,
			 shape2_den_coef, shape2_filter);
	polezero_filter (ptr, filtRes, FSIZE, shape3_num_coef,
			 shape3_den_coef, shape3_filter);
	break;
    }
    for (i = 0, E2 = 0; i < FSIZE; i++)
	E2 += SQR (ptr[i]);
    R = sqrt (E3 / E2);
    for (i = 0; i < FSIZE; i++)
	ptr[i] *= R;
    if (!args->decode_only && sanity_check != R
	&& args->erasure_file == NULL) {
	fprintf (stderr, "Encoder-Decoder Mismatch in NELP\n");
    }

    delayi[0] = (float) (DMIN);
    delayi[1] = (float) (DMIN);
    delayi[2] = (float) (DMIN);

    foutP = outFbuf;

    /* Convert lsp to PC */
    lsp2a (pci, OldlspD, ORDER);

    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;
	Interpol (lspi, OldlspD, OldlspD, i, ORDER);
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] = ptr[i * (SubFrameSize - 1) + j];

	if (args->unquantized_quarter_rate) {	/*Do Nothing */
	}

	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];


	/* Populate the low band excitation to be used by the WB synthesis */
	if (data_packet.WB_MODE_BIT == 1)
	    for (j = 0; j < subframesize; j++)
		LB_Excitation[i * (SubFrameSize - 1) + j] =
		    PitchMemoryD[ACBMemSize + j];

	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);




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
