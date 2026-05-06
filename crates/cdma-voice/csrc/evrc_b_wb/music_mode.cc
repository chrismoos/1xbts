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
#include <stdio.h>
#include <stdlib.h>
#include <math.h>

#include  "globs.h"
#include  "proto.h"
#include  "filt.h"


#include "upgrades.h"

#include "acelp.h"		// for factorial packing

#include "mdct.h"

#define FM_COEF_ERASURE 0.4
#define NOISE_COEF 0.8
#define PI 3.1415926535897931

static float ni_cb[4] = { 0.15, 0.30, 0.45, 0.60 };

static float nr_preemph[2] = { 1, 0.2 }, dr_preemph[1] =
{
1};


float residual_save[GUARD + FrameSize + LOOKAHEAD_LEN];
float mres_buffer[FrameSize];


/****************************************************************************************************/


void
FGV_MEM::celpmdct_resid_calc (int bit_rate)
{


    int i, j;

    // LSP Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (bit_rate == 3) {
	knum = 3;
	enc_lsp_vq_22 (lsp_nq, SScratch, lsp);
	for (i = 0; i < knum; i++)
	    data_packet.LSP_IDX[i] = SScratch[i];
    }
    else {
	if (data_packet.WB_MODE_BIT == 1) {

	    knum = 4;

	    enc_lsp_vq_28 (lsp_nq, SScratch, lsp);

	    for (i = 0; i < knum; i++)
		data_packet.LSP_IDX[i] = SScratch[i];
	}
	else {			//narrowband case

	    knum = 4;
	    enc_lsp_vq_28 (lsp_nq, SScratch, lsp);
	    for (i = 0; i < knum; i++)
		data_packet.LSP_IDX[i] = SScratch[i];

	}
    }

    for (i = 0; i < ORDER; i++)
	cbprevprev_E[i] = cbprev_E[i] = lsp[i];

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


}



