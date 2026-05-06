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
    "$Id: voiced.cc,v 1.25.6.5 2007/06/24 06:43:20 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/







/* This is the new half rate coding in the EVRC shell */
#include "macro.h"
#include "proto.h"
#include "rom.h"
#include "defines.h"
#include "struct.h"
#include "filt.h"

#include <cstring>

#define LSP28FP 1




#define LSP_SPREAD_FACTOR_QPPP 0.01




//////////////////////////////////////////////////////
// Do stand-alone RCELP
// Computes residualm - modified residual (full-frame)
// Computes mspeech - modified speech (full-frame)
// Computes wspeech - weighted modified speech
// Updates pdelay, accshift, dpm, shiftSTATE, delay
//         data_packet.DELAY_IDX,data_packet.DELTA_DELAY_IDX
// Also updates pci, pci_nq etc
//////////////////////////////////////////////////////
void
FGV_MEM::get_rcelp ()
{
    int i, j;
    int subframesize;
    float delayi[3];

    // Update shiftSTATE with hysteresis 
    if (beta < 0.1) {
	accshift = 0;
	dpm = 0;
	shiftSTATE = 0;
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


    data_packet.DELAY_IDX = (int) (delay - DMIN);

    if (lastrateE < 3)
	pdelay = delay;

    //Prepare for the delta delay
    j = (short) (delay - pdelay);
    if (abs (j) > 15)
	j = 0;
    else
	j = j + 16;
    assert (j >= 0 && j < 32);
    data_packet.DELTA_DELAY_IDX = (unsigned short) j;

    if (rate == 3 && !rcelp_half_rateE && !dim_and_burstE)
	assert (fabs (delay - pdelay) <= 15);
    /* Smooth interpolation if the difference between delays is too big */
    if (fabs (delay - pdelay) > 15)
	pdelay = delay;

    //float residualm_frame[FrameSize+EXTRA];
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


	/* Interpolate delay */
	Interpol_delay (delayi, &pdelay, &delay, i);

	ComputeACB (residualm, Excitation + ACBMemSize, delayi,
		    residual + GUARD + i * (SubFrameSize - 1),
		    FrameSize + GUARD - i * (SubFrameSize - 1), &dpm,
		    &accshift, beta, subframesize, RSHIFT);

	for (j = 0; j < subframesize; j++)
	    residualm_frame[i * (SubFrameSize - 1) + j] = residualm[j];

	/* Get weighted speech */
	/* ORIGM */



	SynthesisFilter (mspeech + i * (SubFrameSize - 1), residualm, pci,
			 SynMemoryM, ORDER, subframesize);


	/* Weighting filter */
	weight (wpci, pci_nq, GAMMA1_NB, ORDER);
	fir (Scratch, mspeech + i * (SubFrameSize - 1), wpci, WFmemFIR, ORDER,
	     subframesize);
	weight (wpci, pci_nq, GAMMA2, ORDER);
	iir (wspeech + i * (SubFrameSize - 1), Scratch, wpci, WFmemIIR, ORDER,
	     subframesize);

	/* Update residualm */
	for (j = 0; j < dpm; j++)
	    residualm[j] = residualm[j + subframesize];

    }
}



void
FGV_MEM::jumptofullcelp (int &rate)
{
    int i;

    bit_rate = rate = PACKET_RATE = 4;
    MODE_BIT = 0;

    // Revert back to old memory at the end of previous frame
    delay = delay2, accshift = accshift2, dpm = dpm2, shiftSTATE =
	shiftSTATE2;
    for (i = 0; i < ORDER; i++)
	SynMemoryM[i] = SynMemoryM2[i];
    for (i = 0; i < ORDER; i++)
	WFmemFIR[i] = WFmemFIR2[i];
    for (i = 0; i < ORDER; i++)
	WFmemIIR[i] = WFmemIIR2[i];
    for (i = 0; i < ACBMemSize; i++)
	Excitation[i] = Excitation2[i];
    for (i = 0; i < dpm; i++)
	residualm[i] = residualm2[i];
    for (i = 0; i < ACBMemSize; i++)
	bufferm[i] = bufferm2[i];

    /* Re-initialize PackWdsPtr */
    PackWdsPtr[0] = 16;
    PackWdsPtr[1] = 0;
    for (i = 0; i < PACKWDSNUM; i++)
	PackedWords[i] = 0;

    celpmdct_resid_calc (bit_rate);	//residual calculation from quantized LPCs...common to both celp and mdct and needed by discriminator 
    celp_encoder (bit_rate);
}



