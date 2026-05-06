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
    "$Id: celp.cc,v 1.86.4.6 2007/06/24 06:43:05 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/* This is the original EVRC full rate coder with ACELP */
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "acelp.h"		/* for ACELP fixed codebook */
#include  "struct.h"

#include "filt.h"


#define FER_INT_FAC_INX 4

#include "upgrades.h"



short encode_da_fp (int *daindex, unsigned short fcb[][4]);
void decode_da_fp (int *daindex, unsigned short fcb[][4]);







float A_CLDA = A_DEFAULT;
float B_CLDA = B_DEFAULT;
float SMAX_CLDA = S_DEFAULT;

float THRESH_CLDA = T_DEFAULT;
float ACBG_TH = ABCG_DFLT;




void pitchPreFilter (float *input, float *state, float *delay3, short length, float gppf, float &gppf_sm,	//float &ge_sm,
		     float *output);


float ppf_coef = PPF_COEF;








static float fcb_filt_num_coef[5] = {
    0.592041015625000,
    2.330688476562500,
    3.477539062500000,
    2.330688476562500,
    0.592041015625000,
};
static float fcb_filt_den_coef[5] = {
    1.000000000000000,
    2.922851562500000,
    3.364868164062500,
    1.776367187500000,
    0.366821289062500,
};


static float nr_preemph[2] = { 1, 0.2 }, dr_preemph[1] =
{
1};


void
FGV_MEM::lsp_spread (float *lsp)
{

    int i;

    if (data_packet.WB_MODE_BIT == 1) {
	if (lsp[0] < LSP_SPREAD_FACTOR_FX)
	    lsp[0] = LSP_SPREAD_FACTOR_FX;
	for (i = 1; i < LPCORDER; i++)
	    if (lsp[i] - lsp[i - 1] < LSP_SPREAD_FACTOR_FX)
		lsp[i] = lsp[i - 1] + LSP_SPREAD_FACTOR_FX;
	if (0.5 - lsp[LPCORDER - 1] < 2 * LSP_SPREAD_FACTOR_FX) {
	    lsp[LPCORDER - 1] = 0.5 - 2 * LSP_SPREAD_FACTOR_FX;
	    i = LPCORDER - 2;
	    while ((lsp[i + 1] - lsp[i] < LSP_SPREAD_FACTOR_FX) && (i >= 0)) {
		lsp[i] = MIN (lsp[i + 1] - LSP_SPREAD_FACTOR_FX, 0);
		i--;
	    }
	}
    }
    else {
	if (lsp[0] < LSP_SPREAD_FACTOR_FX_NB)
	    lsp[0] = LSP_SPREAD_FACTOR_FX_NB;
	for (i = 1; i < LPCORDER; i++)
	    if (lsp[i] - lsp[i - 1] < LSP_SPREAD_FACTOR_FX_NB)
		lsp[i] = lsp[i - 1] + LSP_SPREAD_FACTOR_FX_NB;
	if (0.5 - lsp[LPCORDER - 1] < 2 * LSP_SPREAD_FACTOR_FX_NB) {
	    lsp[LPCORDER - 1] = 0.5 - 2 * LSP_SPREAD_FACTOR_FX_NB;
	    i = LPCORDER - 2;
	    while ((lsp[i + 1] - lsp[i] < LSP_SPREAD_FACTOR_FX_NB)
		   && (i >= 0)) {
		lsp[i] = MIN (lsp[i + 1] - LSP_SPREAD_FACTOR_FX_NB, 0);
		i--;
	    }
	}



    }

}