void
FGV_MEM::music_encoder ()
{
    static int firsttime = 1;
    static FILE *ResidualFile;
    static FILE *QuantizedLPCFile;

    float tmp_buf[320], tmp_buf1[240];
    int j;
    float tempval1, tempval2, delayi[3];


    float reconmdct[160];

    short winflag;

    float pci_mod[ORDER];

    double preemphmemTemp[1];
    float FfiltMemTemp[ORDER];

    static long MusicModeNoiseSeed;

    float SynMemoryM_local[ORDER], WFmemFIR_local[ORDER],
	WFmemIIR_local[ORDER];

    static float SynMemoryM_recon[ORDER], WFmemFIR_recon[ORDER],
	WFmemIIR_recon[ORDER];

    static POLEZERO_FILTER PreemphState = { 1, 1, 0, 0, preemphmem };

    /*80 samples from the lookaheas of the present frame are saved to overlap and add with the next frame */
    static float lookahead[80], lookahead_wnoise[80];

    static float emin = -100, emax = 20, norm, sumabs = 0.0, eqmin =
	-80, eqmax = -10, eqnum = 128;

    double egy;
    float einit, eqdel, ermin, egyq, noise_gain, edel = 8;


    float err;

    int indx, imin, subframesize;
    float Scratch1[2 * SubFrameSize + 6];

    float sign;
    int iter;



    static float mdctlpf_filt_num_coef[5] = {
	0.592041015625000,
	2.330688476562500,
	3.477539062500000,
	2.330688476562500,
	0.592041015625000,
    };
    static float mdctlpf_filt_den_coef[5] = {
	1.000000000000000,
	2.922851562500000,
	3.364868164062500,
	1.776367187500000,
	0.366821289062500,
    };

    if (lastrateE != 4 || prev_celp_mdct_dec != 1) {	/* reset memory after transition, also reset the lookahead */

	winflag = 1;

	preemphmem[0] = 0.0;
	preemphmemTemp[0] = 0.0;

	for (j = 0; j < ORDER; j++)
	    FfiltMem[j] = 0.0;

	MusicModeNoiseSeed = 0;

	for (j = 0; j < 80; j++) {
	    lookahead[j] = 0.0;
	    lookahead_wnoise[j] = 0.0;
	}

    }
    else
	winflag = 0;

    if (lastrateE != 4) {
	for (j = 0; j < ORDER; j++) {	//these memories will get updated before the subframe loop
	    SynMemoryM_recon[j] = 0.0;
	    WFmemFIR_recon[j] = 0.0;
	    WFmemIIR_recon[j] = 0.0;
	}

    }

    int i, k;

    float tmplsi[ORDER], tmplsc[ORDER];

    data_packet.PACKET_RATE = 4;

    if (beta < 0.1) {
	accshift = 0;
	dpm = 0;
	shiftSTATE = 0;
	frame_shift[0] = accshift;
	frame_shift[1] = accshift;
	frame_shift[2] = accshift;
    }

    if (accshift > 20)
	shiftSTATE = -1;
    if (accshift < -20)
	shiftSTATE = 1;
    if (accshift <= 10 && shiftSTATE == -1)
	shiftSTATE = 0;
    if (accshift >= -10 && shiftSTATE == 1)
	shiftSTATE = 0;

    /* Control accshift */

    if (shiftSTATE == 1 && beta < 0.4)
	delay += 1.0;
    else if (shiftSTATE == -1 && beta < 0.4)
	delay -= 1.0;
    if (delay > DMAX)
	delay = DMAX;
    if (delay < DMIN)
	delay = DMIN;

    if (lastrateE < 3)
	pdelay = delay;

    if (fabs (delay - pdelay) > 15)
	pdelay = delay;

    lsp2a (pci, OldlspE, ORDER);

    for (j = 0; j < GUARD + FrameSize + LOOKAHEAD_LEN; j++) {
	residual_save[j] = residual[j];
    }


    for (i = 0; i < NoOfSubFrames; i++) {

	Interpol (lspi, OldlspE, lsp, i, ORDER);
	lsp2a (pci, lspi, ORDER);


	if (i < 2) {

	    for (j = 0; j < SubFrameSize - 1 - dpm; j++)
		bl_intrp (residualm + j + dpm,
			  residual_save + GUARD + i * (SubFrameSize - 1) + j +
			  dpm, accshift, BLFREQ, BLPRECISION);

	    dpm = dpm + SubFrameSize - 1 - dpm - (SubFrameSize - 1);

	    for (j = 0; j < SubFrameSize - 1; j++)
		mres_buffer[j + i * (SubFrameSize - 1)] = residualm[j];

	    SynthesisFilter (origm, residualm, pci, SynMemoryM,	//to update SynMemoryM
			     ORDER, SubFrameSize - 1);

	    // Weighting filter 
	    weight (wpci, pci_nq, GAMMA1, ORDER);
	    fir (Scratch1, origm, wpci, WFmemFIR, ORDER, SubFrameSize - 1);	//to update WFmemFIR
	    weight (wpci, pci_nq, GAMMA2, ORDER);
	    iir (worigm, Scratch1, wpci, WFmemIIR, ORDER, SubFrameSize - 1);	//to update WFmemIIR  

	    /*First apply the pre-emphasis...Note that you have to save the state at the 160th sample */
	    polezero_filter (residualm, tmp_buf, SubFrameSize - 1, nr_preemph,
			     dr_preemph, PreemphState);

	    /*Now apply the formant filter */
	    for (k = 0; k < ORDER; k++)
		pci_mod[k] = pci[k] * pow (FM_COEF, (k + 1));
	    SynthesisFilter (residual + (i * (SubFrameSize - 1)) + GUARD, tmp_buf, pci_mod, FfiltMem, ORDER, SubFrameSize - 1);	//rfilter

	    /* Update residualm */
	    for (j = 0; j < dpm; j++)
		residualm[j] = residualm[j + SubFrameSize - 1];

	    /* Update excitation */
	    for (j = 0; j < ACBMemSize; j++)
		Excitation[j] = Excitation[j + SubFrameSize - 1];
	}
	else {

	    for (j = 0; j < SubFrameSize - dpm; j++)
		bl_intrp (residualm + j + dpm,
			  residual_save + GUARD + i * (SubFrameSize - 1) + j +
			  dpm, accshift, BLFREQ, BLPRECISION);

	    for (j = 0; j < SubFrameSize; j++)
		mres_buffer[j + i * (SubFrameSize - 1)] = residualm[j];

	    SynthesisFilter (origm, residualm, pci, SynMemoryM,	//to update SynMemoryM
			     ORDER, SubFrameSize);
	    weight (wpci, pci_nq, GAMMA1, ORDER);
	    fir (Scratch1, origm, wpci, WFmemFIR, ORDER, SubFrameSize);	//to update WFmemFIR
	    weight (wpci, pci_nq, GAMMA2, ORDER);
	    iir (worigm, Scratch1, wpci, WFmemIIR, ORDER, SubFrameSize);	//to update WFmemIIR


	    /*First apply the pre-emphasis...Note that you have to save the state at the 160th sample */
	    polezero_filter (residualm, tmp_buf, SubFrameSize, nr_preemph,
			     dr_preemph, PreemphState);

	    /*Now apply the formant filter */
	    for (k = 0; k < ORDER; k++)
		pci_mod[k] = pci[k] * pow (FM_COEF, (k + 1));
	    SynthesisFilter (residual + (i * (SubFrameSize - 1)) + GUARD, tmp_buf, pci_mod, FfiltMem, ORDER, SubFrameSize);	//rfilter

	    /* Update residualm */
	    for (j = 0; j < dpm; j++)
		residualm[j] = residualm[j + SubFrameSize];

	    /* Update excitation */
	    for (j = 0; j < ACBMemSize; j++)
		Excitation[j] = Excitation[j + SubFrameSize];

	}

    }

    lsp2a (pci, lsp, ORDER);

    /*Use temporary memory to filter the lookahead */
    preemphmemTemp[0] = preemphmem[0];
    POLEZERO_FILTER PreemphStateTemp = { 1, 1, 0, 0, preemphmemTemp };

    for (j = 0; j < ORDER; j++)
	FfiltMemTemp[j] = FfiltMem[j];

    float temp_residualm[LOOKAHEAD_LEN];

    float zp_residual[FrameSize + LOOKAHEAD_LEN + 40];

    for (j = 0; j < FrameSize + LOOKAHEAD_LEN; j++)
	zp_residual[j] = residual_save[GUARD + j];

    for (j = FrameSize + GUARD; j < FrameSize + GUARD + 40; j++)
	zp_residual[j] = 0.0;

    for (j = 0; j < dpm; j++)
	temp_residualm[j] = residualm[j];

    for (j = 0; j < GUARD - dpm; j++)
	bl_intrp (temp_residualm + j + dpm, zp_residual + FSIZE + dpm + j,
		  accshift, BLFREQ, BLPRECISION);

    /*apply the preemphasis filter on the lookahead */
    polezero_filter (temp_residualm, tmp_buf, GUARD, nr_preemph, dr_preemph,
		     PreemphStateTemp);

    /*Now apply the formant filter */
    for (k = 0; k < ORDER; k++)
	pci_mod[k] = pci[k] * pow (FM_COEF, (k + 1));
    SynthesisFilter (residual + FrameSize + GUARD, tmp_buf, pci_mod, FfiltMemTemp, ORDER, GUARD);	//rfilter

    /*At this point the residual from 80 to 320 contains the present frame to be analyized by MDCT */
    mdct (residual + GUARD, mdctcoeff, winflag);

    if (args->unquantized_full_rate)	//unquantized mdct coefficients transmitted to the decoder if this flag is set
	for (j = 0; j < 160; j++)
	    copy[j] = mdctcoeff[j];

    /* Quantize MDCT coeeficients */
    for (j = 0, egy = 0.0; j < 160; j++)
	egy += mdctcoeff[j] * mdctcoeff[j];
    egy = (-10 * log10 (egy + 1e-10) / 1.2);
    egy = egy >= emin ? egy : emin;

    einit = egy;
    norm = pow ((float) 10, (float) egy / 20);

    for (j = 0, sumabs = 0.0; j < 144; j++)
	sumabs += fabs (floor ((norm * mdctcoeff[j]) + 0.5));

    if (sumabs > MDCT_NPULSES)
	sign = -1;
    else if (sumabs < MDCT_NPULSES)
	sign = 1;

    iter = 0;

    while (sumabs != MDCT_NPULSES && egy < emax && edel > 0.000001) {
	egy = egy + sign * edel;
	norm = pow ((float) 10, (float) egy / 20);

	sumabs = 0.0;
	for (j = 0; j < 144; j++)
	    sumabs += fabs (floor ((norm * mdctcoeff[j]) + 0.5));

	if (sumabs > MDCT_NPULSES && sign == 1) {
	    sign = -1;
	    edel = 0.5 * edel;
	}
	else {
	    if (sumabs < MDCT_NPULSES && sign == -1) {
		sign = 1;
		edel = 0.5 * edel;
	    }
	}
	iter = iter + 1;
    }

    sumabs = 0.0;

    for (j = 0; j < 144; j++) {
	tempval1 = floor ((norm * mdctcoeff[j]) + 0.5);
	sumabs += fabs (tempval1);
	reconmdct[j] = tempval1;	// replaced above commented code with this.
    }
    for (j = 144; j < 160; j++)
	reconmdct[j] = 0.0;



    // This is a sanity check
    {

	int isum = (int) sumabs;
	int k;

	if (isum != MDCT_NPULSES) {

	    if (isum > MDCT_NPULSES) {
		j = isum - MDCT_NPULSES;
		k = 143;
		while (j > 0) {
		    if (reconmdct[k] != 0) {
			if (reconmdct[k] > 0.0)
			    reconmdct[k] -= 1.0;
			else if (reconmdct[k] < 0.0)
			    reconmdct[k] += 1.0;
			j--;
		    }
		    else {
			k--;
		    }
		}
	    }
	    else {
		long seed = encode_fcnt;

		j = MDCT_NPULSES - isum;
		for (k = 0; k < j; k++) {
		    int pos = (int) (144 * ran0 (&seed));

		    if (reconmdct[pos] >= 0) {
			reconmdct[pos] += 1.0;
		    }
		    else {
			reconmdct[pos] -= 1.0;
		    }
		}
	    }

	    isum = 0;
	    for (i = 0; i < 144; i++) {
		isum += (int) fabs (reconmdct[i]);
	    }


	}

    }


    {
	intVar cw1;

	cw1 = factorial_pack_new (144, reconmdct);
	cw1.unpackToWordArray ((Word16 *) data_packet.MDCT_IDX, MDCT_NWORDS);

	/* test code */
	intVar cw2;
	int mdct2[144];

	cw2.packFromWordArray ((Word16 *) data_packet.MDCT_IDX, MDCT_NWORDS);
	factorial_unpack_new (cw2, 144, MDCT_NPULSES, mdct2);

	for (i = 0; i < 144; i++) {
	    if ((int) reconmdct[i] != mdct2[i]) {
		printf
		    ("music test error : %6ld reconmdct[%03d]=%2f mdct2[%03d]=%2d\n",
		     encode_fcnt, i, reconmdct[i], i, mdct2[i]);
	    }
	}
    }


    /*recalculate egy as a gain term */
    float crosscorr = 0.0, selfcorr = 0.0;

    for (j = 0; j < 144; j++) {
	crosscorr += (mdctcoeff[j] * reconmdct[j]);
	selfcorr += (reconmdct[j] * reconmdct[j]);
    }
    egy = -20 * log10 (1e-2 + (crosscorr / selfcorr));


    /* Make quantizer independent of MDCT_NPULSES */
    float eqq = 10 * log10 (selfcorr);

    eqmin = -95 + eqq;
    eqmax = -25 + eqq;

    eqdel = (eqmax - eqmin) / (eqnum - 1);

    ermin = fabs (egy - emin);
    imin = 0;
    for (indx = 1; indx < eqnum; indx++) {
	err = fabs (egy - (eqmin + indx * eqdel));
	if (err < ermin) {
	    ermin = err;
	    imin = indx;
	}
    }
    if (imin == eqnum - 1)
	egyq = 100;
    else
	egyq = eqmin + imin * eqdel;

    /*populate the data packet structure to enable packing the frame gains */
    data_packet.FrameGain_IDX = imin;

    norm = pow ((float) 10, (float) egyq / 20);

    /*normalize the reconstructed coefficients */
    for (j = 0; j < 160; j++)
	reconmdct[j] = reconmdct[j] / norm;


    /* populate the input to the upperband after taking the inverse IMDCT and appropriately overlapping and adding */

    imdct (reconmdct, tmp_buf, winflag);


    for (j = 0; j < 80; j++)
	tmp_buf[j + 40] += lookahead[j];


    for (j = 0; j < 160; j++)
	//UBExcitationMusic[j]=tmp_buf[j+40];
	UBExcitationMusic[j] = tmp_buf[j + 40] * MUSIC_MODE_DAMPEN_HB_ENERGY_FACT;	///////////// FAKE OUT HIGH BAND ENERGY

    /*save for next frame's overlap and add */
    for (j = 0; j < 80; j++)
	lookahead[j] = tmp_buf[j + 200];

    float temp_worig[160], temp_recon[160], dummy[80];

    /*these are parameters for the MDCT/CELP detector: Noise addition not included in the signals used in the detector */

    for (j = 0; j < ORDER; j++) {	//copy memory from backed up values
	WFmemFIR_local[j] = WFmemFIR_bup[j];
	WFmemIIR_local[j] = WFmemIIR_bup[j];
	SynMemoryM_local[j] = SynMemoryM_bup[j];

    }


    for (j = 0; j < 80; j++)
	prev_synth[j] = tmp_buf[120 + j];	//save for initializing filter memories


    /*Determine noise gain */

    if (egy < emax) {

	for (j = 40, tempval2 = 0.0; j < 144; j++)
	    if (reconmdct[j] == 0)
		tempval2 += (mdctcoeff[j] * mdctcoeff[j]);
	for (j = 40, tempval1 = 0.0; j < 144; j++)
	    tempval1 += (mdctcoeff[j] * mdctcoeff[j]);
	noise_gain = NOISE_COEF * pow ((tempval2 / (tempval1 + 1e-10)), 0.5);

	/*quantize the noise gain */
	ermin = (noise_gain - ni_cb[0]) * (noise_gain - ni_cb[0]);
	imin = 0;

	for (j = 1; j < 4; j++) {
	    err = (noise_gain - ni_cb[j]) * (noise_gain - ni_cb[j]);
	    if (err < ermin) {
		ermin = err;
		imin = j;
	    }
	}
	noise_gain = ni_cb[imin];
	data_packet.NoiseGain_IDX = imin;

    }
    else
	data_packet.NoiseGain_IDX = 0;

    /*obtain the coded, noise added residual as in the decoder */

    /*add noise to the qmdct based on the quantized noise gain */
    MusicModeNoiseSeed =
	(long) data_packet.MDCT_IDX[0] + (long) data_packet.MDCT_IDX[1] << 16;
    for (j = 40; j < 144; j++)
	if (reconmdct[j] == 0)
	    reconmdct[j] =
		noise_gain * (0.5 - ran0 (&MusicModeNoiseSeed)) / norm;

    if (args->unquantized_full_rate)
	for (j = 0; j < 160; j++)
	    reconmdct[j] = copy[j];


    imdct (reconmdct, tmp_buf, winflag);

    for (j = 0; j < 80; j++) {
	tmp_buf[j + 40] = tmp_buf[j + 40] + lookahead_wnoise[j];
	lookahead_wnoise[j] = tmp_buf[j + 200];
    }

    for (j = 0; j < 160; j++)
	MusicResidual[j] = tmp_buf[j + 40];



    pdelay = delay;		// needed for RCELP purposes only

}