void
FGV_MEM::voiced_encoder (float *out, int &rate)
{
    register int i, j;
    short subframesize;
    int delta_lag_E = 0;

    //static char LASTLAST_PPP_MODE_E='Q', LAST_PPP_MODE_E='Q';
    bool NEWBUMPUP = FALSE;

    //Set NEWBUMPUP to TRUE wherever you want the new bumpup scheme to kick in
    if (args->operating_point == 0)
	NEWBUMPUP = FALSE;
    else if (args->operating_point == 1)
	NEWBUMPUP = FALSE;
    else if (args->operating_point == 2)
	NEWBUMPUP = TRUE;

    //Reset the pattern  when the last frame is non-PPP or full-rate PPP
    if (LAST_PACKET_RATE_E == 1 || LAST_PACKET_RATE_E == 3
	|| LAST_MODE_BIT_E == 0) {
	LASTLAST_PPP_MODE_E = 'Q';
	LAST_PPP_MODE_E = 'F';
	//over-writing above
	LAST_PPP_MODE_E = 'Q';
    }

    // Save MA LSP Memories
    for (i = 0; i < LPCORDER; i++)
	cbprevprev_E2[i] = cbprevprev_E[i];
    for (i = 0; i < LPCORDER; i++)
	cbprev_E2[i] = cbprev_E[i];

    // Figure out  the PPP_MODE
    if (args->rfileP == NULL) {	// Do only if external rate is not provided
	if (args->operating_point == 1) {
	    if (lastrateE != 4) {
		jumptofullcelp (rate);
		return;
		//celp_encoder(rate);
		//return;
	    }

	    else
		PPP_MODE_E = 'Q';
	}
	else if (args->operating_point == 2) {
	    if (!strcmp (args->pattern, "QQF")) {
		//QQF Deterministic Pattern
		if ((LASTLAST_PPP_MODE_E == 'Q') && (LAST_PPP_MODE_E == 'Q'))
		    PPP_MODE_E = 'F';
		else
		    PPP_MODE_E = 'Q';
	    }
	    else if (!strcmp (args->pattern, "FQF")) {
		//FQF Deterministic Pattern
		if ((LASTLAST_PPP_MODE_E == 'F') && (LAST_PPP_MODE_E == 'Q'))
		    PPP_MODE_E = 'F';
		else
		    PPP_MODE_E = 'Q';
	    }
	    else if (!strcmp (args->pattern, "QFF")) {
		//QFF Deterministic Pattern
		if ((LASTLAST_PPP_MODE_E == 'Q') && (LAST_PPP_MODE_E == 'F'))
		    PPP_MODE_E = 'F';
		else
		    PPP_MODE_E = 'Q';
	    }
	    else if (!strcmp (args->pattern, "FFF")) {
		//FFF Deterministic Pattern
		PPP_MODE_E = 'F';
	    }
	    else if (!strcmp (args->pattern, "FQFQ")) {
		//FQFQ Deterministic Pattern
		if (LAST_PPP_MODE_E == 'Q')
		    PPP_MODE_E = 'F';
		else
		    PPP_MODE_E = 'Q';
	    }
	    else if (!strcmp (args->pattern, "FFQQ")) {
		//FQFQ Deterministic Pattern
		if ((LASTLAST_PPP_MODE_E == 'Q'))
		    PPP_MODE_E = 'F';
		if ((LASTLAST_PPP_MODE_E == 'F'))
		    PPP_MODE_E = 'Q';
	    }
	    else if (!strcmp (args->pattern, "QHQF")) {
		//QHQF Deterministic Pattern
		if (LAST_PPP_MODE_E == 'Q') {
		    if (LASTLAST_PPP_MODE_E == 'H')
			PPP_MODE_E = 'F';
		    else
			PPP_MODE_E = 'H';
		}
		else
		    PPP_MODE_E = 'Q';
	    }
	    else {
		fprintf (stderr, "Bad Pattern %s in PPP\n", args->pattern);
		exit (-1);
	    }
	}
	else {
	    fprintf (stderr, "Operating Point-%d in PPP??\n",
		     args->operating_point);
	    exit (-1);
	}
    }
    // Put rate into data_packet strucure
    if (PPP_MODE_E == 'Q')
	data_packet.PACKET_RATE = 2;
    else if (PPP_MODE_E == 'F')
	data_packet.PACKET_RATE = 4;

    // Put mode bit into data_packet strucure
    data_packet.MODE_BIT = 1;	// Mode bit for PPP in full and 1/4-rates

    // LSP Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (PPP_MODE_E == 'Q' || PPP_MODE_E == 'H') {
	// NEW 13 bit LSP VQ

	float delta, delta1, delta2, w[ORDER];
	static float alpha = 0.5 / (2 * 3.141592653589);
	float tmpvect[ORDER];
	double tmpwt[ORDER];

	//compute simpler weights
	/* Generate LSP(i) weights */
	for (i = 0; i < ORDER; i++) {
	    delta1 = (i == 0) ? lsp_nq[i] : lsp_nq[i] - lsp_nq[i - 1];
	    delta2 =
		(i ==
		 ORDER - 1) ? 0.5 - lsp_nq[i] : lsp_nq[i + 1] - lsp_nq[i];
	    delta = sqrt (delta1 * delta2);
	    tmpwt[i] = alpha / delta;
	}
	for (i = 0; i < LPCORDER; i++)
	    tmplsi[i] =
		(lsp_nq[i] - 0.4 * cbprev_E[i] - 0.2 * cbprevprev_E[i]);
	quantize_LSI2 (tmplsi, tmpwt, lsp, LSF_IDX);
	data_packet.LSP_IDX[0] = LSF_IDX[0];
	data_packet.LSP_IDX[1] = LSF_IDX[1];
	for (i = 0; i < LPCORDER; i++)
	    tmplsi[i] = lsp[i];
	for (i = 0; i < LPCORDER; i++) {
	    lsp[i] = 0.2 * cbprevprev_E[i] + 0.4 * cbprev_E[i] + lsp[i];
	    cbprevprev_E[i] = cbprev_E[i];
	    cbprev_E[i] = 2.5 * tmplsi[i];	//need this to make it MALSPVQ
	}

	{

	    if (lsp[0] < LSP_SPREAD_FACTOR_QPPP)
		lsp[0] = LSP_SPREAD_FACTOR_QPPP;
	    for (i = 1; i < LPCORDER; i++)
		if (lsp[i] - lsp[i - 1] < LSP_SPREAD_FACTOR_QPPP)
		    lsp[i] = lsp[i - 1] + LSP_SPREAD_FACTOR_QPPP;
	    if (0.5 - lsp[LPCORDER - 1] < 2 * LSP_SPREAD_FACTOR_QPPP) {
		lsp[LPCORDER - 1] = 0.5 - 2 * LSP_SPREAD_FACTOR_QPPP;
		i = LPCORDER - 2;
		while ((lsp[i + 1] - lsp[i] < LSP_SPREAD_FACTOR_QPPP)
		       && (i >= 0)) {
		    lsp[i] = MIN (lsp[i + 1] - LSP_SPREAD_FACTOR_QPPP, 0);
		    i--;
		}
	    }
	}


    }
    else if (PPP_MODE_E == 'F') {
	knum = 4;
	enc_lsp_vq_28 (lsp_nq, SScratch, lsp);
	for (i = 0; i < knum; i++)
	    data_packet.LSP_IDX[i] = SScratch[i];

	for (i = 0; i < ORDER; i++)
	    cbprevprev_E[i] = cbprev_E[i] = lsp[i];

    }
    else {
	fprintf (stderr, "Bad PPP_MODE '%c' in voiced.cc\n", PPP_MODE_E);
    }


    /* copied quantized LSPs */
    for (j = 0; j < ORDER; j++)
	global_lsp[j] = lsp[j];

    if (args->unquantized_lsp) {
	for (i = 0; i < ORDER; i++)
	    global_lsp[i] = lsp_nq[i];
	for (i = 0; i < ORDER; i++)
	    lsp[i] = lsp_nq[i];
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

    // Start Voiced/PPP  Processing

    float mres[FrameSize];
    float targ_speech[FrameSize];
    float acc3[NoOfSubFrames];

    /* Save target filters' memory for closed loop processing */
    for (i = 0; i < ORDER; i++)
	SynMemoryM2[i] = SynMemoryM[i];
    for (i = 0; i < ORDER; i++)
	WFmemFIR2[i] = WFmemFIR[i];
    for (i = 0; i < ORDER; i++)
	WFmemIIR2[i] = WFmemIIR[i];
    for (i = 0; i < ACBMemSize; i++)
	Excitation2[i] = Excitation[i];
    for (i = 0; i < dpm; i++)
	residualm2[i] = residualm[i];
    delay2 = delay, accshift2 = accshift, dpm2 = dpm, shiftSTATE2 =
	shiftSTATE;
    for (i = 0; i < ACBMemSize; i++)
	bufferm2[i] = bufferm[i];

    get_rcelp ();

    data_packet.delayindex = data_packet.DELAY_IDX;

    ///residualm_Frame has been populated by get_rcelp()
    for (j = 0; j < FSIZE; j++)
	copy[j] = mres[j] = residualm_frame[j];


    int pl = (int) rint (pdelay), l = (int) rint (delay);
    float *in = mres;

    //static float prev_cw_en;

    float tmp, res_enratio = 0, sp_enratio = 0;
    float tmplsp[ORDER], lpc1[ORDER], lpc2[ORDER];
    float SNR = 0.0;		// For Closed Loop Prediction SNR based BUMPUP

    //extern float PPP_TH;// For Closed Loop Prediction SNR based BUMPUP
    DTFS TMP, TMP2, TMP3, currp_q_E;

    Interpol (tmplsp, OldOldlspE, OldlspE, 2, ORDER);
    lsp2a (lpc1, tmplsp, ORDER);
    Interpol (tmplsp, OldlspE, lsp, 2, ORDER);
    lsp2a (lpc2, tmplsp, ORDER);

    if (lastrateE != 3 && lastrateE != 4)
	pl = l;
    if (pl > (int) anint (1.85 * l))
	pl /= 2;
    if (pl * 2 <= MAXLAG_PPP && pl <= (int) anint (0.54 * l))
	pl *= 2;

    //Use the out array as a temp storage for currp
    ppp_extract_pitch_period (in, out, l);
    currp_nq.to_fs (out, l);

    //Ensure the extracted prototype is time-synchronous to the
    //last l samples of the frame. This proves to eliminate
    //some of the PPP-CELP transition problems.
    TMP.to_fs (in + FSIZE - l, l);


    float postmp, negtmp, poscnq, negcnq;

    currp_nq.peaktoaverage (&poscnq, &negcnq);	//ppp
    TMP.peaktoaverage (&postmp, &negtmp);

    if (((postmp / poscnq) > 2) || ((poscnq / postmp) > 2)
	|| ((negtmp / negcnq) > 2) || ((negcnq / negtmp) > 2)) {
	jumptofullcelp (rate);
	return;
    }


    tmp = TMP.alignment_extract (currp_nq, 0.0, lpc2);
    currp_nq.phaseShift (TWO_PI * tmp / l);
    //Need to make sure the "in" array has the past 160 samples as well as
    //the current 160 samples available before using the getSCR function!
    //scr = getSCR(in, (pl+l)>>1) ;
    scr = 0.0;
	    /***TURN off SCR feature for the time being***/

    //Restoring PPP memories when the last frame is non-PPP or full-rate PPP
    if (LAST_PACKET_RATE_E == 1 || LAST_PACKET_RATE_E == 3
	|| LAST_MODE_BIT_E == 0) {
	prev_cw_E.to_fs (prev_en + ACBMemSize - pl, pl);
	ph_offset_E = 0.0;
	prev_cw_en = prev_cw_E.getEngy ();

	TMP = prev_cw_E;
	TMP.car2pol ();
	lastLgainE =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainE =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
	TMP.to_erb (lasterbE);

	LASTLAST_PPP_MODE_E = 'Q';
	LAST_PPP_MODE_E = 'F';
    }
    else if (LAST_PACKET_RATE_E == 4 && LAST_MODE_BIT_E == 1) {
	TMP = prev_cw_E;
	TMP.car2pol ();
	lastLgainE =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainE =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
	TMP.to_erb (lasterbE);
    }

    //Compute prototype-prediction-SNR
    //getpredSNR(curr_nq,prev_cw_E);
    //DECIDE IF prototype-prediction-SNR-based BUMPING UP IS NECESSARY
    //Need to compute this SNR if desired, else it is set to 0.0 dB now.



    if (args->rfileP == NULL) {
	//-----Open-loop Bump-Up

	//Energy ratio calculation in residual and speech domain
	//Also, compute correlation between the previous and the
	//current prototype.
	res_enratio = currp_nq.getEngy () / prev_cw_E.getEngy ();
	TMP = currp_nq, TMP2 = prev_cw_E;

	float tmptmp, tmptmp1, tmptmp2;

	tmptmp = TMP2.alignment_full (TMP, TMP.lag * 2);	//align of prev_cw wrt curr_cw, new method

	//tmptmp2=fmod(TMP2.lag-(tmptmp*TMP2.lag/TMP.lag),TMP2.lag); // (C_p-(tmptmp/C_l)*C_p)%C_p
	tmptmp1 = TMP.lag - tmptmp;	// (C_l-tmptmp), with or without fmod , bit-exact 
	tmp = tmptmp1;




	TMP.phaseShift (-TWO_PI * tmp / TMP.lag);	//fixed bug , phase shift by tmp computed in TMP.lag domain (above)
	float tmpres = TMP.freq_corr (TMP2, 100.0, 3700.0);

	TMP.poleFilter (lpc2, ORDER);



	TMP2.adjustLag (TMP.lag);	// operate in CL domain



	TMP2.poleFilter (lpc1, ORDER);
	tmp = TMP.freq_corr (TMP2, 100.0, 3700.0);
	sp_enratio = TMP.getEngy () / TMP2.getEngy ();



	if (PPP_MODE_E == 'Q' || PPP_MODE_E == 'H') {
	    //Bump up if the lag is out of range
	    if ((l - pl) > 8 || (l - pl) < -7)
		PPP_MODE_E = 'B';
	    else
		delta_lag_E = l - pl;

	    //Bump up if big change between the previous and the current CWs
	    if (args->operating_point == 1) {
		if (NS_SNR < 25.0) {
		    if ((res_enratio > 5.0) && (tmp < 0.65))
			PPP_MODE_E = 'B';
		}
		else {
		    if ((res_enratio > 3.0) && (tmp < 1.2))
			PPP_MODE_E = 'B';
		}
	    }
	    else if (args->operating_point == 2) {
		if (NS_SNR < 25.0) {
		    if ((res_enratio > 5.0) && (tmp < 0.65))
			PPP_MODE_E = 'B';
		}
		else {
		    if ((res_enratio > 3.0) && (tmp < 1.2))
			PPP_MODE_E = 'B';
		}
	    }
	}

	if (args->operating_point == 1) {
	    //Rapid rampdown frame where time resolution is important
	    //Not a suitable PPP frame -> Bump to CELP
	    if (NS_SNR < 25.0) {
		if (res_enratio < 0.025) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    else {
		if (res_enratio < 0.075) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    //Rapid rampup frame where time resolution is important
	    //Not a suitable PPP frame -> Bump to CELP 
	    if (NS_SNR < 25.0) {
		if (res_enratio > 14.5) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    else {
		if (res_enratio > 7.0) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	}
	if (args->operating_point == 2) {
	    //Rapid rampdown frame where time resolution is important
	    //Not a suitable PPP frame -> Bump to CELP
	    if (NS_SNR < 25.0) {
		if (res_enratio < 0.025) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    else {
		if (res_enratio < 0.075) {
		    jumptofullcelp (rate);
		    return;
		}
		if (MIN (res_enratio, sp_enratio) < 0.075 && tmp < 0.5) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    //Rapid rampup frame where time resolution is important
	    //Not a suitable PPP frame -> Bump to CELP 
	    if (NS_SNR < 25.0) {
		if (res_enratio > 14.5) {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	    else {
		if (res_enratio > 7.0) {
		    jumptofullcelp (rate);
		    return;
		}
		if (tmpres <= 0.0 && PPP_MODE_E == 'F') {
		    jumptofullcelp (rate);
		    return;
		}
	    }
	}

	//Bump up when the previous frame is an unvoiced or a silent frame
	if (lastrateE == 2 || lastrateE == 1) {
	    jumptofullcelp (rate);
	    return;
	}
	//-----End Open-loop Bump-Up

	//Quarter-rate or half-rate PPP Quantization
	if (PPP_MODE_E == 'Q' || PPP_MODE_E == 'H') {
	    bool flag = TRUE;

	    if (PPP_MODE_E == 'Q')
		flag =
		    ppp_quarter_encoder (&currp_q_E, prev_cw_E, currp_nq,
					 lpc2, &TMP);

	    if (flag) {
		//TMP: Target prototype: Amp Quantized + Phase Unquantized
		//TMP2: Quantized prototype: Amp Quantized + Phase Quantized
		//TMP3: Delta prototype: Diff betw. target and quant. in speech dom

		//-----Closed-loop Bump-Up
		float pos_nq, neg_nq, pos_q, neg_q;

		TMP.peaktoaverage (&pos_nq, &neg_nq);
		currp_q_E.peaktoaverage (&pos_q, &neg_q);
		//Before we perform the peak-to-average ratio comparison, we have to
		//ensure that the energy is not decaying and also the pitch pulse
		//is clearly defined
		if (args->operating_point == 1) {
		    if (NS_SNR < 25.0) {
			//Nothing
		    }
		    else {
			if ((currp_nq.getEngy () > 0.8 * prev_cw_en)
			    && (MAX (pos_nq, neg_nq) > 3.0)) {
			    if ((pos_nq > neg_nq) && (pos_nq > 2.0 * pos_q))
				PPP_MODE_E = 'B';
			    if ((pos_nq < neg_nq) && (neg_nq > 2.0 * neg_q))
				PPP_MODE_E = 'B';
			}
		    }
		}
		else if (args->operating_point == 2) {
		    if (NS_SNR < 25.0) {
			if ((currp_nq.getEngy () > 0.8 * prev_cw_en)
			    && (MAX (pos_nq, neg_nq) > 3.0)) {
			    if ((pos_nq > neg_nq) && (pos_nq > 2.0 * pos_q))
				PPP_MODE_E = 'B';
			    if ((pos_nq < neg_nq) && (neg_nq > 2.0 * neg_q))
				PPP_MODE_E = 'B';
			}
		    }
		    else {
			if ((currp_nq.getEngy () > prev_cw_en)
			    && (MAX (pos_nq, neg_nq) > 3.5)) {
			    if ((pos_nq > neg_nq) && (pos_nq > 2.5 * pos_q))
				PPP_MODE_E = 'B';
			    if ((pos_nq < neg_nq) && (neg_nq > 2.5 * neg_q))
				PPP_MODE_E = 'B';
			}
			float pos_nq0, neg_nq0;

			currp_nq.peaktoaverage (&pos_nq0, &neg_nq0);

			float impzi[160];
			float impzo[160];
			float mem[10];
			float energy_impz = 0.0;

			for (impzi[0] = 1.0, i = 1; i < 160; i++)
			    impzi[i] = 0.0;
			for (i = 0; i < 160; i++)
			    impzo[i] = 0.0;
			for (i = 0; i < 10; i++)
			    mem[i] = 0.0;

			SynthesisFilter (&impzo[0], &impzi[0], lpc2, &mem[0],
					 10, 160);
			for (i = 0; i < 160; i++)
			    energy_impz += (impzo[i] * impzo[i]);

			energy_impz = 10 * log10 (energy_impz);

			if ((currp_q_E.getEngy () > prev_cw_en)
			    && (MAX (pos_q, neg_q) > 3.5)
			    && energy_impz > 15.0 && tmpres > 0.7) {
			    if ((pos_q > neg_q)
				&& ((pos_q > 3.0 * pos_nq0)
				    || (pos_q > 1.5 * pos_nq0)
				    && (neg_q < 1.5 * neg_q)))
				PPP_MODE_E = 'B';
			    if ((pos_q <= neg_q)
				&& ((neg_q > 3.0 * neg_nq0)
				    || (neg_q > 1.5 * neg_nq0)
				    && (pos_q < 1.5 * pos_q)))
				PPP_MODE_E = 'B';
			}
		    }
		}

		TMP2 = currp_q_E;
		TMP.poleFilter (lpc2, ORDER);
		TMP2.poleFilter (lpc2, ORDER);
		TMP3 = TMP - TMP2;

		if (args->operating_point == 1) {
		    if (TMP.getEngy (1500.0, 3300.0) / TMP.getEngy () > 0.05)
			if (10.0 * log10 (TMP.getEngy (1500.0, 3300.0) /
					  TMP3.getEngy (1500.0,
							3300.0)) < 0.1)
			    if (res_enratio > 0.8)
				PPP_MODE_E = 'B';
		}

		//////////////to increase bump up, raise first threshold, lower second /////////////////////////
		tmp = 10.0 * log10 (TMP.getEngy () / TMP3.getEngy ());

		if (args->operating_point == 1) {
		    if (NS_SNR < 25.0) {
			if ((tmp < 3.05)
			    && (MAX (res_enratio, sp_enratio) > 0.8))
			    PPP_MODE_E = 'B';
		    }
		    else {
			if ((tmp < 4.5)
			    && (MAX (res_enratio, sp_enratio) > 0.5))
			    PPP_MODE_E = 'B';
		    }
		}
		else if (args->operating_point == 2) {
		    if (NS_SNR < 25.0) {
			// if ((tmp<2.0)&&(MAX(res_enratio,sp_enratio)>0.8)) PPP_MODE_E = 'B';
			// if ((tmp<3.0)&&(MAX(res_enratio,sp_enratio)>0.6)) PPP_MODE_E = 'B';
			// ADR TWEAK  1st above gives 3.9634, 2nd: 4.0225, want: 4.0006

			// if    ((tmp<2.6)&&(MAX(res_enratio,sp_enratio)>0.72)) PPP_MODE_E = 'B';
			// if    ((tmp<3.0)&&(MAX(res_enratio,sp_enratio)>0.6)) PPP_MODE_E = 'B';
			if ((tmp < 2.8)
			    && (MAX (res_enratio, sp_enratio) > 0.65))
			    PPP_MODE_E = 'B';
		    }
		    else {
			// if ((tmp<2.0)&&(MAX(res_enratio,sp_enratio)>1.0)) PPP_MODE_E = 'B';
			// if ((tmp<2.6)&&(MAX(res_enratio,sp_enratio)>0.9)) PPP_MODE_E = 'B';
			// ADR TWEAK  1st above gives 4.0478, 2nd: 4.0773, want: 4.0677
			if ((tmp < 2.4)
			    && (MAX (res_enratio, sp_enratio) > 0.94))
			    PPP_MODE_E = 'B';
		    }
		}
		//-----End closed-loop Bump-Up
		//////////////////////////////////////////////////////////////////////////////
	    }
	    else
		PPP_MODE_E = 'B';	//Amplitude quantization is failing
	}
	else if (PPP_MODE_E == 'F')
	    ppp_full_encoder (&currp_q_E, currp_nq, lpc2);
	else if (PPP_MODE_E != 'B')
	    fprintf (stderr, "Bad PPP_MODE '%c' in PPP\n", PPP_MODE_E);


	// Now Look for Bumpups either to FCELP or to FPPP when PPP_MODE_E='B'



	PPP_BUMPUP = 0;
	if (args->operating_point == 1) {
	    if (args->avg_rate_control) {

		if (PPP_MODE_E == 'Q') {
		    patterncount += pattern_m;
		    if (patterncount >= 1000) {
			patterncount -= 1000;
			PPP_MODE_E = 'B';
		    }
		}

	    }
	    if (PPP_MODE_E == 'B' || prev_dim_and_burstE) {
		PPP_BUMPUP = 1;	// Closed loop bumpup is true
		pppcountE = 0;	// To start QP-FC-FC pattern again
		{
		    jumptofullcelp (rate);
		    return;
		}
	    }
	}
	else if (args->operating_point == 2) {
	    if (args->avg_rate_control) {

		if (LAST_PPP_MODE_E == 'Q' && PPP_MODE_E == 'Q') {
		    patterncount += pattern_m;
		    if (patterncount >= 1000) {
			patterncount -= 1000;
			PPP_MODE_E = 'B';
		    }
		}

	    }

	    if (PPP_MODE_E == 'B' || prev_dim_and_burstE) {
		PPP_BUMPUP = 1;	// Closed loop bumpup is true
		{
		    jumptofullcelp (rate);
		    return;
		}
	    }
	}

	else {
	    fprintf (stderr, "Operating Point-%d in PPP??",
		     args->operating_point);
	    fprintf (stderr, "\n");
	    exit (-1);
	}
    }
    else {			// External Rate Commands
	if (PPP_MODE_E == 'Q') {
	    if (!((l - pl) <= 8 && (l - pl) >= -7)) {
		cerr <<
		    "ppp.cc - QPPP delta pitch out of range at the encoder - ";
		cerr << "FrameNum: " << encode_fcnt << "\n";
	    }
	    delta_lag_E = l - pl;
	    //Ignore the return flag from ppp_quarter_encoder
	    ppp_quarter_encoder (&currp_q_E, prev_cw_E, currp_nq, lpc2, &TMP);
	}
	else if (PPP_MODE_E == 'F')
	    ppp_full_encoder (&currp_q_E, currp_nq, lpc2);
	else {
	    fprintf (stderr, "Bad PPP_MODE '%c' in PPP\n", PPP_MODE_E);
	}
    }

    //Packetization of the delta lag in QPPP
    if (PPP_MODE_E == 'Q') {
	Q_delta_lag = delta_lag_E + 7;
	data_packet.Q_delta_lag = Q_delta_lag;
	if (args->rfileP == NULL)
	    assert (Q_delta_lag >= 0 && Q_delta_lag < 16);
    }
    //If QPPP put amp quant and power indices in data_packet structure
    if (PPP_MODE_E == 'Q') {
	data_packet.POWER_IDX = POWER_IDX;	// Power idx
	data_packet.AMP_IDX[0] = AMP_IDX[0];	// Low-band power idx
	data_packet.AMP_IDX[1] = AMP_IDX[1];	// hi-band power idx
    }
    //If FPPP put amp quant, power indices, and aligns in data_packet structure
    else if (PPP_MODE_E == 'F') {
	data_packet.F_ROT_IDX = F_ROT_IDX;	// Rotation
	data_packet.A_POWER_IDX = A_POWER_IDX;	// Power idx
	data_packet.A_AMP_IDX[0] = A_AMP_IDX[0];	// Low-band power idx
	data_packet.A_AMP_IDX[1] = A_AMP_IDX[1];	// hi-band power idx
	data_packet.A_AMP_IDX[2] = A_AMP_IDX[2];	// hi-band power idx
	for (i = 0; i < NUM_FIXED_BAND; i++)
	    data_packet.F_ALIGN_IDX[i] = F_ALIGN_IDX[i];
    }
    LASTLAST_PPP_MODE_E = LAST_PPP_MODE_E;
    LAST_PPP_MODE_E = PPP_MODE_E;

    if (args->unquantized_prototype)
	currp_q_E = currp_nq;

    //WI 2-D SYNTHESIS SCHEME
    WIsyn (prev_cw_E, &currp_q_E, lpc1, lpc2, &ph_offset_E, out, FSIZE);

    V_copy (out, SANITYCHECK, FSIZE);

    prev_cw_E = currp_q_E;
    prev_cw_en = currp_nq.getEngy ();


    if (args->accshift_out)
	write_samples (1, acc3, 3);
    if (args->mform_res_out)
	write_samples (1, mres, FrameSize);

    if (args->target_speech_out)
	write_samples (1, targ_speech, FrameSize);


    /* Update reconstruction filter memory */
    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;
	/* interpolate lsp */
	Interpol (lspi, OldlspE, lsp, i, ORDER);
	Interpol (lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);
	/* Convert lsp ,ORDERto PC */
	lsp2a (pci, lspi, ORDER);
	lsp2a (pci_nq, lspi_nq, ORDER);
	/* Update filters memory */
	ZeroInput (zir, pci_nq, pci, out + i * (SubFrameSize - 1), GAMMA1_NB,
		   GAMMA2, ORDER, subframesize, 1);

    }
    /* Update excitation */
    for (j = 0; j < ACBMemSize; j++)
	Excitation[j] = out[FrameSize - ACBMemSize + j];


    /* Update encoder variables */
    pdelay = rint (delay);

}

short
FGV_MEM::voiced_decoder (float *outFbuf, short post_filter,
			 short run_length, short phase_offset,
			 float time_warp_fraction, short *obuf_len)
{
    register int i, j;
    register float *foutP;
    short subframesize;
    float *ppp_res = new float[FrameSize * 2];	//Phase Matching: Increased size to support PM, Warping

    //extern float prevpD[ACBMemSize];

    //Variables used for Phase Matching

    short do_phase_matching = 0;
    int pm_var1 = 0;
    int temp_samples_count = 0;
    int num_samples;
    float store_ddelay;

    //End Variables used for Phase Matching

    if (phase_offset != 10)	//Phase Offset == 10 denotes Phase Matching disabled
	do_phase_matching = 1;	//check for Phase Matching


    // LSP De-Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (PPP_MODE_D == 'Q' || PPP_MODE_D == 'H') {
	LSF_IDX[0] = data_packet.LSP_IDX[0];
	LSF_IDX[1] = data_packet.LSP_IDX[1];
	unquantize_LSI2 (LSF_IDX, tmplsi);

	for (i = 0; i < LPCORDER; i++) {
	    lsp[i] = 0.2 * cbprevprev_D[i] + 0.4 * cbprev_D[i] + tmplsi[i];
	    cbprevprev_D[i] = cbprev_D[i];
	    cbprev_D[i] = 2.5 * tmplsi[i];	//need this to make it MALSPVQ
	}
	//stabilize_lsp(lsp);
	{
	    if (lsp[0] < LSP_SPREAD_FACTOR_QPPP)
		lsp[0] = LSP_SPREAD_FACTOR_QPPP;
	    for (i = 1; i < LPCORDER; i++)
		if (lsp[i] - lsp[i - 1] < LSP_SPREAD_FACTOR_QPPP)
		    lsp[i] = lsp[i - 1] + LSP_SPREAD_FACTOR_QPPP;
	    if (0.5 - lsp[LPCORDER - 1] < 2 * LSP_SPREAD_FACTOR_QPPP) {
		lsp[LPCORDER - 1] = 0.5 - 2 * LSP_SPREAD_FACTOR_QPPP;
		i = LPCORDER - 2;
		while ((lsp[i + 1] - lsp[i] < LSP_SPREAD_FACTOR_QPPP)
		       && (i >= 0)) {
		    lsp[i] = MIN (lsp[i + 1] - LSP_SPREAD_FACTOR_QPPP, 0);
		    i--;
		}
	    }
	}

	/* Check for monontonic LSP */
	for (j = 1; j < ORDER; j++)
	    if (lsp[j] <= lsp[j - 1])
		errorFlag = 1;


    }
    else if (PPP_MODE_D == 'F') {
	dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);
	/* Check for monontonic LSP */
	for (j = 1; j < ORDER; j++)
	    if (lsp[j] <= lsp[j - 1])
		errorFlag = 1;

	for (i = 0; i < ORDER; i++)
	    cbprevprev_D[i] = cbprev_D[i] = lsp[i];

	lsp_spread (lsp);


    }



    if (args->unquantized_lsp)
	for (i = 0; i < ORDER; i++)
	    lsp[i] = global_lsp[i];

    // Do Voiced/PPP Decoding

    short delayindex;

    delayindex = data_packet.delayindex;

    delay = (float) (delayindex + DMIN);
    if (PPP_MODE_D == 'Q')
	Q_delta_lag = data_packet.Q_delta_lag;
    int delta_lag_D = Q_delta_lag - 7;

    if (PPP_MODE_D == 'Q' || PPP_MODE_D == 'H') {
	if (!(delta_lag_D <= 8 && delta_lag_D >= -7)) {
	    cerr << "voiced.cc - Delta pitch out of range at the decoder -";
	    cerr << "FrameNum: " << decode_fcnt << "\n";
	}
	delay = rint (pdelayD) + delta_lag_D;
	pdeltaD = delta_lag_D;
	if (delay > DMAX)
	    delay = DMAX;
	else if (delay < DMIN)
	    delay = DMIN;
    }

    float *out = ppp_res;
    int pl = (int) rint (pdelayD), l = (int) rint (delay);
    float tmplsp[ORDER], lpc1[ORDER], lpc2[ORDER];
    DTFS TMP, currp_q_D;

    Interpol (tmplsp, OldOldlspD, OldlspD, 2, ORDER);
    lsp2a (lpc1, tmplsp, ORDER);
    Interpol (tmplsp, OldlspD, lsp, 2, ORDER);
    lsp2a (lpc2, tmplsp, ORDER);

    if (lastrateD != 3 && lastrateD != 4)
	pl = l;

    if (pl > (int) anint (1.85 * l))
	pl /= 2;
    if (pl * 2 <= MAXLAG_PPP && pl <= (int) anint (0.54 * l))
	pl *= 2;

    //Restoring PPP memories when the last frame is non-PPP or full-rate PPP
    if (LAST_PACKET_RATE_D == 1 || LAST_PACKET_RATE_D == 3
	|| LAST_MODE_BIT_D == 0 || prev_frame_error) {
	prev_cw_D.to_fs (prevpD + ACBMemSize - pl, pl);
	ph_offset_D = 0.0;

	TMP = prev_cw_D;
	TMP.car2pol ();
	lastLgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));


	TMP.to_erb (lasterbD);
    }
    else if (LAST_PACKET_RATE_D == 4 && LAST_MODE_BIT_D == 1) {
	TMP = prev_cw_D;
	TMP.car2pol ();
	lastLgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
	TMP.to_erb (lasterbD);
    }
    //If QPPP put amp quant and power indices in data_packet structure
    if (PPP_MODE_D == 'Q') {
	POWER_IDX = data_packet.POWER_IDX;	// Power idx
	AMP_IDX[0] = data_packet.AMP_IDX[0];	// Low-band power idx
	AMP_IDX[1] = data_packet.AMP_IDX[1];	// hi-band power idx
    }
    //If FPPP put amp quant, power indices, and aligns in data_packet structure
    else if (PPP_MODE_D == 'F') {
	F_ROT_IDX = data_packet.F_ROT_IDX;	// Rotation
	A_POWER_IDX = data_packet.A_POWER_IDX;	// Power idx
	A_AMP_IDX[0] = data_packet.A_AMP_IDX[0];	// Low-band power idx
	A_AMP_IDX[1] = data_packet.A_AMP_IDX[1];	// hi-band power idx
	A_AMP_IDX[2] = data_packet.A_AMP_IDX[2];	// hi-band power idx
	for (i = 0; i < NUM_FIXED_BAND; i++)
	    F_ALIGN_IDX[i] = data_packet.F_ALIGN_IDX[i];
    }

    //Prototype Dequantization
    //NOTE that currp_q_D has garbage at this point
    currp_q_D.lag = l;

    if (PPP_MODE_D == 'F')
	ppp_full_decoder (&currp_q_D, lpc2);
    else if (PPP_MODE_D == 'Q')
	ppp_quarter_decoder (&currp_q_D, prev_cw_D, lpc2);
    else
	assert (1 == 0);

    LAST_PPP_MODE_D = PPP_MODE_D;

    if (args->unquantized_prototype)
	currp_q_D = currp_nq;


    if (do_phase_matching) {
	double prev_delay, prev_delay2;
	double prev_delay_end, prev_delay2_end;
	double encoder_phase, decoder_phase;

	//if (idxppg != 0) //get delay of previous frame from current frame
	{
	    prev_delay_end = pdelayD;
	    prev_delay2_end = pdelayD;

	    prev_delay = pdelayD;
	    prev_delay2 = pdelayD;
	}

	//Different cases using Phase Matching
	//Get phase of encoder and current phase of decoder
	//The difference between these is the amount of Phase Matching to be done

	decoder_phase = encoder_phase = 0;

	if (phase_offset == 0) {
	    if (run_length == 1) {
		encoder_phase = fmod (160, prev_delay) / prev_delay;
		decoder_phase = fmod (160, pdelayD) / pdelayD;
	    }
	    else if (run_length == 2) {
		encoder_phase =
		    fmod (fmod (160, prev_delay) + fmod (160, prev_delay2),
			  prev_delay) / prev_delay;
		decoder_phase =
		    fmod (fmod (160, pdelayD) * 2, pdelayD) / pdelayD;
	    }
	}
	else if (phase_offset == 1) {
	    if (run_length == 1) {
		encoder_phase = 0;
		decoder_phase = fmod (160, pdelayD) / pdelayD;
	    }
	    else if (run_length == 2) {
		encoder_phase = fmod (160, prev_delay) / prev_delay;
		decoder_phase =
		    fmod (fmod (160, pdelayD) * 2, pdelayD) / pdelayD;
	    }
	}
	else if (phase_offset == 2) {
	    encoder_phase = 0;
	    decoder_phase = fmod (fmod (160, pdelayD) * 2, pdelayD) / pdelayD;
	}
	else if (phase_offset == -1) {
	    encoder_phase =
		fmod (fmod (160, prev_delay) + fmod (160, prev_delay2),
		      prev_delay) / prev_delay;
	    decoder_phase = fmod (160, pdelayD) / pdelayD;
	}

	//End Different cases using Phase Matching

	if (decoder_phase >= encoder_phase) {
	    pm_var1 = (int) ((decoder_phase - encoder_phase) * prev_delay);
	}
	else
	    pm_var1 =
		(int) (prev_delay -
		       prev_delay * (encoder_phase - decoder_phase));

	if (fabs (encoder_phase - decoder_phase) <= 0.05) {
	    pm_var1 = 0;	/* pm_var1 too small */
	}
	if (pm_var1 >= 100)
	    pm_var1 = 0;	/* pm_var1 too large */

	num_samples = 160 - pm_var1;
	if (num_samples < delay)
	    num_samples = 160;

	if (num_samples <= 120) {
	    if (time_warp_fraction < 1)
		time_warp_fraction = 1;
	    else
		time_warp_fraction = 1.5;
	}

	//printf ("\n ppp_delays %f %f %f %d %f", pdelayD, delay, delay, pm_var1, ph_offset_D); //fflush(stdout);

	if (time_warp_fraction > 1 || time_warp_fraction < 1) {
	    if (time_warp_fraction > 1)	//Expansion
	    {
		if (phase_offset == -1 && (num_samples + 3 * delay) <= 320) {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, (int) (num_samples + 3 * delay));
		    num_samples += (int) (3 * delay);
		}
		else if (phase_offset == -1
			 && (num_samples + 2 * delay) <= 320) {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, (int) (num_samples + 2 * delay));
		    num_samples += (int) (2 * delay);
		}
		else {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, (int) (num_samples + delay));
		    num_samples += (int) delay;
		}
	    }
	    else if (time_warp_fraction < 1)	//Compression
	    {
		//printf ("\n Checking PPP condition PM %d %d", (int)(num_samples - delay), currp_q_D.lag);
		//Do PPP compression only if num_samples - delay > currp_q_D.lag
		//Else do CELP-like compression
		if ((int) (num_samples - delay) > currp_q_D.lag) {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, (int) (num_samples - delay));
		    num_samples -= (int) delay;
		}
		else {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, num_samples);
		    int b, merge_start, merge_end, merge_stop;

		    merge_start = 0;

		    if ((prev_cw_D.lag) - floor ((float) prev_cw_D.lag) > 0.5)
			merge_end = (int) prev_cw_D.lag + 1;
		    else
			merge_end = (int) prev_cw_D.lag;

		    if (merge_end - merge_start > num_samples / 2)
			merge_stop = num_samples - (merge_end - merge_start);
		    else
			merge_stop = merge_end - merge_start;

		    if (num_samples <= prev_cw_D.lag)
			merge_stop = 0;

		    if (merge_stop != 0) {
			//Regular compression as requested by de-jitter: produce one less pitch period
			//Compress only if compressed samples >= 40
			if ((num_samples - (merge_end - merge_start)) >= 20) {
			    for (b = merge_start; b < merge_stop; b++)
				ppp_res[b] =
				    ppp_res[b] * (1.0 *
						  (merge_stop - merge_start -
						   b) / (merge_stop -
							 merge_start)) +
				    ppp_res[b +
					    (merge_end -
					     merge_start)] * (1.0 * (b) /
							      (merge_stop -
							       merge_start));

			    for (b = merge_stop;
				 b + merge_end - merge_start < num_samples;
				 b++)
				ppp_res[b] =
				    ppp_res[b + merge_end - merge_start];

			    num_samples -= (merge_end - merge_start);
			}
		    }
		}
	    }
	}
	else			// No Compression or Expansion
	    //WI 2-D SYNTHESIS SCHEME
	    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D, out,
		   num_samples);
    }
    else {
	num_samples = 160;

	if (time_warp_fraction > 1 || time_warp_fraction < 1) {
	    if (time_warp_fraction > 1)	//Expansion
	    {
		WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D, out,
		       (int) (num_samples + delay));
		num_samples += (int) delay;
	    }
	    else if (time_warp_fraction < 1)	//Compression
	    {
		//printf ("\n Checking PPP condition %d %d", (int)(num_samples - delay), currp_q_D.lag);
		//Do PPP compression only if num_samples - delay > currp_q_D.lag
		//Else do CELP-like compression
		if ((int) (num_samples - delay) > currp_q_D.lag) {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, (int) (num_samples - delay));
		    num_samples -= (int) delay;
		}
		else {
		    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D,
			   out, num_samples);

		    int b, merge_start, merge_end, merge_stop;

		    merge_start = 0;

		    if ((prev_cw_D.lag) - floor ((float) prev_cw_D.lag) > 0.5)
			merge_end = (int) prev_cw_D.lag + 1;
		    else
			merge_end = (int) prev_cw_D.lag;

		    if (merge_end - merge_start > num_samples / 2)
			merge_stop = num_samples - (merge_end - merge_start);
		    else
			merge_stop = merge_end - merge_start;

		    if (num_samples <= prev_cw_D.lag)
			merge_stop = 0;
		    //merge_stop = 0;
		    //printf ("\n Checking PPP %d %d %d %d %d %f", merge_stop, num_samples, merge_end, currp_q_D.lag, prev_cw_D.lag, delay);
		    if (merge_stop != 0) {
			//Regular compression as requested by de-jitter: produce one less pitch period
			//Compress only if compressed samples >= 40
			if ((num_samples - (merge_end - merge_start)) >= 20) {
			    for (b = merge_start; b < merge_stop; b++)
				ppp_res[b] =
				    ppp_res[b] * (1.0 *
						  (merge_stop - merge_start -
						   b) / (merge_stop -
							 merge_start)) +
				    ppp_res[b +
					    (merge_end -
					     merge_start)] * (1.0 * (b) /
							      (merge_stop -
							       merge_start));

			    for (b = merge_stop;
				 b + merge_end - merge_start < num_samples;
				 b++)
				ppp_res[b] =
				    ppp_res[b + merge_end - merge_start];

			    num_samples -= (merge_end - merge_start);
			}
		    }
		}
	    }
	}
	else {			// No Compression or Expansion
	    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D, out,
		   num_samples);

	    //printf ("\n voiced num_samples %d", num_samples);
	}
    }

    //Sanity check - Ensure encoder and decoder matches
    if (!args->decode_only && args->erasure_file == NULL)
	for (i = 0; i < FrameSize; i++)
	    if (out[i] != SANITYCHECK[i]) {
		cout << "Encoder-Decoder Mismatch in PPP; FrameNum: " <<
		    decode_fcnt << "\t" << out[i] << "\t" << SANITYCHECK[i] <<
		    "\n";
		i = FrameSize;
	    }

    /* Update fer coefficients */
    ave_acb_gain =
	MIN (1.0, sqrt (sqrt (currp_q_D.getEngy () / prev_cw_D.getEngy ())));
    ave_fcb_gain = 0.0;

    prev_cw_D = currp_q_D;


    foutP = outFbuf;

    //More support for Phase Matching/Warping
    //printf ("\n voiced num_samples %d, fr_cnt:%d", num_samples,decode_fcnt);
    *obuf_len = num_samples;
    temp_samples_count = 0;

    //End: More support for Phase Matching/Warping

    for (i = 0; i < NoOfSubFrames; i++) {

	//Warping: Change in size of subframesize, not always equal to 53 since residual size may not be = 160

	if (num_samples % 3 == 0)
	    subframesize = num_samples / 3;
	else if (num_samples % 3 == 1) {
	    if (i < 2)
		subframesize = num_samples / 3;
	    else
		subframesize = num_samples / 3 + 1;
	}
	else if (num_samples % 3 == 2) {
	    if (i < 1)
		subframesize = num_samples / 3;
	    else
		subframesize = num_samples / 3 + 1;
	}
	//End Change in size of subframesize, not always equal to 53

	Interpol (lspi, OldlspD, lsp, i, ORDER);
	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);

	//Warping: Slight change in call to support smaller/larger number of samples
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] = ppp_res[temp_samples_count + j];

	//Warping: Slight change in call to support smaller/larger number of samples
	if (args->unquantized_half_rate)
	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[ACBMemSize + j] =
		    copy[i * (SubFrameSize - 1) + j];
	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];
	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);






	/* Synthesis of decoder output signal and postfilter output signal */
	SynthesisFilter (DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory,
			 ORDER, subframesize);
	/* Postfilter */
	if (post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 delay, 0.4, 0.15, subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);
	/* Write p.f. decoder output and variables to files */
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

	temp_samples_count += subframesize;	//Addition to support Warping
    }
    delete[]ppp_res;
    pdelayD = delay;

    return (0);
}