void
FGV_MEM::celp_encoder (int bit_rate)
{
    register int i, j, n;
    float delayi[3];
    short subframesize;
    short Aveidxppg;
    float sum1;
    float tmpf;

    float ghnw;


    float Hsnr[SubFrameSize];
    float worigm_snr[SubFrameSize];
    float SynTmp[SubFrameSize];

    float CopyExconvH[SubFrameSize];

    float tmpbuf1[2 * SubFrameSize];
    float tmpbuf2[2 * SubFrameSize];
    double tmp_fcb_filt_mem[4];

    POLEZERO_FILTER tmp_fcb_filt = { 4, 4, 4, 0, tmp_fcb_filt_mem };


    if (lastrateE != 4) {	//pre-emphasis and formant filtering performed during celp too
	preemphmem[0] = 0.0;
	for (j = 0; j < ORDER; j++)
	    FfiltMem[j] = 0.0;
    }


    // Gain Quantization Setups
    if (data_packet.WB_MODE_BIT == 1) {
	if (bit_rate == 3) {
	    FCBGainSize = 16;
	    gnvq = gnvq_4;
	}
	else


	{
	    FCBGainSize = 32;
	    gnvq = gnvq_8;
	}

    }
    else {			//Narrowband coding
	if (bit_rate == 3) {
	    FCBGainSize = 16;
	    gnvq = gnvq_4;
	}
	else {
	    FCBGainSize = 32;
	    gnvq = gnvq_8;
	}

    }


    // Start Full/Half CELP Processing

    // Update shiftSTATE with hysteresis 
/*Note that the get_rcelp() function is no longer there in WB code...segments of that function
are now in-line below*/
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

    if (data_packet.WB_MODE_BIT == 1) {

	if (bit_rate == 4) {

	    if (delay > 40)
		data_packet.DELAY_IDX = (int) delay;
	    else {
		data_packet.DELAY_IDX = (int) ((delay - DMIN) * 2);
	    }
	}

	else if (bit_rate == 3) {
	    if (delay > 40)
		data_packet.DELAY_IDX = (int) (delay - DMIN);
	    else {		//fractional
		if (rint (delay) == delay)
		    data_packet.DELAY_IDX = (int) (delay - DMIN);
		else
		    data_packet.DELAY_IDX = (int) (delay + 80.5);	// maps delays of
		//[20.5, 21.5, 22.5 , ......, 39.5] --> [101,102,103,...., 120]

		// to work around SPL packets and TTY identifiers
		if (data_packet.DELAY_IDX == 110)
		    data_packet.DELAY_IDX = 124;
		if (data_packet.DELAY_IDX == 120)
		    data_packet.DELAY_IDX = 125;
		// implies in the range [121 -127] the lags 123 (SPL_HCELP_WB), 124, 125 ( above, to accomodate SPL NB packets and
		//TTY lags) and 126,127 ( WB_HR) are used up and CANNOT be used for bad-rate detections for HCELP 
	    }

	}




    }
    else {			//Narrowband case
	data_packet.DELAY_IDX = (int) (delay - DMIN);
    }

    if (lastrateE < 3 || (lastrateE == 4 && prev_celp_mdct_dec == 1))
	pdelay = delay;

    //Prepare for the delta delay
    j = (short) (delay - pdelay);
    if (abs (j) > 15)
	j = 0;
    else
	j = j + 16;
    assert (j >= 0 && j < 32);
    data_packet.DELTA_DELAY_IDX = (unsigned short) j;
    if (data_packet.WB_MODE_BIT == 1) {
	/* Smooth interpolation if the difference between delays is too big */
	if (fabs (delay - pdelay) > 15)
	    pdelay = delay;

	Aveidxppg = 0;


	if (lastrateE != 4) {
	    for (j = 0; j < ORDER; j++)
		FfiltMem[j] = 0.0;
	    preemphmem[0] = 0.0;
	}
    }
    else {			//Narrowband case
	if (rate == 3 && !rcelp_half_rateE && !dim_and_burstE)
	    assert (fabs (delay - pdelay) <= 15);
	/* Smooth interpolation if the difference between delays is too big  */
	if (fabs (delay - pdelay) > 15)
	    pdelay = delay;

	//float residualm_frame[FrameSize+EXTRA];
	for (i = 0; i < NoOfSubFrames; i++) {
	    if (i < 2)
		subframesize = SubFrameSize - 1;
	    else
		subframesize = SubFrameSize;

	    /* interpolate lsp   */


	    Interpol (lspi, OldlspE, lsp, i, ORDER);

	    Interpol (lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);

	    /* Convert lsp to PC   */


	    lsp2a (pci, lspi, ORDER);

	    lsp2a (pci_nq, lspi_nq, ORDER);


	    /* Interpolate delay     */
	    Interpol_delay (delayi, &pdelay, &delay, i);

	    ComputeACB (residualm, Excitation + ACBMemSize, delayi,
			residual + GUARD + i * (SubFrameSize - 1),
			FrameSize + GUARD - i * (SubFrameSize - 1), &dpm,
			&accshift, beta, subframesize, RSHIFT);

	    for (j = 0; j < subframesize; j++)
		residualm_frame[i * (SubFrameSize - 1) + j] = residualm[j];

	    /* Get weighted speech  */
	    /* ORIGM */



	    SynthesisFilter (mspeech + i * (SubFrameSize - 1), residualm, pci,
			     SynMemoryM, ORDER, subframesize);


	    /* Weighting filter */
	    weight (wpci, pci_nq, GAMMA1_NB, ORDER);
	    fir (Scratch, mspeech + i * (SubFrameSize - 1), wpci, WFmemFIR,
		 ORDER, subframesize);
	    weight (wpci, pci_nq, GAMMA2, ORDER);
	    iir (wspeech + i * (SubFrameSize - 1), Scratch, wpci, WFmemIIR,
		 ORDER, subframesize);

	    /* Update residualm   */
	    for (j = 0; j < dpm; j++)
		residualm[j] = residualm[j + subframesize];

	}

	Aveidxppg = 0;



    }				//end of Narrowband else                      

  /*********************************
   * CELP codebook search procedure *
   *********************************/
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

	/* Get zir */

	if (data_packet.WB_MODE_BIT == 1)
	    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1,
		       GAMMA2, ORDER, subframesize, 0);
	else
	    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1_NB,
		       GAMMA2, ORDER, subframesize, 0);

	if (data_packet.WB_MODE_BIT == 1) {


	    /* Calculate impulse response of 1/A(z) * A(z/g1) / A(z/g2) */



	    ImpulseRzp (H, pci_nq, pci, GAMMA1, GAMMA2, ORDER, Hlength);



	}
	else			//narrowband modes
	    ImpulseRzp (H, pci_nq, pci, GAMMA1_NB, GAMMA2, ORDER, Hlength);


	/* Interpolate delay */
	Interpol_delay (delayi, &pdelay, &delay, i);
	if (data_packet.WB_MODE_BIT == 1) {


	    ComputeACB (residualm, Excitation + ACBMemSize, delayi,
			residual + GUARD + i * (SubFrameSize - 1),
			FrameSize + GUARD - i * (SubFrameSize - 1), &dpm,
			&accshift, beta, subframesize, RSHIFT);
/* UB Specific stuff */
	    frame_shift[i] = accshift;




	}
	else {			//Narrowband case
	    for (j = 0; j < subframesize; j++)
		residualm[j] = residualm_frame[i * (SubFrameSize - 1) + j];
	    //get excitation, by extending ACB
	    putacbc (Excitation + ACBMemSize, Excitation + ACBMemSize, 0,
		     subframesize, 10, delayi, BLFREQ_NB, BLPRECISION);



	}


	for (j = 0; j < subframesize; j++)
	    copy[i * (SubFrameSize - 1) + j] = residualm[j];

	if (data_packet.WB_MODE_BIT == 1) {
	    /* Get weighted speech */
	    /* ORIGM */
	    SynthesisFilter (origm, residualm, pci, SynMemoryM, ORDER,
			     subframesize);

	    if (args->target_speech_out)
		write_samples (1, origm, subframesize);


	}
	else {			//Narrowband case

	    //origm being copied into only for writing out target_speech_out
	    for (j = 0; j < subframesize; j++)
		origm[j] = mspeech[i * (SubFrameSize - 1) + j];
	    if (args->target_speech_out)
		write_samples (1, origm, subframesize);

	    for (j = 0; j < subframesize; j++)
		worigm[j] = wspeech[i * (SubFrameSize - 1) + j];


	    /* Remove Zero input response from weighted speech */
	    for (j = 0; j < subframesize; j++)
		worigm[j] -= zir[j];

	    /* Calculate closed loop gain */
	    getgain (Excitation + ACBMemSize, &sum1, H, &idxppg, ppvq,
		     ppvq_mid, ACBGainSize, 1, worigm, subframesize, Hlength);

	    Aveidxppg += idxppg;

	    /* Get TARGET for fixed c.b. */
	    /* Convolve excitation with H */
	    /* ExconvH stored in Scratch memory */
	    ConvolveImpulseR (ExconvH, Excitation + ACBMemSize, H, Hlength,
			      subframesize);
	    for (j = 0; j < subframesize; j++)
		TARGETw[j] = worigm[j] - ExconvH[j];
	    for (; j < SubFrameSize; j++)
		TARGETw[j] = 0.0;	//to avoid UMR in Weight2Res function

	    /* Convert TARGET from weighted domain to residual domain */
	    Weight2Res (TARGET, TARGETw, pci_nq, pci, GAMMA1_NB, GAMMA2,
			ORDER, SubFrameSize);

	    if (subframesize < SubFrameSize)
		TARGETw[subframesize] = TARGET[subframesize] =
		    Scratch[subframesize] = 0.0;
	}			//end of else for narrowband modes

	if (data_packet.WB_MODE_BIT == 1) {
	    /* Weighting filter */
	    weight (wpci, pci_nq, GAMMA1, ORDER);
	    fir (Scratch, origm, wpci, WFmemFIR, ORDER, subframesize);


	    wpci[0] = DEEMPH_FAC;
	    iir (worigm, Scratch, wpci, WFmemIIR, 1, subframesize);


	    if ((prev_celp_mdct_dec == 1 || lastrateE < 3) && i == 0) {

		for (j = 0; j < 4; j++)
		    fcb_target_lpfmem[j] = 0.0;
	    }
	    POLEZERO_FILTER fcb_target_filt =
		{ 4, 4, 4, 0, fcb_target_lpfmem };
	    float temp_worigm[SubFrameSize];

	    for (j = 0; j < subframesize; j++)
		temp_worigm[j] = worigm[j];
	    polezero_filter (temp_worigm, worigm, subframesize,
			     fcb_filt_num_coef, fcb_filt_den_coef,
			     fcb_target_filt);



	    for (j = 0; j < subframesize; j++) {
		worig_celp[i * (SubFrameSize - 1) + j] = worigm[j];
	    }


	}
	else {			//narrowband modes
	    /* get delay for current subframe */
	    n = (short) ((delayi[1] + delayi[0]) / 2.0 + 0.5);
	    /* Compute fixed codebook contribution */
	    if (n > subframesize)
		n = 200;

	    if (bit_rate == 4) {
		/* ACELP fixed codebook search */
		ACELP_Code (TARGETw, TARGET, H, (int) n, sum1, subframesize,
			    Scratch, &fcbGain, y2, fcbIndexVector, 1);
	    }
	    else {		//bit_rate==3 case
		cod3_10jcelp (TARGETw, H, (int) n, ppvq[idxppg], subframesize,
			      Scratch, &fcbGain, fcbIndexVector);
	    }


	    /* constrain fcb gain */
	    fcbGain *= (1.0 - ppvq[idxppg] * 0.15);

	    if (lastrateE <= 2) {
		if (i == 0)
		    fcbGain *= 0.45;
		else if (i == 1)
		    fcbGain *= 0.7;
		else
		    fcbGain *= 0.85;
	    }
	    /* quantize fcb gain */
	    for (idxcbg = 0, j = 1; j < FCBGainSize; ++j) {
		if (fcbGain > (gnvq[j] + gnvq[j - 1]) / 2.0)
		    idxcbg = j;
	    }
	    fcbGain = gnvq[idxcbg];
	}

	if (data_packet.WB_MODE_BIT == 1) {

	    {



#define HNWStateExtra 0

		if (i == 0 && encode_fcnt - 1 != celp_hnw_encode_fcnt_last) {
		    for (j = 0; j < ACBMemSize + SubFrameSize + HNWStateExtra; j++) {
			celp_hnw_state[j] = 0.0;
		    }
		    for (j = 0; j < ACBMemSize + SubFrameSize; j++) {
			celp_hnw_zir_state[j] = 0.0;
		    }
		}
		else {
		    for (j = 0; j < ACBMemSize + HNWStateExtra; j++) {
			celp_hnw_state[j] = celp_hnw_state[celp_hnw_last_subframesize + j];
		    }
		    for (j = 0; j < ACBMemSize; j++) {
			celp_hnw_zir_state[j] =
			    celp_hnw_zir_state[celp_hnw_last_subframesize + j];
		    }
		}

		celp_hnw_encode_fcnt_last = encode_fcnt;
		celp_hnw_last_subframesize = subframesize;


		ghnw = beta;


		hnwFilter (worigm, celp_hnw_state + ACBMemSize + HNWStateExtra,
			   delayi, subframesize, ghnw, worigm);

#define HNW_DUMP 0

		hnwZeroState (H, delayi, Hlength, ghnw, H);

		hnwZeroInput (zir, celp_hnw_zir_state, delayi, subframesize, ghnw,
			      zir);


	    }


	    //reset memory if previous frame is not FCELP

	    if ((prev_celp_mdct_dec == 1 || lastrateE < 3) && i == 0) {	//1st subframe and prev frame not CELP
		for (j = 0; j < 4; j++)
		    fcb_filt_mem[j] = 0.0;
	    }

	    //compute ZIR of fcb_lpf
	    for (j = 0; j < 4; j++)
		tmp_fcb_filt_mem[j] = fcb_filt_mem[j];
	    for (j = 0; j < subframesize; j++)
		tmpbuf1[j] = 0.0;
	    polezero_filter (tmpbuf1, tmpbuf2, subframesize,
			     fcb_filt_num_coef, fcb_filt_den_coef,
			     tmp_fcb_filt);

	    //convolve ZIR of fcb_lpf with Impulseresponse of weighting filter
	    //to get ZIR->ZSR of weighting filter
	    //tmpbuf2 is input, tmpbuf1 is output
	    ConvolveImpulseR (tmpbuf1, tmpbuf2, H, Hlength, subframesize);

	    //add to zir that will be subtracted from target
	    for (j = 0; j < subframesize; j++)
		zir[j] += tmpbuf1[j];


	    /* Remove Zero input response from weighted speech */
	    for (j = 0; j < subframesize; j++)
		worigm[j] -= zir[j];


	    /* Save weighted impulse response for SNR calculation (pitch sharpening trashes H) */
	    for (j = 0; j < Hlength; j++) {
		Hsnr[j] = H[j];
	    }


	    if (bit_rate == 4) {

		{

		    float delayAdjust, deltaDelayAdjust, delayStd,
			delayStdFuncOut;

		    static const float delayAdjustCodebook[5] =
			{ 0.0, 1.0, -1.0, 2.0, -2.0 };




		    int jbest;
		    float polarity;

		    float delayCandidate[3];

		    float cross, ener;
		    float crossBest, enerBest, delayAdjustBest;

		    float exBest[SubFrameSize];
		    float temp[SubFrameSize];
		    float temp1[SubFrameSize];

		    int k;



		    if (i == 0) {
			celp_delay_x0 = celp_delay_x1;
			celp_delay_x1 = delay;



			if ((prev_celp_mdct_dec == 1 || lastrateE < 3)) {

			    celp_delay_x0 = celp_delay_x1;
			}


			delayStd = 0.707106781186547 * fabs (celp_delay_x0 - celp_delay_x1);
			delayStdFuncOut = Min ((A_CLDA * delayStd + B_CLDA), SMAX_CLDA);	/// These are tunable
			deltaDelayAdjust = delayStdFuncOut * (celp_delay_x0 + celp_delay_x1) / 2.0;
		    }


		    polarity = +1.0;


		    float mseBest = 1e38;

		    for (j = 0; j < 5; j++) {

			delayAdjust =
			    polarity * deltaDelayAdjust *
			    delayAdjustCodebook[j];

			delayCandidate[0] =
			    Min (Max (delayi[0] + delayAdjust, DMIN), DMAX);
			delayCandidate[1] =
			    Min (Max (delayi[1] + delayAdjust, DMIN), DMAX);
			delayCandidate[2] =
			    Min (Max (delayi[2] + delayAdjust, DMIN), DMAX);

			if (data_packet.WB_MODE_BIT == 1)
			    putacbc (Excitation + ACBMemSize,
				     Excitation + ACBMemSize, 0, subframesize,
				     0, delayCandidate, BLFREQ, BLPRECISION);
			else
			    putacbc (Excitation + ACBMemSize,
				     Excitation + ACBMemSize, 0, subframesize,
				     0, delayCandidate, BLFREQ_NB,
				     BLPRECISION);

			ConvolveImpulseR (temp, Excitation + ACBMemSize, H,
					  Hlength, subframesize);

			cross = ener = 0.0;
			for (k = 0; k < subframesize; k++) {
			    cross += worigm[k] * temp[k];
			    ener += temp[k] * temp[k];
			}



			float a, mse = 0.0, gain;

			gain = Min (1.2, Max (0.0, cross / (ener + 1.0)));
			for (k = 0; k < subframesize; k++) {
			    a = worigm[k] - gain * temp[k];
			    mse += a * a;
			}

			float predGain = 0.0;

			for (k = 0; k < subframesize; k++) {
			    predGain += worigm[k] * worigm[k];
			}
			predGain /= mse;

			if (j == 0 || (mse < mseBest && predGain > ACBG_TH)) {	// ACBG_TH => prediction gain threshold

			    if (j == 0) {
				mseBest = mse / THRESH_CLDA;
			    }
			    else {
				mseBest = mse;
			    }
			    jbest = j;
			    delayAdjustBest = delayAdjust;
			    for (k = 0; k < subframesize; k++) {
				exBest[k] = Excitation[ACBMemSize + k];
			    }
			}

		    }

		    data_packet.DELAY_ADJ_IDX[i] = jbest;


		    da_index[i] = jbest;	// pass info to decoder & cod7_35()


		    delayi[0] =
			Min (Max ((delayi[0] + delayAdjustBest), DMIN), DMAX);
		    delayi[1] =
			Min (Max ((delayi[1] + delayAdjustBest), DMIN), DMAX);
		    delayi[2] =
			Min (Max ((delayi[2] + delayAdjustBest), DMIN), DMAX);


		    for (k = 0; k < subframesize; k++) {
			Excitation[ACBMemSize + k] = exBest[k];
		    }

		}
	    }			//end if (if bit_rate==4)


	    n = (short) ((delayi[1] + delayi[0]) / 2.0 + 0.5);
	    if ((n >= RATIO_FACTOR * subframesize) && (bit_rate == 4)
		&& (lastrateE > LASTRATE_TH)) {
		getgain (Excitation + ACBMemSize, &sum1, H, &idxppg, ppvq,
			 ppvq_mid, ACBGainSize, 0, worigm, subframesize,
			 Hlength);
	    }
	    else
	    {
		getgain (Excitation + ACBMemSize, &sum1, H, &idxppg, ppvq,
			 ppvq_mid, ACBGainSize, 1, worigm, subframesize,
			 Hlength);

		for (j = 0; j < subframesize; j++)
		    Excitation[ACBMemSize + j] *= sum1;

	    }



	    Aveidxppg += idxppg;

	    /* Get TARGET for fixed c.b. */
	    /* Convolve excitation with H */
	    /* ExconvH stored in Scratch memory */



	    ConvolveImpulseR (ExconvH, Excitation + ACBMemSize, H, Hlength,
			      subframesize);



	    if ((n >= RATIO_FACTOR * subframesize) && (bit_rate == 4)
		&& (lastrateE > LASTRATE_TH)) {
		for (j = 0; j < subframesize; j++)
		    TARGETw[j] = worigm[j] - sum1 * ExconvH[j];
	    }
	    else
	    {
		for (j = 0; j < subframesize; j++)
		    TARGETw[j] = worigm[j] - ExconvH[j];
	    }

	    for (; j < SubFrameSize; j++)
		TARGETw[j] = 0.0;	//to avoid UMR in Weight2Res function

	    /* Convert TARGET from weighted domain to residual domain */


	    if (subframesize < SubFrameSize)
		TARGETw[subframesize] = TARGET[subframesize] =
		    Scratch[subframesize] = 0.0;


	    /* get delay for current subframe */
	    n = (short) ((delayi[1] + delayi[0]) / 2.0 + 0.5);
	    /* Compute fixed codebook contribution */
	    if (n > subframesize)
		n = 200;



	    //pass Impresp through ZSR of fcb_filt
	    for (j = 0; j < 4; j++)
		tmp_fcb_filt_mem[j] = 0.0;


	    polezero_filter (H, tmpbuf2, Hlength, fcb_filt_num_coef,
			     fcb_filt_den_coef, tmp_fcb_filt);

	    if (bit_rate == 4) {	//position of this statement moved from line 1322 to enable FCB LPF on half rate CELP


		/* ACELP fixed codebook search */

		if ((n >= RATIO_FACTOR * subframesize)
		    && (lastrateE > LASTRATE_TH)) {
		    float enerjs = 100;
		    float acbgainmismatch;

		    acbgainmismatch = fabs (sum1 - ppvq[ACBGainSize - 1]);
		    for (j = 1; j < ACBGainSize; j++) {
			if (ppvq[j] > sum1)
			    break;
		    }
		    if (j < ACBGainSize) {
			if (fabs (sum1 - ppvq[j - 1]) < fabs (sum1 - ppvq[j])) {
			    acbgainmismatch = fabs (sum1 - ppvq[j - 1]);
			}
			else {
			    acbgainmismatch = fabs (sum1 - ppvq[j]);
			}

		    }


		    for (j = 0; j < subframesize; j++)
			CopyExconvH[j] = ExconvH[j];
		    for (j = 0; j < subframesize; j++)
			enerjs += ExconvH[j] * ExconvH[j];
		    enerjs = 1.0 / sqrt (1.01 * enerjs);
		    if (sum1 >= 1.15)
			enerjs *= (1.3 - sum1);
		    if (sum1 <= 0.05)
			enerjs *= sum1;
		    enerjs *= 0.97;
		    if (n < subframesize)
			enerjs *= 0.98 * (0.76 + 1.5 * acbgainmismatch);
		    for (j = 0; j < subframesize; j++)
			y2[j] = ExconvH[j] * enerjs;
		}
		else
		    for (j = 0; j < subframesize; j++)
			y2[j] = 0;


		ACELP_Code (TARGETw, TARGET, tmpbuf2, (int) n, sum1,
			    subframesize, Scratch, &fcbGain, y2,
			    fcbIndexVector, 1);




		if (i == 0) {
		    Er = 0;
		    for (j = GUARD; j < GUARD + FrameSize; j++) {
			Er += residual[j] * residual[j];
		    }
		    Er = sqrt (Er);
		}



		/* Quantize frame residual energy */
		if (i == 0) {
		    float D;
		    float tmp, sum;
		    float Etmp = log (Er);	// Er has sqrt() applied

		    for (j = 0, sum = 1e30; j < NUM_Q_LEVELS_E; j++) {
			D = Etmp - (MIN_LOG_ENERGY_E +
				    (float) j * (MAX_LOG_ENERGY_E -
						 MIN_LOG_ENERGY_E) /
				    NUM_Q_LEVELS_E);
			tmp = D * D;
			if (tmp < sum) {
			    sum = tmp;
			    idxe = j;
			}
		    }
		}
		Erq_ln =
		    (MIN_LOG_ENERGY_E +
		     (float) idxe * (MAX_LOG_ENERGY_E -
				     MIN_LOG_ENERGY_E) / NUM_Q_LEVELS_E);

		Erq = exp (Erq_ln);

		ckck = 1.0e-10;
		for (j = 0; j < subframesize; j++) {
		    ckck += Scratch[j] * Scratch[j];
		}






		if ((n >= RATIO_FACTOR * subframesize)
		    && (lastrateE > LASTRATE_TH)) {

		    gainvq (worigm, CopyExconvH, y2, Excitation + ACBMemSize,
			    &fcbGain, &sum1, &idxppg, &idxcbg, ppvq, gnvq,
			    ACBGainSize, FCBGainSize, n, subframesize);


		    for (j = 0; j < subframesize; j++)
			Excitation[ACBMemSize + j] *= sum1;
		}

	    }



	    if (bit_rate != 4) {	//bit_rate==3 case
		cod3_10jcelp (TARGETw, tmpbuf2, (int) n, ppvq[idxppg],
			      subframesize, Scratch, &fcbGain,
			      fcbIndexVector);
	    }

	    if (bit_rate == 3) {
		fprintf (stderr, "\t\t\t%d -- half rate\n", encode_fcnt);
	    }


	    /* constrain fcb gain */

	    if ((n < RATIO_FACTOR * subframesize) || (bit_rate != 4)
		|| (lastrateE <= LASTRATE_TH))
	    {
		if (bit_rate == 4) {


		    fcbGain *= (1.0 - ppvq[idxppg] * 0.15);





		    /* quantize fcb gain */




		    gainfcbsearch (TARGETw, y2, &fcbGain, sum1, &idxcbg,
				   subframesize);




		}		//end of if bit_rate==4
		else {		//FCB Gain Quantization for half rate (executed only if bitrate==3)
		    fcbGain *= (1.0 - ppvq[idxppg] * 0.15);
		    if (lastrateE <= 2) {
			if (i == 0)
			    fcbGain *= 0.45;
			else if (i == 1)
			    fcbGain *= 0.7;
			else
			    fcbGain *= 0.85;
		    }
		    for (idxcbg = 0, j = 1; j < FCBGainSize; ++j) {
			if (fcbGain > (gnvq[j] + gnvq[j - 1]) / 2.0)
			    idxcbg = j;
		    }
		    fcbGain = gnvq[idxcbg];
		}

	    }


	    // do the fcb lpf, updating the memory
	    for (j = 0; j < subframesize; j++)
		Scratch[j] *= fcbGain;
	    POLEZERO_FILTER fcb_filt = { 4, 4, 4, 0, fcb_filt_mem };

	    polezero_filter (Scratch, tmpbuf2, subframesize,
			     fcb_filt_num_coef, fcb_filt_den_coef, fcb_filt);



	    /* Add to total excitation */
	    for (j = 0; j < subframesize; j++)
		Excitation[j + ACBMemSize] += tmpbuf2[j];


	    if (i == 1) {
		for (j = 0; j < 16; j++)
		    prev_synthSave[j] = Excitation[ACBMemSize + 38 + j];
	    }
	    if (i == 2)
		for (j = 0; j < SubFrameSize; j++)
		    prev_synthSave[j + 16] = Excitation[ACBMemSize + j];



	    /* Synthesize through weighted synthesis filter */
	    ConvolveImpulseR (SynTmp, Excitation + ACBMemSize, Hsnr,
			      subframesize, subframesize);

	    for (j = 0; j < subframesize; j++)
		SynTmp[j] += (zir[j] - tmpbuf1[j]);



	    for (j = 0; j < subframesize; j++)
		worigm_snr[j] = worigm[j] + zir[j];
	    wsnr (worigm_snr, SynTmp, i, subframesize);

	    /*populate for celp/ mdct discriminator */
	    for (j = 0; j < subframesize; j++) {
		wsynth_celp[i * (SubFrameSize - 1) + j] = SynTmp[j];
	    }


	    {
		float filt_exct[54];

		if (i == 0 && ((encode_fcnt - 1 != celp_ppf_encode_fcnt_last))) {
		    for (j = 0; j < ACBMemSize + SubFrameSize; j++) {
			celp_ppf_state[j] = 0.0;
		    }
		}
		celp_ppf_encode_fcnt_last = encode_fcnt;
		pitchPreFilter (Excitation + ACBMemSize, celp_ppf_state, delayi,
				subframesize, ppf_coef * sum1, celp_gppf_sm,
				filt_exct);

		for (j = 0; j < subframesize; j++)
		    P_LB.whtnd_la[i * (SubFrameSize - 1) + j] = filt_exct[j];
	    }


	    {
		float tmp_env, tmp_gain, x2;
		float *buf_ptr;
		int k;
		float fb1 = 0.97;
		float ff1 = 0.14;
		float fb2 = 0.98;
		float ff2 = 0.1;

		if (bit_rate == 4) {

		    if ((prev_celp_mdct_dec == 1 || lastrateE != 4) && (i == 0))	// reset state only for 1st sub-frame - bug fix

		    {
			celp_env1 = 1.0;
			celp_env2 = 1.0;
		    }
		    buf_ptr = &(P_LB.whtnd_la[i * (SubFrameSize - 1)]);

		    for (k = 0; k < subframesize; k++) {
			x2 = buf_ptr[k] * buf_ptr[k];

			tmp_env = ff1 * x2 + (1 - ff1) * celp_env1;
			celp_env1 *= fb1;
			if (tmp_env > celp_env1)
			    celp_env1 = tmp_env;

			tmp_env = ff2 * x2 + (1 - ff2) * celp_env2;
			celp_env2 *= fb2;
			if (tmp_env > celp_env2)
			    celp_env2 = tmp_env;

			tmp_gain = celp_env1 / celp_env2;
			tmp_gain = tmp_gain > 1.1 ? 1.1 : tmp_gain;
			tmp_gain = tmp_gain < 0.5 ? 0.5 : tmp_gain;

			buf_ptr[k] *= tmp_gain;

		    }
		}
	    }

	}
	else {			//narrowband case

	    /* Add to total excitation */
	    for (j = 0; j < subframesize; j++)
		Excitation[j + ACBMemSize] += Scratch[j] * fcbGain;

	}
	if (data_packet.WB_MODE_BIT == 1)
	    /* Update filters memory */
	    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1,
		       GAMMA2, ORDER, subframesize, 1);

	else
	    ZeroInput (zir, pci_nq, pci, Excitation + ACBMemSize, GAMMA1_NB,
		       GAMMA2, ORDER, subframesize, 1);


	if (data_packet.WB_MODE_BIT == 1) {

	    {
		for (j = 0; j < subframesize; j++) {
		    celp_hnw_zir_state[ACBMemSize + j] = zir[j];
		}
	    }

	}


	/* Update residualm */
	for (j = 0; j < dpm; j++)
	    residualm[j] = residualm[j + subframesize];

	/* Update excitation */
	for (j = 0; j < ACBMemSize; j++)
	    Excitation[j] = Excitation[j + subframesize];

	if (bit_rate == 4) {
	    /* Pack bits */
	    /* ACB gain index */
	    data_packet.ACBG_IDX[i] = idxppg;
	    idxcb = fcbIndexVector[0];
	    data_packet.FCB_PULSE_IDX[i][0] = idxcb;
	    idxcb = fcbIndexVector[1];
	    data_packet.FCB_PULSE_IDX[i][1] = idxcb;
	    idxcb = fcbIndexVector[2];
	    data_packet.FCB_PULSE_IDX[i][2] = idxcb;
	    idxcb = fcbIndexVector[3];
	    data_packet.FCB_PULSE_IDX[i][3] = idxcb;
	    if (data_packet.WB_MODE_BIT == 1) {

	    }
	    /* FCB gain index */
	    data_packet.FCBG_IDX[i] = idxcbg;
	}
	else {			//bit_rate==3 case
	    /* Pack bits */
	    /* ACB gain index */
	    data_packet.ACBG_IDX[i] = idxppg;
	    idxcb = fcbIndexVector[0];
	    //Bitpack(idxcb, (unsigned short *) PackedWords, 11, PackWdsPtr);
	    data_packet.FCB_PULSE_IDX[i][0] = idxcb;
	    /* FCB gain index */
	    data_packet.FCBG_IDX[i] = idxcbg;
	}
    }

    if (data_packet.WB_MODE_BIT == 1) {
	if (bit_rate == 4) {


	    encode_da_fp (da_index, data_packet.FCB_PULSE_IDX);

	}



    }
    pdelay = delay;

    //packing routine
    PktPtr[0] = 16;
    PktPtr[1] = 0;

    if (bit_rate == 4) {	//full rate
	data_packet.PACKET_RATE = 4;
	data_packet.MODE_BIT = 0;
    }
    else {			//for(bit_rate==3) 
	data_packet.PACKET_RATE = 3;
    }

}