void
FGV_MEM::Update_CELPMEM ()
{

    int i, subframesize, j;


    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	// interpolate lsp 
	Interpol (lspi, OldlspE, lsp, i, ORDER);
	Interpol (lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);
	// Convert lsp to PC 
	lsp2a (pci, lspi, ORDER);
	lsp2a (pci_nq, lspi_nq, ORDER);

	ZeroInput (zir, pci_nq, pci, MusicResidual + i * (SubFrameSize - 1),
		   GAMMA1, GAMMA2, ORDER, subframesize, 1);


	for (j = 0; j < ACBMemSize; j++)
	    Excitation[j] = MusicResidual[FrameSize - ACBMemSize + j];

    }

    for (j = 0; j < ACBMemSize; j++)
	bufferm[j] = mres_buffer[FrameSize - ACBMemSize + j];

    // the above ensure the next frame ( if CELP) does RCELP with the correct target buffer which is basically
    // an ACB extended version of the previous frame's unq , unshifted residual signal ( since this applies
    // only to the case when RCELP is not done inside MDCT

}


void
FGV_MEM::music_decoder (float *outFbuf)
{
    static int firsttimeindecoder = 1;
    static FILE *MDCTFile;

    short subframesize;

    float delayi[3];


    register float *foutP;


    float tmp_buf[160];

    short winflag;

    POLEZERO_FILTER PreemphState = { 1, 0, 1, 0, dec_preemphmem };

    float qmdct[160];

    float noise_gain, frame_gain, eqdel;

    static float eqmin = -80, eqmax = -10, eqnum = 128;
    float norm;

    int i, k, j;

    float pci_mod[ORDER];
    float tmplsi[ORDER], tmplsc[ORDER];


    foutP = outFbuf;
    if (firsttimeindecoder == 1) {
	firsttimeindecoder = 0;




    }

    if (prev_celp_mdct_dec != 1) {	// this will be satisfied only if the last rate is 

	winflag = 1;

	for (j = 0; j < 80; j++)
	    dec_lookahead[j] = 0.0;
	for (j = 0; j < 80; j++)
	    dec_lookahead_wonoise[j] = 0.0;

	dec_preemphmem[0] = 0.0;
	for (j = 0; j < ORDER; j++)
	    dec_FormantFilterMemory[j] = 0.0;

    }
    else
	winflag = 0;

    /*****decode the packed parameters********/
    dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);	/*decode the lsfs */



    {
	intVar cw2;
	int mdct2[144];

	cw2.packFromWordArray ((Word16 *) data_packet.MDCT_IDX, MDCT_NWORDS);
	factorial_unpack_new (cw2, 144, MDCT_NPULSES, mdct2);

	for (i = 0; i < 144; i++) {
	    qmdct[i] = (float) mdct2[i];
	}
	for (i = 144; i < 160; i++) {
	    qmdct[i] = 0.0;
	}
    }

    /* Make quantizer independent of MDCT_NPULSES */
    float selfcorr = 0.0;

    for (j = 0; j < 144; j++) {
	selfcorr += qmdct[j] * qmdct[j];
    }
    float eqq = 10 * log10 (selfcorr);

    eqmin = -95 + eqq;
    eqmax = -25 + eqq;
    eqdel = (eqmax - eqmin) / (eqnum - 1);

    if (data_packet.FrameGain_IDX == eqnum - 1)
	frame_gain = 100;	/*decode the norm factor */
    else
	frame_gain = eqmin + (data_packet.FrameGain_IDX * eqdel);
    norm = pow ((float) 10, (float) frame_gain / 20);


    noise_gain = ni_cb[data_packet.NoiseGain_IDX];	/*decode the noise gain */
    prev_noise_gain = noise_gain;

    for (j = 0; j < 160; j++)
	prev_qmdct[j] = qmdct[j];	//save for erasure processing
    prev_norm = norm;



    for (j = 0; j < 160; j++)
	qmdct[j] = qmdct[j] / norm;

    imdct (qmdct, MusicResidual, winflag);

    /*overlap and add at the decoder */
    for (j = 0; j < 80; j++) {
	MusicResidual[j + 40] =
	    MusicResidual[j + 40] + dec_lookahead_wonoise[j];
	dec_lookahead_wonoise[j] = MusicResidual[j + 200];
    }

    for (j = 0; j < 160; j++)
	LB_Excitation[j] = MusicResidual[j + 40];



    /*add noise to the qmdct based on the quantized noise gain */
    dec_MusicModeNoiseSeed =
	(long) data_packet.MDCT_IDX[0] + (long) data_packet.MDCT_IDX[1] << 16;


    for (j = 40; j < 144; j++)
	if (qmdct[j] == 0)
	    qmdct[j] =
		noise_gain * (0.5 - ran0 (&dec_MusicModeNoiseSeed)) / norm;

    if (args->unquantized_full_rate)
	for (j = 0; j < 160; j++)
	    qmdct[j] = copy[j];


    imdct (qmdct, MusicResidual, winflag);

    for (j = 0; j < 80; j++) {
	MusicResidual[j + 40] = MusicResidual[j + 40] + dec_lookahead[j];
	dec_lookahead[j] = MusicResidual[j + 200];

    }

    for (i = 0; i < ORDER; i++)
	tmplsc[i] = cos (pi2 * lsp[i]);
    for (i = 0; i < ORDER; i++)
	tmplsi[i] =
	    (tmplsc[i] <
	     0.) + (SIGN (tmplsc[i])) * 0.5 * sqrt (1 - fabs (tmplsc[i]));
    for (i = 0; i < ORDER; i++)
	cbprevprev_D[i] = cbprev_D[i] = tmplsi[i];

    lsp_spread (lsp);

    if (PREV_LSP_FLAG == 1) {
	for (i = 0; i < LPCORDER; i++) {
	    OldOldlspD[i] = OldlspD[i] = lsp[i];
	}
    }

    /*Per Subframe synthesis */

    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	Interpol (lspi, OldlspD, lsp, i, ORDER);
	lsp2a (pci, lspi, ORDER);


	/*first apply the inverse formant filter. Note that the inverse formant filter is like the filter used to get the residual */

	for (k = 0; k < ORDER; k++)
	    pci_mod[k] = pci[k] * pow (FM_COEF, (k + 1));
	GetResidual (tmp_buf, MusicResidual + i * (SubFrameSize - 1) + 40,
		     pci_mod, dec_FormantFilterMemory, ORDER, subframesize);

	/*update for next celp frame */
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] = MusicResidual[i * (SubFrameSize - 1) + j + 40];	//bug-fix


	/* Populate the low band excitation to be used by the WB synthesis, currently turned off */

	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];




	/*next apply the de-emphasis filter */
	polezero_filter (tmp_buf, MusicResidual + i * (SubFrameSize - 1) + 40,
			 subframesize, dr_preemph, nr_preemph, PreemphState);

	SynthesisFilter (DECspeech,
			 MusicResidual + i * (SubFrameSize - 1) + 40, pci,
			 SynMemory, ORDER, subframesize);


	delayi[0] = (float) (DMIN);
	delayi[1] = (float) (DMIN);
	delayi[2] = (float) (DMIN);

	if (args->post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 (delayi[0] + delayi[1]) / 2.0, 0.0, 0.0,
			 subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);


	if (i == 2)
	    for (j = 0; j < subframesize; j++)
		last_music_signal_sf[j] = DECspeechPF[j];


	if (lastrateD == 4 && prev_celp_mdct_dec != 1 && i == 0) {	//the previous mode was celp. so the ringing of the bass boost mem needs to be added
	    float temp_bbmem[SubFrameSize];

	    for (j = 0; j < SubFrameSize; j++)
		temp_bbmem[j] = 0.0;
	    bass_boost (temp_bbmem, last_delayBB, subframesize);
	    for (j = 0; j < SubFrameSize; j++)
		DECspeechPF[j] += temp_bbmem[j];
	}


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