void
FGV_MEM::voiced_erasure_decoder (float *outFbuf, short post_filter)
{
    register int i, j;
    register float *foutP;
    short subframesize;
    float *ppp_res = new float[FrameSize];

    //extern float prevpD[ACBMemSize];

    delay = rint (pdelayD);

    float *out = ppp_res;
    int pl = (int) rint (pdelayD), l = pl;
    float tmp, tmplsp[ORDER], lpc1[ORDER], lpc2[ORDER];
    DTFS TMP, currp_q_D;

    PPP_MODE_D = 'E';

    Interpol (tmplsp, OldOldlspD, OldlspD, 2, ORDER);
    lsp2a (lpc1, tmplsp, ORDER);
    Interpol (tmplsp, OldlspD, lsp, 2, ORDER);
    lsp2a (lpc2, tmplsp, ORDER);

    if (lastrateD != 3 && lastrateD != 4)
	pl = l;

    if (pl > (int) anint (1.85 * l))
	pl /= 2;
    if (pl * 2 <= MAXLAG_PPP && pl <= (int) anint (0.54 * l))
	pl *= 2;

    //Restoring PPP memories when the last frame is non-PPP or full-rate PPP
    if (LAST_PACKET_RATE_D == 1 || LAST_PACKET_RATE_D == 3
	|| LAST_MODE_BIT_D == 0 || prev_frame_error) {
	prev_cw_D.to_fs (prevpD + ACBMemSize - pl, pl);
	ph_offset_D = 0.0;

	TMP = prev_cw_D;
	TMP.car2pol ();
	lastLgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
	TMP.to_erb (lasterbD);
    }
    else if (LAST_PACKET_RATE_D == 4 && LAST_MODE_BIT_D == 1) {
	TMP = prev_cw_D;
	TMP.car2pol ();
	lastLgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
	lastHgainD =
	    log10 (TMP.lag *
		   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0, 1.0));
	TMP.to_erb (lasterbD);
    }

    //Prototype Repeat with an appropriate scaling
    currp_q_D = prev_cw_D * ave_acb_gain;

    //Use the RCELP extended pitch memory to get the phase of the current prototype
    {
	DTFS TMP, TMP2;
	float temp_pl = pdelayD, temp_l = l;	//delay, pdelayD
	float temp_acb[ACBMemSize + SubFrameSize + 10], temp_res[160],
	    delayD3[3];

	//Extending the ACB memory
	for (i = 0; i < ACBMemSize; i++)
	    temp_acb[i] = PitchMemoryD[i];
	for (i = 0; i < NoOfSubFrames; i++) {
	    if (i < 2)
		subframesize = SubFrameSize - 1;
	    else
		subframesize = SubFrameSize;

	    // Interpolate delay
	    Interpol_delay (delayD3, &temp_pl, &temp_l, i);
	    // Compute adaptive codebook contribution
	    acb_excitation (temp_acb + ACBMemSize, 1, delayD3, temp_acb,
			    subframesize);

	    for (j = 0; j < subframesize; j++)
		temp_res[j + i * (SubFrameSize - 1)] =
		    temp_acb[j + ACBMemSize];
	    for (j = 0; j < ACBMemSize; j++)
		temp_acb[j] = temp_acb[j + subframesize];
	}

	//Perform extraction on the ACB extended memory
	//Using temp_acb as a temporary storage for the extracted prototype
	ppp_extract_pitch_period (temp_res, temp_acb, l);
	TMP.to_fs (temp_acb, l);

	//Ensure the extracted prototype is time-synchronous to the
	//last l samples of the frame. This proves to eliminate
	//some of the PPP-CELP transition problems.
	TMP2.to_fs (temp_res + FSIZE - l, l);
	tmp = TMP2.alignment_extract (TMP, 0.0, lpc2);
	TMP.phaseShift (TWO_PI * tmp / l);

	//Copying phase spectrum over
	TMP.car2pol ();
	currp_q_D.car2pol ();
	for (i = 0; i <= currp_q_D.lag >> 1; i++)
	    currp_q_D.b[i] = TMP.b[i];
	currp_q_D.pol2car ();
    }

    LAST_PPP_MODE_D = PPP_MODE_D;

    if (args->unquantized_prototype)
	currp_q_D = currp_nq;


    //WI 2-D SYNTHESIS SCHEME
    WIsyn (prev_cw_D, &currp_q_D, lpc1, lpc2, &ph_offset_D, out, FSIZE);

    prev_cw_D = currp_q_D;


    foutP = outFbuf;
    for (i = 0; i < NoOfSubFrames; i++) {
	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;
	Interpol (lspi, OldlspD, lsp, i, ORDER);
	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] =
		ppp_res[i * (SubFrameSize - 1) + j];
	if (args->unquantized_half_rate) {	/*Do Nothing */
	}
	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	for (j = 0; j < ACBMemSize; j++)
	    PitchPreFiltMemoryD[j] = PitchMemoryD[j];
	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);
	/* Synthesis of decoder output signal and postfilter output signal */
	SynthesisFilter (DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory,
			 ORDER, subframesize);
	/* Postfilter */
	if (post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 delay, 0.4, 0.15, subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);
	/* Write p.f. decoder output and variables to files */
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
    delete[]ppp_res;
    pdelayD = delay;
}