/* second order normalized lattice filter */
static void
norm_lattice_biquad (float *speech_out,	// memory should have been allocated beforehand
		     float *State,	// state vector (size 2)
		     const float *speech_in, const float *BCoef,	// 2 MA coefficients (filter is monic)
		     const float *ACoef,	// 2 AR coefficients
		     int len)
{
    float S1, S2, C1, C2, v0, v1, v2, tmp;
    int n;

    S1 = ACoef[1];
    S2 = ACoef[0] / (1.0 + ACoef[1]);
    C1 = sqrt (1.0 - S1 * S1);
    C2 = sqrt (1.0 - S2 * S2);
    v2 = BCoef[1];
    v1 = (BCoef[0] - BCoef[1] * ACoef[0]) / C1;
    v0 = (1.0 - C1 * S2 * v1 - S1 * v2) / (C1 * C2);

    for (n = 0; n < len; n++) {
	tmp = C1 * speech_in[n] - S1 * State[0];
	speech_out[n] = v2 * (S1 * speech_in[n] + C1 * State[0]);
	State[0] = S2 * tmp + C2 * State[1];
	speech_out[n] += v1 * State[0];
	State[1] = C2 * tmp - S2 * State[1];
	speech_out[n] += v0 * State[1];
    }
}


/* boost first harmonic for speech with low fundamental frequency */
void
FGV_MEM::bass_boost (float *speech, float lag, int len)
{


    /* second order filter */
    float r1, r2, A_bb[2], B_bb[2];
    int n;

    /* gain */
    r1 = 0.99 - 0.5 / lag;
    for (n = 0; n < len; n++)
	speech[n] *= r1;

    r1 = 1.001 - 3.0 / lag;
    r2 = 1.001 - 2.5 / lag;
    A_bb[0] = -2.0 * cos ((1.0 + 0.0015 * lag) * 3.1415926 / lag) * r1;
    A_bb[1] = r1 * r1;
    B_bb[0] = -2.0 * cos ((0.2 + 0.0015 * lag) * 3.1415926 / lag) * r2;
    B_bb[1] = r2 * r2;
    norm_lattice_biquad (speech, mem_BB, speech, B_bb, A_bb, len);

}