void
FGV_MEM::music_erasure_decoder (float *outFbuf)
{

    float norm_fade;
    int i, j;
    float tmp_buf[160], estim_qmdct[160];
    short subframesize;

    register float *foutP;
    POLEZERO_FILTER PreemphState = { 1, 0, 1, 0, dec_preemphmem };
    float pci_mod[ORDER];

    float delayi[3];


    foutP = outFbuf;

    norm_fade = norm_fade_fac * (1 / prev_norm);


    for (j = 0; j < 160; j++)
	estim_qmdct[j] = prev_qmdct[j] * norm_fade;

    imdct (estim_qmdct, MusicResidual, 0);

    /*overlap and add at the decoder for upperband excitaiton */
    for (j = 0; j < 80; j++) {
	MusicResidual[j + 40] =
	    MusicResidual[j + 40] + dec_lookahead_wonoise[j];
	dec_lookahead_wonoise[j] = MusicResidual[j + 200];
    }

    for (j = 0; j < 160; j++)
	LB_Excitation[j] = MusicResidual[j + 40];

    /*add noise to the qmdct based on the quantized noise gain */
    if (!prev_frame_error)
	dec_MusicModeNoiseSeed = (long) (OldlspD[ORDER - 1] * 65536.0);
    for (j = 40; j < 144; j++)
	if (estim_qmdct[j] == 0)
	    estim_qmdct[j] =
		prev_noise_gain * (0.5 -
				   ran0 (&dec_MusicModeNoiseSeed)) *
		norm_fade;
    imdct (estim_qmdct, MusicResidual, 0);

    for (j = 0; j < 80; j++) {
	MusicResidual[j + 40] = MusicResidual[j + 40] + dec_lookahead[j];
	dec_lookahead[j] = MusicResidual[j + 200];
    }



    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	Interpol (lspi, OldlspD, OldlspD, i, ORDER);
	lsp2a (pci, lspi, ORDER);


	/*first apply the inverse formant filter. Note that the inverse formant filter is like the filter used to get the residual */

	for (int k = 0; k < ORDER; k++)
	    pci_mod[k] = pci[k] * pow (FM_COEF_ERASURE, (k + 1));
	GetResidual (tmp_buf, MusicResidual + i * (SubFrameSize - 1) + 40,
		     pci_mod, dec_FormantFilterMemory, ORDER, subframesize);

	/*update for next celp frame */
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] = MusicResidual[i * (SubFrameSize - 1) + j + 40];	//bug-fix


	/* Populate the low band excitation to be used by the WB synthesis, currently turned off */

	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];




	/*next apply the de-emphasis filter */
	polezero_filter (tmp_buf, MusicResidual + i * (SubFrameSize - 1) + 40,
			 subframesize, dr_preemph, nr_preemph, PreemphState);

	SynthesisFilter (DECspeech,
			 MusicResidual + i * (SubFrameSize - 1) + 40, pci,
			 SynMemory, ORDER, subframesize);


	delayi[0] = (float) (DMIN);
	delayi[1] = (float) (DMIN);
	delayi[2] = (float) (DMIN);

	if (args->post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 (delayi[0] + delayi[1]) / 2.0, 0.0, 0.0,
			 subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);

	if (i == 2)
	    for (j = 0; j < subframesize; j++)
		last_music_signal_sf[j] = DECspeechPF[j];
	if ((lastrateD == 4 || prev_celp_erasure == 1) && prev_celp_mdct_dec != 1 && i == 0) {	//the previous mode was celp. so the ringing of the bass boost mem needs to be added
	    float temp_bbmem[SubFrameSize];

	    for (j = 0; j < SubFrameSize; j++)
		temp_bbmem[j] = 0.0;
	    bass_boost (temp_bbmem, last_delayBB, subframesize);
	    for (j = 0; j < SubFrameSize; j++)
		DECspeechPF[j] += temp_bbmem[j];
	}

	prev_celp_mdct_dec = 1;

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