DTFS
freq_trim (DTFS freqtrim_in, float cutoff, float *ENERGY_TO_B_ADDED,
	   float *SIGNAL_ENERGY, float *LP_SIGNAL_ENERGY, int *subframecount)
{
    int ncount;
    float freq;

    *ENERGY_TO_B_ADDED = 0.0;
    DTFS freqtrim_out;

    freqtrim_out = freqtrim_in;

    for (ncount = 0; ncount <= freqtrim_in.lag >> 1; ncount++) {
	freq = 8000 * ncount / freqtrim_in.lag;
	if (freq > cutoff) {
	    //Removing only 1/2 the energy above voicing threshold
	    freqtrim_out.a[ncount] /= sqrt (2.0);
	    freqtrim_out.b[ncount] /= sqrt (2.0);
	}
    }

    *ENERGY_TO_B_ADDED = (freqtrim_out.getEngy (cutoff, 4000.0)) * 160;
    *SIGNAL_ENERGY =
	(freqtrim_in.lag * (freqtrim_in.getEngy (0, 4000.0))) * 160 /
	freqtrim_in.lag;
    *LP_SIGNAL_ENERGY =
	(freqtrim_out.lag * (freqtrim_out.getEngy (0, 4000.0))) * 160 /
	freqtrim_out.lag;
    *subframecount = (int) floor ((double) FSIZE / freqtrim_in.lag);

    return freqtrim_out;
}