void
ImpulseRzp_new (float *output, float *coef, short order, short length)
{
    register short i, j;
    float SUM;
    float memory[ORDER];
    float wcoef[ORDER];

    /* ImpulseR of 1/A(Z) */
    output[0] = 1.0;
    memory[0] = 1.0;

    for (i = 1; i < order; i++)
	memory[i] = 0;
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = 0; j > 0; j--) {
	    SUM -= coef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM -= coef[0] * memory[0];
	*memory = SUM;
	output[i] = SUM;
    }
}


float
det_interpol (float *lspnew, float *lspold, int ordL, int SFS)
{
    float pcold[ORDER], pc[ORDER];
    float H[100];
    float PG_old = 0.0, PG_new = 0.0, ratio;
    int j;


    lsp2a (pc, lspold, ORDER);
    ImpulseRzp_new (H, pc, ORDER, SFS);
    /* Get energy of H */
    for (j = 0, PG_old = 0; j < SFS; j++)
	PG_old += H[j] * H[j];

    lsp2a (pc, lspnew, ORDER);
    ImpulseRzp_new (H, pc, ORDER, SFS);
    /* Get energy of H */
    for (j = 0, PG_new = 0; j < SFS; j++)
	PG_new += H[j] * H[j];

    ratio = PG_old / PG_new;
    return (ratio);

}




short
FGV_MEM::celp_decoder (float *outFbuf, short post_filter, int bit_rate,
		       float time_warp_fraction, short *oBuf_len)
{
    register int i, j, n;
    register float *foutP;
    float delayi[3];
    short subframesize;
    float sum1;
    float sum_acb;

    //Time-Warping: Increase size of PitchMemoryD_Frame below
    float PitchMemoryD_Frame[160 * 2];
    DTFS LAST_BAD_CW;
    bool local_lpc_flag = FALSE;

    float g2;			// For 2nd pitch filter

    //Time-Warping: New Variables for supporting warping
    short do_phase_matching = 0;
    int num_residual = 160;
    int pm_var1 = 0, pm_var2;
    int temp_samples_count = 0;
    int num_samples;
    float temp_pitch[320];
    int b, c, merge_start, merge_end, merge_stop, sample_counter;
    float store_ddelay;

    float prev_ave_fcb_gain = 0.0, curr_ave_acb_gain = 0.0;
    int ct = 0;

    //End: Time-Warping: New Variables for supporting warping

    POLEZERO_FILTER PreemphState = { 1, 0, 1, 0, dec_preemphmem };


    float filt_exct[54], agc_fac, PM_ltffilt[160];



    float tmpbuf2[SubFrameSize];

    // compiler error ok if used with above...  never tested
    float delayi_adjusted[3][3];	//[subframe][endpoints]


    if (data_packet.WB_MODE_BIT == 0) {	//narrowband specific thing
	if (phase_offset != 10)	//Phase Offset == 10 denotes Phase Matching disabled
	    do_phase_matching = 1;	//check for Phase Matching
    }
    else {			//FCB LPF defined only for wideband modes

	//reset memory if previous frame is not FCELP
	if (lastrateD < 3 || (lastrateD == 4 && prev_celp_mdct_dec == 1)) {
	    for (j = 0; j < 4; j++)
		fcb_filt_mem_dec[j] = 0.0;
	}

    }
    LAST_BAD_CW.lag = 0;

    // Gain Quantization Setups
    if (data_packet.WB_MODE_BIT == 1) {

	if (bit_rate == 3) {
	    FCBGainSize = 16;
	    gnvq = gnvq_4;
	}
	else


	{
	    FCBGainSize = 32;
	    gnvq = gnvq_8;

//gautam:check to see if mode bit rate is 4
	    if (data_packet.MODE_BIT != 0) {
		cerr << "The mode bit rate is not set to 0 in Frame no :" <<
		    decode_fcnt << "\n";
	    }
	}
    }
    else {			//narrowband specific
	if (bit_rate == 3) {
	    FCBGainSize = 16;
	    gnvq = gnvq_4;
	}
	else {
	    FCBGainSize = 32;
	    gnvq = gnvq_8;

	}
    }

    if (data_packet.WB_MODE_BIT == 1) {
	if (lastrateD != 4) {
	    dec_preemphmem[0] = 0.0;
	    for (j = 0; j < ORDER; j++)
		dec_FormantFilterMemory[j] = 0.0;
	}
    }




    // LSP De-Quantization
    float tmplsi[ORDER], tmplsc[ORDER];

    if (data_packet.WB_MODE_BIT == 1) {
	if (bit_rate == 3) {
	    if (SPL_HCELP_WB == 0)
		dec_lsp_vq_22 ((short int *) data_packet.LSP_IDX, lsp);
	    else
		dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);
	}
	else {




	    dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);




	}
    }
    else {			//narrowband case
	if (bit_rate == 3) {
	    dec_lsp_vq_22 ((short int *) data_packet.LSP_IDX, lsp);
	}
	else {
	    dec_lsp_vq_28 ((short int *) data_packet.LSP_IDX, lsp);
	}
    }

    /* Check for monontonic LSP */
    if (data_packet.WB_MODE_BIT == 0)
	for (j = 1; j < ORDER; j++)
	    if (lsp[j] <= lsp[j - 1])
		errorFlag = 1;

    for (i = 0; i < ORDER; i++)
	cbprevprev_D[i] = cbprev_D[i] = lsp[i];



    lsp_spread (lsp);


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
    // Do CELP Decoding

    if (data_packet.WB_MODE_BIT == 1) {


	if ((bit_rate == 4) || (SPL_HCELP_WB == 1)) {

	    if (data_packet.DELAY_IDX > 40) {
		delay = (float) data_packet.DELAY_IDX;
	    }
	    else {
		delay = DMIN + data_packet.DELAY_IDX * (0.5);
	    }
	}

	else if (bit_rate == 3) {
	    if (data_packet.DELAY_IDX <= 100) {
		data_packet.DELAY_IDX += DMIN;
		delay = (float) data_packet.DELAY_IDX;
	    }
	    else		//fractional
	    {
		if (data_packet.DELAY_IDX == 124)
		    data_packet.DELAY_IDX = 110;
		if (data_packet.DELAY_IDX == 125)
		    data_packet.DELAY_IDX = 120;
		delay = data_packet.DELAY_IDX - 80.5;
	    }

	}




    }
    else {			//narrowband case 
	data_packet.DELAY_IDX += DMIN;
	delay = (float) data_packet.DELAY_IDX;

	store_ddelay = data_packet.DELTA_DELAY_IDX;	//Phase Matching: need to store ddelay

	if (lastrateD < 3)
	    pdelayD = delay;

	/* Fix delay countour of previous erased frame */
	// Fix delay contour only if no Phase Matching; else PM fixes it anyway

	if (do_phase_matching)
	    LAST_BAD_CW.to_fs (PitchMemoryD + ACBMemSize - (int) pdelayD,
			       (int) pdelayD);
    }
    if (data_packet.WB_MODE_BIT == 1)
	if (lastrateD < 3 || (lastrateD == 4 && prev_celp_mdct_dec == 1))
	    pdelayD = delay;

    /* Fix delay countour of previous erased frame */

    if ((data_packet.WB_MODE_BIT == 0 && !do_phase_matching)
	|| data_packet.WB_MODE_BIT == 1) {
	if (fer_counter == 2) {
	    float lpc_gain1, lpc_gain2;
	    float LPCG_THD;

	    if (lastrateD == 4)
		LPCG_THD = 2.5;
	    else
		LPCG_THD = 5.0;

	    lsp2a (pci, OldlspD, ORDER);
	    ImpulseRzp (H, pci, pci, 1.0, 1.0, ORDER, Hlength);
	    /* Get energy of H */
	    for (j = 0, lpc_gain1 = 0; j < Hlength; j++)
		lpc_gain1 += H[j] * H[j];
	    lsp2a (pci, lsp, ORDER);
	    ImpulseRzp (H, pci, pci, 1.0, 1.0, ORDER, Hlength);
	    /* Get energy of H */
	    for (j = 0, lpc_gain2 = 0; j < Hlength; j++)
		lpc_gain2 += H[j] * H[j];
	    /* determine spectral transistion degree (set flag if large) --
	       for frame erasures */
	    if (lpc_gain1 / lpc_gain2 > LPCG_THD) {
		local_lpc_flag = TRUE;
	    }
	}
	if (data_packet.WB_MODE_BIT == 1) {


	    j = 0;


	}
	else {			//narrowband case
	    j = data_packet.DELTA_DELAY_IDX;
	}

	if (lastrateD <= 2 && fer_counter == 2)
	    data_packet.ACBG_IDX[0] = 0;

	if (bit_rate == 4 && lastrateD > 2 && j != 0
	    && (fer_counter == 2
		|| (fer_counter == 1 && lastrateD == 3 && prev_rcelp_half == 0
		    && LAST_PPP_MODE_D == 'Q' && lastlastrateD > 2)))


	{

	    pdelayD = delay - (float) (j - 16);

	    LAST_BAD_CW.to_fs (PitchMemoryD + ACBMemSize - (int) pdelayD,
			       (int) pdelayD);

	    if (fer_counter == 2) {
		if (!prev_prev_frame_error) {
		    if (fabs (pdelayD - pdelayD2) > 15)
			pdelayD2 = pdelayD;
		    for (i = 0; i < ACBMemSize; i++)
			PitchMemoryD[i] = PitchMemoryD2[i];
		    for (i = 0; i < NoOfSubFrames; i++) {
			if (i < 2)
			    subframesize = SubFrameSize - 1;
			else
			    subframesize = SubFrameSize;
			// Interpolate delay
			Interpol_delay (delayi, &pdelayD2, &pdelayD, i);
			if (data_packet.WB_MODE_BIT == 1) {


#  warning 'Need to adjust delay contour here! (?)'


			}
			// Compute adaptive codebook contribution
			if (i == 0)
			    sum_acb = ave_acb_gain;
			else
			    sum_acb = 1.0;
			acb_excitation (PitchMemoryD + ACBMemSize, sum_acb,
					delayi, PitchMemoryD, subframesize);
			for (j = 0; j < ACBMemSize; j++)
			    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
		    }
		}
	    }
	    else {
		pdelayD2 = pdelayD - pdeltaD;
		for (i = 0; i < ACBMemSize; i++)
		    PitchMemoryD[i] = PitchMemoryD3[i];
		for (i = 0; i < NoOfSubFrames; i++) {
		    if (i < 2)
			subframesize = SubFrameSize - 1;
		    else
			subframesize = SubFrameSize;
		    // Interpolate delay
		    Interpol_delay (delayi, &pdelayD3, &pdelayD2, i);
		    if (data_packet.WB_MODE_BIT == 1) {


#  warning 'Need to adjust delay contour here! (?)'


		    }
		    // Compute adaptive codebook contribution
		    if (i == 0)
			sum_acb = ave_acb_gain_back;
		    else
			sum_acb = 1.0;
		    acb_excitation (PitchMemoryD + ACBMemSize, sum_acb,
				    delayi, PitchMemoryD, subframesize);
		    for (j = 0; j < ACBMemSize; j++)
			PitchMemoryD[j] = PitchMemoryD[j + subframesize];
		}

		// Do QPPP synthesis here
		//Do the QPPP to correct the ACB Memory
		DTFS prev, curr, TMP;
		float out[FSIZE], tmplsp[ORDER], lpc1[ORDER], lpc2[ORDER],
		    ph_offset = 0.0;

		//Get the lsps for this Q frame
		lsp2a (lpc1, OldOldlspD, ORDER);

		Interpol (tmplsp, OldOldlspD, OldlspD, 2, ORDER);
		lsp2a (lpc2, tmplsp, ORDER);

		//Extract the previous prototype and back up the memory
		prev.to_fs (PitchMemoryD + ACBMemSize - (int) pdelayD2,
			    (int) pdelayD2);
		TMP = prev;
		TMP.car2pol ();
		lastLgainD =
		    log10 (TMP.lag *
			   TMP.setEngyHarm (92.0, 1104.5, 0.0, 1104.5, 1.0));
		lastHgainD =
		    log10 (TMP.lag *
			   TMP.setEngyHarm (1104.5, 3300.0, 1104.5, 4000.0,
					    1.0));
		TMP.to_erb (lasterbD);

		curr.lag = (int) pdelayD;
		ppp_quarter_decoder (&curr, prev, lpc2);
		WIsyn (prev, &curr, lpc1, lpc2, &ph_offset, out, FSIZE);

		for (j = 0; j < ACBMemSize; j++)
		    PitchMemoryD[j] = out[FSIZE - ACBMemSize + j];
	    }
	}
    }
    // End: Fix delay contour only if no Phase Matching in NB mode; else PM fixes it anyway in NB mode

    /* Smooth interpolation if the difference between delays is too big */
    if (fabs (delay - pdelayD) > 15)
	pdelayD = delay;



    if (SPL_HCELP_WB == 1 || SPL_HCELP == 1) {

	ct = 0;
	for (i = 0; i < 3; i++) {
	    if (ppvq[data_packet.ACBG_IDX[i]] < 0.5499)
		ct += 1;

	    curr_ave_acb_gain +=
		ppvq[data_packet.ACBG_IDX[i]] * (i + 1) / 6.0;

	}
	if (curr_ave_acb_gain == 0)
	    curr_ave_acb_gain = ave_acb_gain;

	prev_ave_fcb_gain = ave_fcb_gain;
    }





    ave_acb_gain = ave_fcb_gain = 0.0;
    if (data_packet.WB_MODE_BIT == 1) {




	if (bit_rate == 4) {	// no huffman coded DA in half rate CELP



	    for (int j = 0; j < 3; j++)
		data_packet.DELAY_ADJ_IDX[j] = da_index[j];



	}			//end of bit_rate !=3
    }


    foutP = outFbuf;
    if (data_packet.WB_MODE_BIT == 1)
	PitchGain = 0.0;
    for (i = 0; i < NoOfSubFrames; i++) {

	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	Interpol (lspi, OldlspD, lsp, i, ORDER);

	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);
	if (data_packet.WB_MODE_BIT == 0) {
	    /* No to be used with Motorola Code
	       // Bandwidth expansion after frame erasure only if LPCflag is set
	     */
	}
	Interpol_delay (delayi, &pdelayD, &delay, i);

	if (bit_rate == 4 && data_packet.WB_MODE_BIT == 1) {	//MOTOROLA_CLOSED_LOOP_DELAY_ADJUST is done only for full rate CELP

	    {
		static float delayAdjust, deltaDelayAdjust, delayStd,
		    delayStdFuncOut;
		static float x0, x1;


		static float delayAdjustCodebook[5] =
		    { 0.0, 1.0, -1.0, 2.0, -2.0 };



		static int jbest;
		static float polarity;

		int k;

		if (i == 0) {
		    x0 = x1;
		    x1 = delay;

		    if (lastrateD < 3
			|| (lastrateD == 4 && prev_celp_mdct_dec == 1)) {
			x0 = x1;
		    }

		    delayStd = 0.707106781186547 * fabs (x0 - x1);
		    delayStdFuncOut =
			Min ((A_CLDA * delayStd + B_CLDA), SMAX_CLDA);
		    deltaDelayAdjust = delayStdFuncOut * (x0 + x1) / 2.0;
		}


		polarity = +1.0;





		jbest = data_packet.DELAY_ADJ_IDX[i];


		da_index[i] = jbest;	// used by dec_fpmp()


		delayAdjust =
		    polarity * deltaDelayAdjust * delayAdjustCodebook[jbest];

		delayi[0] = Min (Max ((delayi[0] + delayAdjust), DMIN), DMAX);
		delayi[1] = Min (Max ((delayi[1] + delayAdjust), DMIN), DMAX);
		delayi[2] = Min (Max ((delayi[2] + delayAdjust), DMIN), DMAX);


		// Save adjusted delay countour for frame synthesis loop
		delayi_adjusted[i][0] = delayi[0];
		delayi_adjusted[i][1] = delayi[1];
		delayi_adjusted[i][2] = delayi[2];

	    }

	}			//end of if (bit_rate==4) for disabling MOTOROLA_CLOSED_LOOP_DELAY_ADJUST
	if (SPL_HCELP_WB == 0 || SPL_HCELP == 0) {

	    if (bit_rate == 4) {
		fcbIndexVector[0] = data_packet.FCB_PULSE_IDX[i][0];
		fcbIndexVector[1] = data_packet.FCB_PULSE_IDX[i][1];
		fcbIndexVector[2] = data_packet.FCB_PULSE_IDX[i][2];
		fcbIndexVector[3] = data_packet.FCB_PULSE_IDX[i][3];
		if (data_packet.WB_MODE_BIT == 1) {




		}
		/* FCB gain index */
	    }
	    else {		//bit_rate==3 case
		//BitUnpack(SScratch, (unsigned short *) PackedWords, 11, PackWdsPtr);
		fcbIndexVector[0] = data_packet.FCB_PULSE_IDX[i][0];
	    }
	}
	/* Compute adaptive codebook contribution */
	sum_acb = ppvq[data_packet.ACBG_IDX[i]];

	if (SPL_HCELP_WB == 1 || SPL_HCELP == 1)	//additional processing for SPL_HCELP
	{
	    if (sum_acb < 0.5499)
		sum_acb = curr_ave_acb_gain;

	    if ((i != 0) && (ct == 3))
		sum_acb = 1.0;
	}


	if (ct != 3)		//if regular celp ct always=0 
	    ave_acb_gain += sum_acb * (i + 1) / 6;
	else			//SPL_HCELP
	    ave_acb_gain = curr_ave_acb_gain;



	if (args->vad_out) {	//overloading vad_out for acb write out
	    fprintf (stdout, "%f\n", sum_acb);
	}

	if (data_packet.WB_MODE_BIT == 1)
	    PitchGain += (sum_acb / 3.0);	//Populate the external variable PitchGain for the Wideband analysis
	if (lastrateD == 1)
	    ave_acb_gain = 0.0;
	g2 = MIN (0.5, 0.5 * sum_acb);
	if (data_packet.WB_MODE_BIT == 1) {


	    acb_excitation (PitchMemoryD + ACBMemSize, sum_acb, delayi,
			    PitchMemoryD, subframesize);


	}
	else {			//narrowband case
	    acb_excitation (PitchMemoryD + ACBMemSize, sum_acb, delayi,
			    PitchMemoryD, subframesize);
	}


	/* Compute fixed codebook contribution */


	if (SPL_HCELP_WB == 1 || SPL_HCELP == 1) {
	    if (curr_ave_acb_gain < 0.4) {
		for (j = 0; j < subframesize; j++)
		    PitchMemoryD[ACBMemSize + j] *= FadeScale;

	    }
	    /* Compute fixed codebook contribution */
	    ave_fcb_gain = 0.0;
	}
	else
	    ave_fcb_gain += gnvq[data_packet.FCBG_IDX[i]] / NoOfSubFrames;


	float acbg_save = sum_acb;


	if (sum_acb > 0.9)
	    sum_acb = 0.9;
	if (sum_acb < 0.2)
	    sum_acb = 0.2;

	/* get intrpolated delay for this subframe */
	n = (short) ((delayi[1] + delayi[0]) / 2.0 + 0.5);
	if (n > subframesize)
	    n = 200;

	/* Compute fixed codebook contribution */

	if (data_packet.WB_MODE_BIT == 1) {
	    if (SPL_HCELP_WB == 0) {


		if (bit_rate == 4) {
		    dec_fpmp (fcbIndexVector, Scratch,
			      (int) data_packet.WB_MODE_BIT);
		}
		else {		// bit_rate==3 case
		    dec3_10jcelp (n, sum_acb, subframesize, fcbIndexVector,
				  Scratch);
		}



		if (bit_rate == 4)
		    pit_shrp (Scratch, (int) (n), sum_acb, subframesize);


	    }			//SPL_HCELP_WB==0
	}
	else {			//narrowband case
	    if (SPL_HCELP == 0) {

		/* Compute fixed codebook contribution */
		if (bit_rate == 4) {

		    dec_fpmp (fcbIndexVector, Scratch,
			      (int) data_packet.WB_MODE_BIT);

		}
		else {		// bit_rate==3 case
		    dec3_10jcelp (n, sum_acb, subframesize, fcbIndexVector,
				  Scratch);
		}

		if (bit_rate == 4)
		    pit_shrp (Scratch, (int) (n), sum_acb, subframesize);
		sum1 = gnvq[data_packet.FCBG_IDX[i]];
	    }
	    else		//SPL_HCELP => zero out the fcb contribution
	    {
		for (j = 0; j < subframesize; j++)
		    Scratch[j] = 0.0;
		sum1 = 0.0;



	    }
	}

	if (data_packet.WB_MODE_BIT == 1) {



	    if (bit_rate == 4) {	//RESIDUAL_ENERGY_GAIN_QUANTIZATION is turned off for half rate CELP.
		Erq_ln =
		    (MIN_LOG_ENERGY_E +
		     (float) idxe * (MAX_LOG_ENERGY_E -
				     MIN_LOG_ENERGY_E) / NUM_Q_LEVELS_E);
		Erq = exp (Erq_ln);
		ckck = 1.0e-10;
		for (j = 0; j < subframesize; j++) {
		    ckck += Scratch[j] * Scratch[j];
		}
		sum1 =
		    GAIN_RE_F (ppvq[data_packet.ACBG_IDX[i]],
			       data_packet.FCBG_IDX[i]);
	    }
	    else		//if bit_rate is not equal to 4 
		sum1 = gnvq[data_packet.FCBG_IDX[i]];



	    if (SPL_HCELP_WB == 1) {
		for (j = 0; j < subframesize; j++)
		    Scratch[j] = 0.0;
		sum1 = 0.0;

	    }

	}

	if (data_packet.WB_MODE_BIT == 1) {


	    for (j = 0; j < subframesize; j++)
		Scratch[j] *= sum1;
	    POLEZERO_FILTER fcb_filt = { 4, 4, 4, 0, fcb_filt_mem_dec };

	    polezero_filter (Scratch, tmpbuf2, subframesize,
			     fcb_filt_num_coef, fcb_filt_den_coef, fcb_filt);


	    /* Add to total excitation */
	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[j + ACBMemSize] += tmpbuf2[j];



	}



	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);


	if (args->mform_res_out)
	    write_samples (1, residualm, subframesize);



	if (data_packet.WB_MODE_BIT == 1) {

	    static float ppf_state[ACBMemSize + SubFrameSize];
	    static long decode_fcnt_last;
	    static float gppf_sm = 0.0;

	    /* reset the ppf_state at the first 
	     ** good frame after erasure
	     */
	    if (i == 0 && fer_counter == 2) {
		for (j = 0; j < ACBMemSize + SubFrameSize; j++) {
		    ppf_state[j] = PitchMemoryD[j];
		}
	    }

	    if (i == 0
		&& ((decode_fcnt - 1 != decode_fcnt_last)
		    && (fer_counter != 2))) {
		for (j = 0; j < ACBMemSize + SubFrameSize; j++) {


		    ppf_state[j] = 0.0;

		}
	    }
	    decode_fcnt_last = decode_fcnt;


	    float gppf = ppf_coef * acbg_save;

	    pitchPreFilter (PitchMemoryD + ACBMemSize, ppf_state, delayi,
			    subframesize, gppf, gppf_sm, filt_exct);


	    for (j = 0; j < subframesize; j++)
		PM_ltffilt[i * (SubFrameSize - 1) + j] = filt_exct[j];


	}			//end of WB_MODE_BIT==1

	if (SPL_HCELP_WB == 1 && data_packet.WB_MODE_BIT == 1) {

	    printf (" Special HCELP in frame %d\n", decode_fcnt);
	    if (ave_acb_gain < 0.4) {
		FadeScale -= 0.05;
		if (FadeScale < 0.0)
		    FadeScale = 0.0;
		/* Add some gaussian noise */

		for (j = 0; j < subframesize; j++)
		    PitchMemoryD[ACBMemSize + j] +=
			0.1 * prev_ave_fcb_gain * ran_g (&celpSPL_HCELPSeed);
	    }
	}


	if (data_packet.WB_MODE_BIT == 0) {	//narrowband specific

	    if (do_phase_matching)	//check for Phase Matching: main Phase Matching processing
	    {
		//calculation of phase here
		if (i == 0) {
		    double prev_delay, prev_delay2;
		    double prev_delay_end, prev_delay2_end;
		    double encoder_phase, decoder_phase;

		    //if (idxppg != 0) //get delay of previous frame from current frame
		    {
			prev_delay_end = delay - (float) (store_ddelay - 16);
			prev_delay2_end = (prev_delay_end + pdelayD) / 2;

			if ((phase_offset == 0 && run_length == 1)
			    || (phase_offset == 1 && run_length == 2))
			    prev_delay = (prev_delay_end + pdelayD) / 2;
			else
			    prev_delay =
				(prev_delay_end + prev_delay2_end) / 2;

			prev_delay2 = (prev_delay2_end + pdelayD) / 2;
		    }

		    //Different cases using Phase Matching
		    //Get phase of encoder and current phase of decoder
		    //The difference between these is the amount of Phase Matching to be done

		    decoder_phase = encoder_phase = 0;

		    if (phase_offset == 0) {
			if (run_length == 1) {
			    encoder_phase =
				fmod (160, prev_delay) / prev_delay;
			    decoder_phase = fmod (160, pdelayD) / pdelayD;
			}
			else if (run_length == 2) {
			    encoder_phase =
				fmod (fmod (160, prev_delay) +
				      fmod (160, prev_delay2),
				      prev_delay) / prev_delay;
			    decoder_phase =
				fmod (fmod (160, pdelayD) * 2,
				      pdelayD) / pdelayD;
			}
		    }
		    else if (phase_offset == 1) {
			if (run_length == 1) {
			    encoder_phase = 0;
			    decoder_phase = fmod (160, pdelayD) / pdelayD;
			}
			else if (run_length == 2) {
			    encoder_phase =
				fmod (160, prev_delay) / prev_delay;
			    decoder_phase =
				fmod (fmod (160, pdelayD) * 2,
				      pdelayD) / pdelayD;
			}
		    }
		    else if (phase_offset == 2) {
			encoder_phase = 0;
			decoder_phase =
			    fmod (fmod (160, pdelayD) * 2, pdelayD) / pdelayD;
		    }
		    else if (phase_offset == -1) {
			encoder_phase =
			    fmod (fmod (160, prev_delay) +
				  fmod (160, prev_delay2),
				  prev_delay) / prev_delay;
			decoder_phase = fmod (160, pdelayD) / pdelayD;
		    }

		    //End Different cases using Phase Matching

		    if (decoder_phase >= encoder_phase) {
			pm_var1 =
			    (int) ((decoder_phase -
				    encoder_phase) * prev_delay);
		    }
		    else
			pm_var1 =
			    (int) (prev_delay -
				   prev_delay * (encoder_phase -
						 decoder_phase));

		    num_residual = 160 - pm_var1;
		    if (pm_var1 >= 100) {
			pm_var1 = 0;
		    }

		    //printf ("\n delays %f %f %f %f %d phases %f %f", prev_delay_end, pdelayD, delay, prev_delay, pm_var1, encoder_phase, decoder_phase); //fflush(stdout);

		    //If Phase Matching delta > subframesize, PM extends to two subframes
		    if (pm_var1 > 53) {
			pm_var2 = pm_var1 - 53;
			pm_var1 = 53;
		    }
		    else
			pm_var2 = 0;
		}
		else {
		    //If Phase Matching delta > subframesize, PM extends to two subframes

		    pm_var1 = pm_var2;
		    pm_var2 = 0;
		}

		for (j = 0; j < subframesize - pm_var1; j++)
		    PitchMemoryD[j + ACBMemSize] +=
			sum1 * Scratch[j + pm_var1];
	    }
	    else {
		pm_var1 = 0;

		for (j = 0; j < subframesize; j++)
		    PitchMemoryD[j + ACBMemSize] += sum1 * Scratch[j];
	    }


	    //Warping: Slight change in call to support smaller/larger number of samples
	    if (args->unquantized_full_rate)
		for (j = 0; j < subframesize; j++)
		    PitchMemoryD[ACBMemSize + j] =
			copy[i * (SubFrameSize - 1) + j];
	    //Changed to support Phase Matching
	    for (j = 0; j < ACBMemSize; j++)
		PitchMemoryD[j] = PitchMemoryD[j + subframesize - pm_var1];
	    //End Changed to support Phase Matching

	    if (args->qform_res_out)
		write_samples (1, PitchMemoryD + ACBMemSize, subframesize);


	    if (SPL_HCELP == 1) {
		if (ave_acb_gain < 0.4) {
		    FadeScale -= 0.05;
		    if (FadeScale < 0.0)
			FadeScale = 0.0;
		    /* Add some gaussian noise */

		    for (j = 0; j < subframesize; j++)
			PitchMemoryD[ACBMemSize + j] +=
			    0.1 * prev_ave_fcb_gain *
			    ran_g (&celpSPL_HCELPSeed);
		}
	    }
	}
	if (data_packet.WB_MODE_BIT == 1) {
	    /* update PitchMemoryD */
	    for (j = 0; j < ACBMemSize; j++)
		PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	    /* Collecting the entire frame of excitation */
	    for (j = 0; j < subframesize; j++)
		PitchMemoryD_Frame[i * (SubFrameSize - 1) + j] =
		    PitchMemoryD[ACBMemSize + j];
	}
	else {
	    /* Collecting the entire frame of excitation */
	    //Changed to support Phase Matching
	    for (j = 0; j < subframesize - pm_var1; j++)
		PitchMemoryD_Frame[temp_samples_count + j] =
		    PitchMemoryD[ACBMemSize + j];

	    temp_samples_count += subframesize - pm_var1;	//Added to support Phase Matching

	}
    }
    if (data_packet.WB_MODE_BIT == 1) {

	for (j = 0; j < FSIZE; j++)
	    PitchMemoryD_Frame[j] = PM_ltffilt[j];

    }
    //Post WI synthesis for the purpose of smoothing
    if (LAST_BAD_CW.lag != 0 && fabs (delay - pdelayD) < 8.0
	&& local_lpc_flag == FALSE && ppvq[data_packet.ACBG_IDX[0]] > 0
	&& ppvq[data_packet.ACBG_IDX[1]] > 0
	&& ppvq[data_packet.ACBG_IDX[2]] > 0) {

	float enratio, out[FSIZE], tmplsp[ORDER], lpc1[ORDER], lpc2[ORDER],
	    ph_offset = 0.0;
	DTFS CURR_GOOD_CW;

	CURR_GOOD_CW.to_fs (PitchMemoryD + ACBMemSize - (int) delay,
			    (int) delay);

	enratio = CURR_GOOD_CW.getEngy () / LAST_BAD_CW.getEngy ();

	//Ensure there is no drastic energy change between successive CWs before interp
	if (enratio > 0.025 && enratio < 14.5) {
	    Interpol (tmplsp, OldOldlspD, OldlspD, 2, ORDER);
	    lsp2a (lpc1, tmplsp, ORDER);

	    Interpol (tmplsp, OldlspD, lsp, 2, ORDER);
	    lsp2a (lpc2, tmplsp, ORDER);
	    if (data_packet.WB_MODE_BIT == 1) {
		WIsyn (LAST_BAD_CW, &CURR_GOOD_CW, lpc1, lpc2, &ph_offset,
		       out, FSIZE);

		for (j = 0; j < ACBMemSize; j++)
		    PitchMemoryD[j] = out[FSIZE - ACBMemSize + j];
		for (j = 0; j < FSIZE; j++)
		    PitchMemoryD_Frame[j] = out[j];
	    }
	    else {		//narrowband case
		//printf ("\n post_WI_SYN");

		if (do_phase_matching) {

		}
		else {
		    WIsyn (LAST_BAD_CW, &CURR_GOOD_CW, lpc1, lpc2, &ph_offset,
			   out, FSIZE);
		    for (j = 0; j < ACBMemSize; j++)
			PitchMemoryD[j] = out[FSIZE - ACBMemSize + j];
		    for (j = 0; j < FSIZE; j++)
			PitchMemoryD_Frame[j] = out[j];
		}
	    }
	}
    }

    if (data_packet.WB_MODE_BIT == 1) {

//copy over to UB
	for (j = 0; j < FSIZE; j++)
	    LB_Excitation[j] = PitchMemoryD_Frame[j];

	if (args->unquantized_full_rate)
	    for (j = 0; j < FSIZE; j++)
		PitchMemoryD_Frame[j] = copy[j];


	{
	    static float env1, env2;
	    float tmp_env, tmp_gain, x2;
	    int k;
	    float fb1 = 0.97;
	    float ff1 = 0.14;
	    float fb2 = 0.98;
	    float ff2 = 0.1;

	    if (bit_rate == 4) {
		if (lastrateD != 4 || (lastrateD == 4 && prev_celp_mdct_dec == 1))	// reset state
		{
		    env1 = 1.0;
		    env2 = 1.0;
		}

		for (k = 0; k < FSIZE; k++) {
		    x2 = LB_Excitation[k] * LB_Excitation[k];

		    tmp_env = ff1 * x2 + (1 - ff1) * env1;
		    env1 *= fb1;
		    if (tmp_env > env1)
			env1 = tmp_env;

		    tmp_env = ff2 * x2 + (1 - ff2) * env2;
		    env2 *= fb2;
		    if (tmp_env > env2)
			env2 = tmp_env;

		    tmp_gain = env1 / env2;
		    tmp_gain = tmp_gain > 1.1 ? 1.1 : tmp_gain;
		    tmp_gain = tmp_gain < 0.5 ? 0.5 : tmp_gain;

		    LB_Excitation[k] *= tmp_gain;

		}
	    }
	}




	if (lastrateD < 3 || (lastrateD == 4 && prev_celp_mdct_dec == 1))
	    for (j = 0; j < 4; j++)
		mem_BB[j] = 0.0;	/* reset memory */



	//Warping-expanding or compressing

	num_samples = 160;


	LB_delay = delay;

	if (LB_delay > 0 && (time_warp_fraction > 1 || time_warp_fraction < 1))	//Expansion or Compression
	{
	    merge_start = 0;

	    if ((delay) - floor (delay) > 0.5)
		merge_end = (int) delay + 1;
	    else
		merge_end = (int) delay;

	    double tmp_check = merge_end % 8;

	    if (merge_end - merge_start > num_residual / 2)
		merge_stop = num_residual - (merge_end - merge_start);
	    else
		merge_stop = merge_end - merge_start;


	    if (num_residual <= delay)
		merge_stop = 0;

	    num_samples = num_residual;

	    for (b = 0; b < num_residual; b++)
		temp_pitch[b] = PitchMemoryD_Frame[b];

	    sample_counter = 0;

	    if (merge_stop != 0) {
		if (time_warp_fraction > 1)	//Expansion
		{
		    for (b = merge_start; b < merge_end; b++)
			PitchMemoryD_Frame[b] = temp_pitch[b];

		    sample_counter += merge_end;

		    for (b = merge_start; b < merge_stop; b++)
			PitchMemoryD_Frame[b + merge_end] =
			    temp_pitch[b +
				       (merge_end -
					merge_start)] * (1.0 * (merge_stop -
								merge_start -
								b) /
							 (merge_stop -
							  merge_start)) +
			    temp_pitch[b] * (1.0 * (b) /
					     (merge_stop - merge_start));

		    sample_counter += merge_stop - merge_start;

		    //Regular expansion as requested by de-jitter: produce one extra pitch period
		    {
			for (b = merge_start + merge_end + merge_stop;
			     b < num_residual + merge_end - merge_start; b++)
			    PitchMemoryD_Frame[b] =
				temp_pitch[b - (merge_end - merge_start)];

			num_samples =
			    num_residual + (merge_end - merge_start);
			//printf ("\n expand_num_samples = %d %d %f %d %d %f", num_residual, num_samples, delay, merge_end, merge_stop, time_warp_fraction);
		    }
		}
		//Regular compression as requested by de-jitter: produce one less pitch period
		//Compress only if compressed samples >= 40
		else if (time_warp_fraction < 1
			 && (num_residual - (merge_end - merge_start)) >=
			 20) {
		    for (b = merge_start; b < merge_stop; b++)
			PitchMemoryD_Frame[b] =
			    PitchMemoryD_Frame[b] * (1.0 *
						     (merge_stop -
						      merge_start -
						      b) / (merge_stop -
							    merge_start)) +
			    PitchMemoryD_Frame[b +
					       (merge_end -
						merge_start)] * (1.0 * (b) /
								 (merge_stop -
								  merge_start));

		    for (b = merge_stop;
			 b + merge_end - merge_start < num_residual; b++)
			PitchMemoryD_Frame[b] =
			    PitchMemoryD_Frame[b + merge_end - merge_start];

		    num_samples = num_residual - (merge_end - merge_start);
		}
	    }
	}
	//End: Warping-expanding or compressing
    }
    else {			//narrowband mode
	num_samples = temp_samples_count;

	//Warping-expanding or compressing

	if (num_samples <= 120) {
	    if (time_warp_fraction < 1)
		time_warp_fraction = 1;
	    else
		time_warp_fraction = 1.5;
	}

	if (time_warp_fraction > 1 || time_warp_fraction < 1)	//Expansion or Compression
	{
	    merge_start = 0;

	    if ((delay) - floor (delay) > 0.5)
		merge_end = (int) delay + 1;
	    else
		merge_end = (int) delay;

	    if (merge_end - merge_start > num_residual / 2)
		merge_stop = num_residual - (merge_end - merge_start);
	    else
		merge_stop = merge_end - merge_start;


	    if (num_residual <= delay)
		merge_stop = 0;

	    num_samples = num_residual;

	    for (b = 0; b < num_residual; b++)
		temp_pitch[b] = PitchMemoryD_Frame[b];

	    sample_counter = 0;

	    if (merge_stop != 0) {
		if (time_warp_fraction > 1)	//Expansion
		{
		    for (b = merge_start; b < merge_end; b++)
			PitchMemoryD_Frame[b] = temp_pitch[b];

		    sample_counter += merge_end;

		    for (b = merge_start; b < merge_stop; b++)
			PitchMemoryD_Frame[b + merge_end] =
			    temp_pitch[b +
				       (merge_end -
					merge_start)] * (1.0 * (merge_stop -
								merge_start -
								b) /
							 (merge_stop -
							  merge_start)) +
			    temp_pitch[b] * (1.0 * (b) /
					     (merge_stop - merge_start));

		    sample_counter += merge_stop - merge_start;

		    //If erased packet is being reconstructed, need to produce two extra pitch periods
		    // But only two produce two extra pitch periods if length will be less than 320
		    if (phase_offset == -1
			&& (num_residual + 2 * (merge_end - merge_start)) <=
			320) {
			if (merge_end - merge_stop > 0) {
			    for (b = sample_counter, c = 0;
				 b < merge_end + merge_end; b++, c++)
				PitchMemoryD_Frame[b] =
				    temp_pitch[merge_stop + c];

			    sample_counter += merge_end - merge_stop;
			}

			for (b = 0; b < merge_stop; b++)
			    PitchMemoryD_Frame[b + sample_counter] =
				temp_pitch[b +
					   (merge_end -
					    merge_start)] * (1.0 *
							     (merge_stop -
							      merge_start -
							      b) /
							     (merge_stop -
							      merge_start)) +
				temp_pitch[b] * (1.0 * (b) /
						 (merge_stop - merge_start));
			sample_counter += merge_stop;


			for (b = merge_start + merge_end + merge_stop;
			     b < num_residual + merge_end - merge_start; b++)
			    PitchMemoryD_Frame[b + merge_end] =
				temp_pitch[b - (merge_end - merge_start)];

			num_samples =
			    num_residual + 2 * (merge_end - merge_start);
			//printf ("\n 1expand_num_samples = %d %d %f %d %d %f", num_residual, num_samples, delay, merge_end, merge_stop, time_warp_fraction);
		    }
		    else	//Else, regular expansion as requested by de-jitter: produce one extra pitch period
		    {
			for (b = merge_start + merge_end + merge_stop;
			     b < num_residual + merge_end - merge_start; b++)
			    PitchMemoryD_Frame[b] =
				temp_pitch[b - (merge_end - merge_start)];

			num_samples =
			    num_residual + (merge_end - merge_start);
			//printf ("\n expand_num_samples = %d %d %f %d %d %f", num_residual, num_samples, delay, merge_end, merge_stop, time_warp_fraction);
		    }
		}
		//Regular compression as requested by de-jitter: produce one less pitch period
		//Compress only if compressed samples >= 40
		else if (time_warp_fraction < 1
			 && (num_residual - (merge_end - merge_start)) >=
			 20) {
		    for (b = merge_start; b < merge_stop; b++)
			PitchMemoryD_Frame[b] =
			    PitchMemoryD_Frame[b] * (1.0 *
						     (merge_stop -
						      merge_start -
						      b) / (merge_stop -
							    merge_start)) +
			    PitchMemoryD_Frame[b +
					       (merge_end -
						merge_start)] * (1.0 * (b) /
								 (merge_stop -
								  merge_start));

		    for (b = merge_stop;
			 b + merge_end - merge_start < num_residual; b++)
			PitchMemoryD_Frame[b] =
			    PitchMemoryD_Frame[b + merge_end - merge_start];

		    num_samples = num_residual - (merge_end - merge_start);
		}
	    }
	}
	//End Warping-expanding or compressing        

    }
    //More support for Time-Warping
    temp_samples_count = 0;
    *oBuf_len = num_samples;
    //End: More support for Time-Warping
    if (data_packet.WB_MODE_BIT == 1) {
	if (lastrateD != 4) {
	    for (j = 0; j < ORDER; j++)
		dec_FormantFilterMemory[j] = 0.0;
	    dec_preemphmem[0] = 0.0;
	}
    }


    //SECOND PART OF THE SYNTHESIS
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
	if (data_packet.WB_MODE_BIT == 1) {

	    if (fer_counter == 2) {	//If previous frame was erased, determine if the LSPS need to be interpolated.
		float GainRatio;

		GainRatio = det_interpol (lsp, OldlspD, ORDER, SubFrameSize);

		if (GainRatio >= 5.0) {
		    Interpol (lspi, OldlspD, lsp, FER_INT_FAC_INX, ORDER);
		}
		else {
		    Interpol (lspi, OldlspD, lsp, i, ORDER);
		}
	    }
	    else {
		Interpol (lspi, OldlspD, lsp, i, ORDER);
	    }



	}
	else
	    Interpol (lspi, OldlspD, lsp, i, ORDER);


	/* Convert lsp to PC */
	lsp2a (pci, lspi, ORDER);

	if (data_packet.WB_MODE_BIT == 1) {

	    if (bit_rate == 4) {
		delayi[0] = delayi_adjusted[i][0];
		delayi[1] = delayi_adjusted[i][1];
		delayi[2] = delayi_adjusted[i][2];
	    }
	    else
		Interpol_delay (delayi, &pdelayD, &delay, i);



	}
	else
	    Interpol_delay (delayi, &pdelayD, &delay, i);


	SynthesisFilter (DECspeech, PitchMemoryD_Frame + temp_samples_count,
			 pci, SynMemory, ORDER, subframesize);



	/* Postfilter */
	if (post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 (delayi[0] + delayi[1]) / 2.0, 0.4, 0.15,
			 subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);

	if (data_packet.WB_MODE_BIT == 1) {


	    if (i == 0 && prev_celp_mdct_dec == 1) {
		bass_boost (last_music_signal_sf,
			    (delayi[0] + delayi[1]) / 2.0, subframesize);
	    }

	    bass_boost (DECspeechPF, (delayi[0] + delayi[1]) / 2.0,
			subframesize);
	    last_delayBB = (delayi[0] + delayi[1]) / 2.0;



/* Check if bbg estimator should be before or after bass_boost() */

	    {
		float decoutenergy = 0;

		for (j = 0; j < subframesize; j++) {
		    decoutenergy += DECspeechPF[j] * DECspeechPF[j];
		}
		update_bbg_estimate2 (pci, decoutenergy);
	    }



	}


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
	temp_samples_count += subframesize;	//Time-Warping: counting number of samples synthesized
    }



    pdelayD = delay;

    if (data_packet.WB_MODE_BIT == 1)
	prev_celp_erasure = 0;

    return (0);
}

void
FGV_MEM::celp_erasure_decoder (float *outFbuf, short post_filter)
{
    register int i, j;
    register float *foutP;
    float delayi[3];
    short subframesize;

    float filt_exct[54], agc_fac;


// long Seed=0;
    // static long Seed;

    float sum_acb;

    float g2;			// For 2nd pitch filter

    delay = pdelayD;
    /* Convert lsp to PC */
    lsp2a (pci, OldlspD, ORDER);
    if (!prev_frame_error)
	celpErasureSeed = (long) (OldlspD[ORDER - 1] * 65536.0);
    if (data_packet.WB_MODE_BIT == 1) {

	if (lastrateD != 4 || (lastrateD == 4 && prev_celp_mdct_dec == 1))
	    for (j = 0; j < 4; j++)
		mem_BB[j] = 0.0;	/* reset memory */

	PitchGain = 0.0;

    }
    foutP = outFbuf;
    for (i = 0; i < NoOfSubFrames; i++) {

	if (i < 2)
	    subframesize = SubFrameSize - 1;
	else
	    subframesize = SubFrameSize;

	Interpol (lspi, OldlspD, OldlspD, i, ORDER);

	Interpol_delay (delayi, &pdelayD, &delay, i);

	/* Compute adaptive codebook contribution */
	g2 = MIN (0.5, 0.5 * ave_acb_gain);
	sum_acb = 1.0;
	if (ave_acb_gain > 0.8)
	    sum_acb = 1.0;
	else
	    sum_acb = ave_acb_gain;
	if (i == 0)
	    sum_acb = ave_acb_gain;

	if (args->vad_out) {	//overloading vad_out for acb write out
	    fprintf (stdout, "%f\n", sum_acb);
	}

	if (data_packet.WB_MODE_BIT == 1) {

	    if (FrameRateMemory[2] == 14 && FrameRateMemory[1] == 4
		&& FrameRateMemory[0] != 4) {
		if (sum_acb < 0.9) {
		    if (i == 0)
			sum_acb = 0.95;
		    else if (i == 1)
			sum_acb = 0.9;
		    else
			sum_acb = 0.85;
		}

	    }






	    PitchGain += (sum_acb / 3.0);	//Populate the external variable PitchGain for the Wideband analysis
	}
	acb_excitation (PitchMemoryD + ACBMemSize, sum_acb, delayi,
			PitchMemoryD, subframesize);

	/* Compute fixed codebook contribution */
	for (j = 0; j < subframesize; j++)
	    PitchMemoryD[ACBMemSize + j] *= FadeScale;
	if (data_packet.WB_MODE_BIT == 1) {




	    agc_fac = 0.4;	// or acb_gain
	    for (j = 0; j < subframesize; j++)
		filt_exct[j] = PitchMemoryD[j + ACBMemSize];
	    ltf_fir (PitchMemoryD, (delayi[0] + delayi[1]) / 2.0, agc_fac,
		     subframesize, filt_exct);
	    agc_new (PitchMemoryD + ACBMemSize, filt_exct, subframesize);

	}
	/* update PitchMemoryD */
	for (j = 0; j < ACBMemSize; j++)
	    PitchMemoryD[j] = PitchMemoryD[j + subframesize];
	if (data_packet.WB_MODE_BIT == 1) {

	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[j + ACBMemSize] = filt_exct[j];

	}



	FadeScale -= 0.05;
	if (FadeScale < 0.0)
	    FadeScale = 0.0;
	/* Add some gaussian noise */
	if (ave_acb_gain < 0.4) {
	    if (data_packet.WB_MODE_BIT == 0)
		/* attenuate the ave_fcb_gain in case that there are multiple erasures in a row. */
		if (total_fer_counter > 3)
		    ave_fcb_gain *= 0.75;

	    for (j = 0; j < subframesize; j++)
		PitchMemoryD[ACBMemSize + j] +=
		    0.1 * ave_fcb_gain * ran_g (&celpErasureSeed);
	}

	if (args->unquantized_full_rate) {	/*Do Nothing */
	}

	//    for (j=0; j<ACBMemSize; j++) PitchMemoryD[j]=PitchMemoryD[j+subframesize];
	if (args->qform_res_out)
	    write_samples (1, PitchMemoryD + ACBMemSize, subframesize);
	if (data_packet.WB_MODE_BIT == 1) {
	    for (j = 0; j < subframesize; j++)
		LB_Excitation[i * (SubFrameSize - 1) + j] =
		    PitchMemoryD[ACBMemSize + j];



	    // update FCB- LPF memory
	    for (j = 0; j < 4; j++)
		fcb_filt_mem_dec[j] = 0.0;


	    {
		static float env1, env2;
		float tmp_env, tmp_gain, x2;
		float *buf_ptr;
		int k;
		float fb1 = 0.97;
		float ff1 = 0.14;
		float fb2 = 0.98;
		float ff2 = 0.1;

		if (bit_rate == 4) {
		    if (lastrateD != 4 || (lastrateD == 4 && prev_celp_mdct_dec == 1))	// reset state , 
		    {
			env1 = 1.0;
			env2 = 1.0;
		    }
		    buf_ptr = &(LB_Excitation[i * (SubFrameSize - 1)]);

		    for (k = 0; k < subframesize; k++) {
			x2 = buf_ptr[k] * buf_ptr[k];

			tmp_env = ff1 * x2 + (1 - ff1) * env1;
			env1 *= fb1;
			if (tmp_env > env1)
			    env1 = tmp_env;

			tmp_env = ff2 * x2 + (1 - ff2) * env2;
			env2 *= fb2;
			if (tmp_env > env2)
			    env2 = tmp_env;

			tmp_gain = env1 / env2;
			tmp_gain = tmp_gain > 1.1 ? 1.1 : tmp_gain;
			tmp_gain = tmp_gain < 0.5 ? 0.5 : tmp_gain;

			buf_ptr[k] *= tmp_gain;
		    }
		}
	    }


	}

	/* Synthesis of decoder output signal and postfilter output signal */
	SynthesisFilter (DECspeech, PitchMemoryD + ACBMemSize, pci, SynMemory,
			 ORDER, subframesize);

	/* Postfilter */
	if (post_filter) {
	    Post_Filter (DECspeech, lspi, pci, DECspeechPF,
			 (delayi[0] + delayi[1]) / 2.0, ave_acb_gain, 0.15,
			 subframesize);
	}
	else
	    V_copy (DECspeech, DECspeechPF, subframesize);

	if (data_packet.WB_MODE_BIT == 1) {


	    if (i == 0 && prev_celp_mdct_dec == 1) {
		bass_boost (last_music_signal_sf,
			    (delayi[0] + delayi[1]) / 2.0, subframesize);
	    }

	    bass_boost (DECspeechPF, (delayi[0] + delayi[1]) / 2.0,
			subframesize);
	    last_delayBB = (delayi[0] + delayi[1]) / 2.0;

	}

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

    pdelayD = delay;
    if (data_packet.WB_MODE_BIT == 1)
	prev_celp_erasure = 1;
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     w2res                                                   */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

void
FGV_MEM::Weight2Res (float *output,
		     float *input,
		     float *coef_uq,
		     float *coef,
		     float gamma1, float gamma2, short order, short length)
{
    register short i, j;
    float SUM;
    float memory[ORDER];
    float wcoef[ORDER];

    /* A(Z) */
    for (i = 1; i < ORDER; i++)
	memory[i] = 0;
    memory[0] = input[0];
    output[0] = input[0];
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = input[i]; j > 0; j--) {
	    SUM += coef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM += coef[0] * memory[0];
	*memory = input[i];
	output[i] = SUM;
    }

    /* Cascade with of 1/A(Z/gamma1) */
    weight (wcoef, coef_uq, gamma1, order);

    for (i = 1; i < ORDER; i++)
	memory[i] = 0;
    memory[0] = output[0];
    for (i = 1; i < length; i++) {
	for (j = order - 1, SUM = output[i]; j > 0; j--) {
	    SUM -= wcoef[j] * memory[j];
	    memory[j] = memory[j - 1];
	}
	SUM -= wcoef[0] * memory[0];
	*memory = SUM;
	output[i] = SUM;
    }
    if (data_packet.WB_MODE_BIT == 1) {

	/* Cascade with of A(Z/gamma2) */
	weight (wcoef, coef_uq, gamma2, order);

	for (i = 1; i < ORDER; i++)
	    memory[i] = 0;
	memory[0] = output[0];
	for (i = 1; i < length; i++) {
	    for (j = order - 1, SUM = output[i]; j > 0; j--) {
		SUM += wcoef[j] * memory[j];
		memory[j] = memory[j - 1];
	    }
	    SUM += wcoef[0] * memory[0];
	    *memory = output[i];
	    output[i] = SUM;
	}

    }
    else {
	/* Cascade with of A(Z/gamma2) */
	weight (wcoef, coef_uq, gamma2, order);

	for (i = 1; i < ORDER; i++)
	    memory[i] = 0;
	memory[0] = output[0];
	for (i = 1; i < length; i++) {
	    for (j = order - 1, SUM = output[i]; j > 0; j--) {
		SUM += wcoef[j] * memory[j];
		memory[j] = memory[j - 1];
	    }
	    SUM += wcoef[0] * memory[0];
	    *memory = output[i];
	    output[i] = SUM;
	}

    }
}
