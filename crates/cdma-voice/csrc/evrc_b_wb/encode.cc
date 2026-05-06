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
    "$Id: encode.cc,v 1.70.2.6 2007/06/24 06:43:07 apadmana Exp $";

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
/*  Module:     encode                                               	*/
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     08/18/96  Modified LSP interpolation in preencode to use         */
/*               non-quantized Endpoints.                               */
/*----------------------------------------------------------------------*/
/* EVRC Encoder */
/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/
#include  <string.h>
#include  <unistd.h>
#include  "globs.h"
#include  "macro.h"
#include  "proto.h"
#include  "rom.h"
#include  "acelp.h"		/* for ACELP fixed codebook */
#include  "struct.h"






/*======================================================================*/
/*         ..Reset RCELP encoder parameters.                            */
/*----------------------------------------------------------------------*/

void
FGV_MEM::InitEncoder ()
{
    /*....(local) variables.... */
    int j;
    float ftmp;
    float sum1, sum2;

  /****************************************************/
    /*         Algorithm (one time) initializations     */
  /****************************************************/

    //NOISE SUPRESSION related inits
    /* ensure time alignment between low band and high band */
    if (args->noise_suppression) {

	hb_ana_delay = HB_ANA_DELAY;

    }
    else
	hb_ana_delay = HB_ANA_DELAY;

    /* More initialization is done in the routines using FirstTime */
    /* static variable to indicate algorithmic reset.              */

    bit_rate = 4;

    encode_fcnt = 0;

    /* ACB gains */
    for (j = 0; j < ACBGainSize - 1; j++)
	ppvq_mid[j] = (ppvq[j] + ppvq[j + 1]) / 2.0;

    for (j = 0; j < ORDER; j++)
	SynMemoryM[j] = 0.0;

    ftmp = 0.48 / (float) ORDER;
    OldOldlspE[0] = OldlspE[0] = Oldlsp_nq[0] = ftmp;
    for (j = 1; j < ORDER; j++) {
	OldOldlspE[j] = OldOldlspE[j - 1] + ftmp;
	OldlspE[j] = OldlspE[j - 1] + ftmp;
	Oldlsp_nq[j] = Oldlsp_nq[j - 1] + ftmp;
    }

    /*    HPspeech = residual; */

    for (j = 0; j < GUARD; j++)
	ConstHPspeech[j] = 0.0;

    for (j = 0; j < ORDER; j++)
	WFmemIIR[j] = WFmemFIR[j] = 0.0;

    for (j = 0; j < ACBMemSize; j++)
	Excitation[j] = 0.0;
    accshift = 0.0;
    shiftSTATE = 0;
    dpm = 0;
    pdelay = 40.0;

    LPCgain = 1.0;
    if (data_packet.WB_MODE_BIT == 1) {
	cod3_10_offset[0] = 0;
	cod3_10_offset[1] = 2;
	cod3_10_offset[2] = 4;
	for (j = 0; j < ORDER; j++)
	    pci[j] = 0.0;
    }

}

void
FGV_MEM::InitEncoder_1 ()
{
    /*....(local) variables.... */
    int j;
    float ftmp;
    float sum1, sum2;

  /****************************************************/
    /*         Algorithm (one time) initializations     */
  /****************************************************/

    /* More initialization is done in the routines using FirstTime */
    /* static variable to indicate algorithmic reset.              */

    //bit_rate=4;

    /* ACB gains */
    for (j = 0; j < ACBGainSize - 1; j++)
	ppvq_mid[j] = (ppvq[j] + ppvq[j + 1]) / 2.0;

    for (j = 0; j < ORDER; j++)
	SynMemoryM[j] = 0.0;

    for (j = 0; j < ORDER; j++)
	WFmemIIR[j] = WFmemFIR[j] = 0.0;

    for (j = 0; j < ACBMemSize; j++)
	Excitation[j] = 0.0;
    accshift = 0.0;
    shiftSTATE = 0;
    dpm = 0;
    //pdelay = 40.0;

    LPCgain = 1.0;
    cod3_10_offset[0] = 0;
    cod3_10_offset[1] = 2;
    cod3_10_offset[2] = 4;

}

/*======================================================================*/
/*         ..Get beta, et al.                                           */
/*----------------------------------------------------------------------*/
#define SIGN(a) ((a)>=0?(1):(-1))
#define SQR(a) ((a)*(a))
double sens[ORDER], P_s[ORDER / 2], Q_s[ORDER / 2];
float lsi[ORDER];



void
FGV_MEM::pre_encode (float *inFbuf, float *Rs)
{
    /*....(local) variables.... */
    register int i, j;		/* counters */
    float sum1;			/* tmp accumulator */

    float lsc[ORDER];

//  static float  last_delay, last_beta;
    int k, k0;
    float sum0;
    float factor;

    /*....execute.... */

    /* Re-initialize PackWdsPtr */
    PackWdsPtr[0] = 16;
    PackWdsPtr[1] = 0;
    PktPtr[0] = 16;
    PktPtr[1] = 0;
    for (i = 0; i < PACKWDSNUM; i++) {
	PackedWords[i] = 0;
	TxPkt[i] = 0;
    }

    /* Process Data */
    for (j = 0; j < FrameSize + LOOKAHEAD_LEN; j++)
	HPspeech[j + GUARD] = inFbuf[j];

    /* Make sure HPspeech - HPspeech+2*GUARD have the right memory */
    for (j = 0; j < GUARD; j++)
	HPspeech[j] = ConstHPspeech[j];

    /* Update ConstHPspeech for next frame */
    for (j = 0; j < GUARD; j++)
	ConstHPspeech[j] = HPspeech[j + FrameSize];

    /*Calculate the window for the LPC analysis. This is done only once */

    if (autocorrelationFirstTime) {
	autocorrelationFirstTime = 0;
	factor = AUTO_PI2 / (float) LPC_ANALYSIS_WINDOW_LENGTH;


#define GAMMA   8.4		/* length 320, center at 240 */
#define LAMBDA  0.0

	double maxw;

	/* Get base window */
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    HammingWindow[k] =
		(0.5 + LAMBDA / 2) - (0.5 - LAMBDA / 2) * cos (factor * k);

	/* Exponentially warp the base window */
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    HammingWindow[k] =
		(HammingWindow[k] -
		 LAMBDA) * exp (GAMMA * k / LPC_ANALYSIS_WINDOW_LENGTH);

	/* Find max and normalize */
	maxw = HammingWindow[0];
	for (k = 1; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    if (HammingWindow[k] > maxw)
		maxw = HammingWindow[k];
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++) {
	    HammingWindow[k] =
		(1.0 - LAMBDA) * HammingWindow[k] / maxw + LAMBDA;
	}
	for (k = 0; k < ORDER + 7; k++)
	    w[k] =
		exp (-0.5 * pow (2 * 3.1416 * F0 * (float) k / 8000.0, 2.0));
	w[0] = 1.00003;

    }


    /* Calculate prediction coefficients */
    /* reflection coef. not needed-returned in Scratch */
    lpcanalys (pci, Scratch, HPspeech, ORDER, LPC_ANALYSIS_WINDOW_LENGTH, Rs,
	       HammingWindow);


    /* Calculate P_s and Q_s for weight computation */
    for (i = 0; i < ORDER / 2; i++)
	P_s[i] = pci[i] + pci[ORDER - 1 - i];
    for (i = 0; i < ORDER / 2; i++)
	Q_s[i] = pci[i] - pci[ORDER - 1 - i];

    /* Calculate impulse response of 1/A(z) Note: only one IIR filter */
    ImpulseRzp (H, pci, pci, 1.0, 1.0, ORDER, Hlength);

    /* Get energy of H */
    for (j = 0, sum1 = 0; j < Hlength; j++)
	sum1 += H[j] * H[j];

    /* determine spectral transistion degree (set dtx_flag if large) --
       for frame erasures */
    if (sum1 / LPCgain > 10.0)
	LPCflag = 1;
    else
	LPCflag = 0;

    LPCgain = sum1;

    /* Bandwidth expansion */
    weight (pci, pci, _Gamma_4, ORDER);


    /* Convert prediction coefficients to lsp */
    a2lsp (lsp_nq, pci, ORDER);


    if (data_packet.WB_MODE_BIT == 1) {
    }


    /* compute lsi for quantization */
    for (i = 0; i < ORDER; i++)
	lsc[i] = cos (pi2 * lsp_nq[i]);
    for (i = 0; i < ORDER; i++)
	lsi[i] =
	    (lsc[i] <
	     0.0) + (SIGN (lsc[i])) * 0.5 * sqrt (1.0 - fabs (lsc[i]));
    /* compute sensitivity weights */
    compute_sens (lsc, Rs, P_s, Q_s, sens);

    /* Get residual signal */
    for (j = 0; j < ORDER; j++)
	Scratch[j] = 0;		/* Scratch is used as filter memory */

    lsp2a (pci_nq, Oldlsp_nq, ORDER);

    GetResidual (residual, HPspeech, pci_nq, Scratch, ORDER, GUARD);

    for (i = 0; i < NoOfSubFrames; i++) {

	Interpol (lspi_nq, Oldlsp_nq, lsp_nq, i, ORDER);
	lsp2a (pci_nq, lspi_nq, ORDER);
	if (i < 2)
	    GetResidual (residual + i * (SubFrameSize - 1) + GUARD,
			 HPspeech + i * (SubFrameSize - 1) + GUARD, pci_nq,
			 Scratch, ORDER, SubFrameSize - 1);
	else
	    GetResidual (residual + i * (SubFrameSize - 1) + GUARD,
			 HPspeech + i * (SubFrameSize - 1) + GUARD, pci_nq,
			 Scratch, ORDER, SubFrameSize);
    }

    GetResidual (residual + FrameSize + GUARD, HPspeech + FrameSize + GUARD,
		 pci, Scratch, ORDER, LOOKAHEAD_LEN);
    if (data_packet.WB_MODE_BIT == 1) {
	/* Calculate pitch period at the end of the frame, use n.q. lpc coef. */
	fndppf (&delay1, &beta1, residual + GUARD, DMIN, DMAX, FrameSize);
	fndppf (&delay, &beta, residual + 2 * GUARD, DMIN, DMAX, FrameSize);
    }
    else {

	fndppf (&delay1, &beta1, residual + GUARD, DMIN, DMAX, FrameSize);
	fndppf (&delay, &beta, residual + 2 * GUARD, DMIN, DMAX, FrameSize);


    }
    if (beta1 > beta + 0.4) {
	if (fabs (delay - delay1) > 15) {
	    beta = beta1;
	    delay = delay1;
	}
	else {
	    beta = (beta + beta1) / 2.0;
	    delay = floor ((delay + delay1) / 2.0);
	}
    }
    else if ((beta1 > beta + 0.05) && fabs (delay1 - last_delay) < 7) {
	/* if delay1 is close to delay for last frame and beta1 > beta */
	if (fabs (delay - delay1) > 15) {
	    beta = beta1;
	    delay = delay1;
	}
	else {
	    beta = (beta + beta1) / 2.0;
	    j = (short) ((delay + delay1) / 2.0 + 0.5);

	    for (k = j - 1, sum0 = -1.0E30; k <= j + 1; k++) {
		for (i = 0, sum1 = 0.0; i < FrameSize - k; i++)
		    sum1 +=
			residual[GUARD * 3 / 2 + i] * residual[GUARD * 3 / 2 +
							       i + k];
		if (sum1 > sum0) {
		    sum0 = sum1;
		    k0 = k;
		}
	    }
	    delay = (float) k0;
	}
    }

    last_delay = delay;
    last_beta = beta;
    if (data_packet.WB_MODE_BIT == 1) {

	float tmpbuf[FrameSize];

	//compute fractional lag only for lag < 40 
	//this doesnt affect beta
	if (delay < 40) {
	    static float ic[16] = {
		-5.095727437053253e-003,
		1.068984979634032e-002,
		-2.110094095197110e-002,
		3.799185162812797e-002,
		-6.477470413983405e-002,
		1.104431941719034e-001,
		-2.086144185903460e-001,
		6.651013321897413e-001,
		6.651013321897413e-001,
		-2.086144185903460e-001,
		1.104431941719034e-001,
		-6.477470413983405e-002,
		3.799185162812797e-002,
		-2.110094095197110e-002,
		1.068984979634032e-002,
		-5.095727437053253e-003
	    };

	    //get corr at best integer delay
	    k = (int) delay;
	    for (i = 0, sum1 = 0.0; i < FrameSize - k; i++)
		sum1 +=
		    residual[GUARD * 3 / 2 + i] * residual[GUARD * 3 / 2 + i +
							   k];

	    //try int delay + 0.5
	    for (i = 0; i < FrameSize - k; i++) {
		for (j = 0, tmpbuf[i] = 0.0; j < 16; j++)
		    tmpbuf[i] +=
			ic[j] * residual[GUARD * 3 / 2 + i + k + 1 - 8 + j];
	    }

	    for (i = 0, sum0 = 0.0; i < FrameSize - k; i++)
		sum0 += residual[GUARD * 3 / 2 + i] * tmpbuf[i];

	    if (sum0 > sum1) {
		sum1 = sum0;
		delay = (float) k + 0.5;
	    }

	    //try int delay - 0.5
	    if (delay > DMIN) {
		for (i = 0; i < FrameSize - k; i++) {
		    for (j = 0, tmpbuf[i] = 0.0; j < 16; j++)
			tmpbuf[i] +=
			    ic[j] * residual[GUARD * 3 / 2 + i + k - 8 + j];
		}

		for (i = 0, sum0 = 0.0; i < FrameSize - k; i++)
		    sum0 += residual[GUARD * 3 / 2 + i] * tmpbuf[i];

		if (sum0 > sum1) {
		    sum1 = sum0;
		    delay = (float) k - 0.5;
		}
	    }
	}
//printf("%f\n",delay);
    }

    nacf =
	get_nacf_at_pitch (residual + GUARD, (int) delay1, (int) delay,
			   nacf_ap + 2);

    for (k = 0; k < 2; k++) {
	curr_ns_snr[k] = (next_ns_snr[0][k] + next_ns_snr[1][k]) / 2.0;
    }

    if (args->nacf_out) {
	float tmp;

	for (k = 2; k < 5; k++) {
	    tmp = nacf_ap[k] * 100;
	    write_samples (1, &tmp, 1);
	}
	tmp = nacf * 100;
	write_samples (1, &tmp, 1);
    }

    return;
}

/***** Compute the sensitivity weights for LSI quantization  *********
   Inputs:
          float *lsc -- lsc coeffs
	  float *Rs -- aotucorrs
	  double *P_s, *Q_s -- P Q coeffs derived from LPC coeffs
   Outputs:
          double *sens -- weights
	  ***********************************************************/
void
compute_sens (float *lsc, float *Rs, double *P_s, double *Q_s, double *sens)
{
    int i, j, k;
    double J[ORDER][ORDER], RJ[ORDER];

    for (i = 0; i < ORDER; i += 2) {
	J[i][ORDER - 1] = J[i][0] = 1.0;
	J[i][ORDER - 2] = J[i][1] = 2.0 * lsc[i] + P_s[0];
	J[i + 1][ORDER - 1] = -(J[i + 1][0] = 1.0);
	J[i + 1][ORDER - 2] = -(J[i + 1][1] = 2.0 * lsc[i + 1] + Q_s[0]);
	for (k = 2; k < ORDER / 2; k++) {
	    J[i][ORDER - 1 - k] =
		J[i][k] =
		2.0 * lsc[i] * J[i][k - 1] + P_s[k - 1] - J[i][k - 2];
	    J[i + 1][ORDER - 1 - k] = -(J[i + 1][k] =
					2.0 * lsc[i + 1] * J[i + 1][k - 1] +
					Q_s[k - 1] - J[i + 1][k - 2]);
	}

	RJ[0] = 1.0;
	for (j = 1; j < ORDER / 2; j++)
	    RJ[0] += SQR (J[i][j]);
	RJ[0] *= 2.0;
	for (k = 2; k <= ORDER - 2; k += 2) {
	    RJ[k] = J[i][k];
	    RJ[k - 1] =
		J[i][k - 1] + J[i][ORDER / 2 - 1 + k / 2] * J[i][ORDER / 2 -
								 k / 2] / 2.0;
	    for (j = 1; j < ORDER / 2 - k / 2; j++) {
		RJ[k] += J[i][j + k] * J[i][j];
		RJ[k - 1] += J[i][j + k - 1] * J[i][j];
	    }
	    RJ[k] *= 2.0;
	    RJ[k - 1] *= 2.0;
	}
	RJ[ORDER - 1] = J[i][ORDER - 1];
	sens[i] = Rs[0] * RJ[0] / 2.0;
	for (j = 1; j < ORDER; j++)
	    sens[i] += Rs[j] * RJ[j];
	sens[i] *= (1.0 - fabs (lsc[i]));
	sens[i] = (sens[i] > 0) ? sens[i] : 1.0;
	RJ[0] = 1.0;
	for (j = 1; j < ORDER / 2; j++)
	    RJ[0] += SQR (J[i + 1][j]);
	RJ[0] *= 2.0;
	for (k = 2; k <= ORDER - 2; k += 2) {
	    RJ[k] = J[i + 1][k];
	    RJ[k - 1] = J[i + 1][k - 1] +
		J[i + 1][ORDER / 2 - 1 + k / 2] * J[i + 1][ORDER / 2 -
							   k / 2] / 2.0;
	    for (j = 1; j < ORDER / 2 - k / 2; j++) {
		RJ[k] += J[i + 1][j + k] * J[i + 1][j];
		RJ[k - 1] += J[i + 1][j + k - 1] * J[i + 1][j];
	    }
	    RJ[k] *= 2.0;
	    RJ[k - 1] *= 2.0;
	}
	RJ[ORDER - 1] = J[i + 1][ORDER - 1];
	sens[i + 1] = Rs[0] * RJ[0] / 2.0;
	for (j = 1; j < ORDER; j++)
	    sens[i + 1] += Rs[j] * RJ[j];
	sens[i + 1] *= (1.0 - fabs (lsc[i + 1]));
	sens[i + 1] = (sens[i + 1] > 0) ? sens[i + 1] : 1.0;
    }
}				/* End Next Sensitivities */


/*======================================================================*/
/*         ..Encode speech data.                                        */
/*----------------------------------------------------------------------*/


//char PPP_MODE_E='Q' ;

//int rcelp_half_rateE=0,prev_dim_and_burstE=0,dim_and_burstE=0;

void
FGV_MEM::encode (short &rate, short *codeBuf)
{
    /*....(local) variables.... */
    register int i, j;
    unsigned short tmp;


    short WB_HR = 0x3F;		//63 in 6 bits => (127 ,126 are reserved by WB_HR)

    if (data_packet.WB_MODE_BIT == 1) {
	//some backing up for next frame
	prev_nacf = nacf;
    }
    for (i = 0; i < 2; i++)
	nacf_ap[i] = nacf_ap[i + 2];

    celp_mdct_dec = 0;		//default is CELP.

    if (args->rfileP) {
	if (!feof (args->rfileP)) {
	    fread (&rate, sizeof (short), 1, args->rfileP);
	}
	else {
	    fprintf (stderr, "End of Rate Parameter File Reached\n");
	    exit (-1);
	}

	//Set the full, half and quarter rate PPP to rate=3
	if (rate == 9) {
	    rcelp_half_rateE = 0;
	    PPP_MODE_E = 'Q';
	    rate = 3;
	    data_packet.PACKET_RATE = 2;
	    data_packet.MODE_BIT = 0;	//to be set to 1 for ppp
	}
	else if (rate == 12) {
	    rcelp_half_rateE = 0;
	    PPP_MODE_E = 'H';
	    rate = 3;
	}
	else if (rate == 15 || rate == 16) {
	    rcelp_half_rateE = 0;
	    PPP_MODE_E = 'F';
	    rate = 3;
	}
	else if (rate == 6) {
	    rate = 3;
	    rcelp_half_rateE = 1;
	}
	else if (rate == 3) {	// For OP-0, we may get rate=3, but we need RCELP 1/2
	    rcelp_half_rateE = 1;
	}
	else if (rate == 8)
	    rate = 4;

	assert (rate >= 1 && rate <= 4);
    }

    bit_rate = rate;
    if (data_packet.WB_MODE_BIT == 1) {
    }
    else {


    }
    //to find the voicing threshold for adding noise
    float Po = (float) delay;

    /* Do encoding here */

    if (data_packet.WB_MODE_BIT == 1) {

	if ((bit_rate == 1) && (lastrateE != 1) && (encode_HO == 0))	//1st ER frame after a NER frame , force to nelp (hangover frame)
	{
	    bit_rate = 2;
	    rate = 2;
	    encode_HO = 1;
	}
	else
	    encode_HO = 0;




    }
    switch (bit_rate) {

    case 1:
	silence_encoder ();
	PACKET_RATE = 1;
	break;

    case 2:
	nelp_encoder (q_resid);
	PACKET_RATE = 2;
	MODE_BIT = 0;
	break;

    case 3:
	if (rcelp_half_rateE || dim_and_burstE) {

	    celpmdct_resid_calc (bit_rate);	//residual calculation from quantized LPCs...common to both celp and mdct and needed by discriminator 
	    celp_encoder (bit_rate);
	    PACKET_RATE = 3;
	}
	else {
	    voiced_encoder (q_resid, bit_rate);
	    if (bit_rate == 3) {	//No bumpup
		if (PPP_MODE_E == 'Q') {
		    PACKET_RATE = 2;
		    MODE_BIT = 1;
		}
		else if (PPP_MODE_E == 'F') {
		    PACKET_RATE = 4;
		    MODE_BIT = 1;
		}
	    }
	}
	break;

    case 4:





	PACKET_RATE = 4;
	MODE_BIT = 0;



	celpmdct_resid_calc (bit_rate);	//residual calculation from quantized LPCs...common to both celp and mdct and needed by discriminator


	switch (args->fullrate_coding_method) {
	case 0:
	    celp_mdct_dec = 0;
	    break;
	case 1:
	    celp_mdct_dec = 1;
	    break;
	case 2:
	    celp_mdct_discriminator (&celp_mdct_dec);
	    break;

	}




	if (celp_mdct_dec == 0) {
	    celp_encoder (bit_rate);
	    data_packet.Celp_Mdct_Flag = 0;
	    for (j = 0; j < 80; j++)
		prev_synth[j] = prev_synthSave[j];
	}
	else {
	    music_encoder ();

	    data_packet.Celp_Mdct_Flag = 1;

	    if (data_packet.WB_MODE_BIT == 1) {
		//set parameters for UB encoding
		P_LB.Mode = 4;	//this is just an identifier for the UB_encoder
		P_LB.PitchGain = 1;

		P_LB.PitchLag = 60;


		for (j = 0; j < 160; j++)
		    P_LB.whtnd_la[j] = UBExcitationMusic[j];

		for (j = 0; j < 3; j++)
		    frame_shift[j] = 0.0;
	    }

	    Update_CELPMEM ();	// update memory in celp so that the decoder is in sync with the encoder

	}

	prev_celp_mdct_dec = celp_mdct_dec;



	break;




    }

    if (bit_rate != 4) {
	mdct_hist_cnt = 0, celp_hist_cnt = 0;
    }

    if (data_packet.PACKET_RATE != 4) {
	for (j = 0; j < 80; j++)
	    prev_synth[j] = 0;
    }


    switch (data_packet.PACKET_RATE) {
    case 1:
	//silence


	if (data_packet.WB_MODE_BIT == 1) {
	    //fprintf(stderr,"packing eighth rates codes\n");
	    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 5,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 5,
		     PktPtr);
	    Bitpack (data_packet.SILENCE_GAIN, (unsigned short *) TxPkt,
		     NUM_EQ_BITS_WB, PktPtr);
	    //pack 1 in the 16th bit if WB
	    Bitpack (1, (unsigned short *) TxPkt, 1, PktPtr);
	}
	else {			//narrowband case

	    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 4,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 4,
		     PktPtr);
	    Bitpack (data_packet.SILENCE_GAIN, (unsigned short *) TxPkt,
		     NUM_EQ_BITS, PktPtr);

	}
	break;



    case 2:
	if (data_packet.MODE_BIT == 0) {
	    if (data_packet.WB_MODE_BIT == 1) {
		//NELP
		Bitpack (WB_HR, (unsigned short *) TxPkt, 6, PktPtr);	//WB HALF RATE IDENTIFIER

		Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 6,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 6,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt, 9,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[3], (unsigned short *) TxPkt, 7,
			 PktPtr);
		Bitpack (data_packet.NELP_GAIN_IDX[0][0],
			 (unsigned short *) TxPkt, 5, PktPtr);
		Bitpack (data_packet.NELP_GAIN_IDX[1][0],
			 (unsigned short *) TxPkt, 6, PktPtr);
		Bitpack (data_packet.NELP_GAIN_IDX[1][1],
			 (unsigned short *) TxPkt, 6, PktPtr);
		Bitpack (data_packet.NELP_FID, (unsigned short *) TxPkt, 2,
			 PktPtr);
	    }
	    else {		//narrowband case


		if (SPL_HNELP == 0) {
		    //NELP
		    Bitpack (data_packet.MODE_BIT, (unsigned short *) TxPkt,
			     1, PktPtr);
		    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt,
			     8, PktPtr);
		    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt,
			     8, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[0][0],
			     (unsigned short *) TxPkt, 5, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[1][0],
			     (unsigned short *) TxPkt, 6, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[1][1],
			     (unsigned short *) TxPkt, 6, PktPtr);
		    Bitpack (data_packet.NELP_FID, (unsigned short *) TxPkt,
			     2, PktPtr);
		}
		else {		//SPL_HNELP
		    data_packet.PACKET_RATE = 3;
		    Bitpack (0x6E, (unsigned short *) TxPkt, 7, PktPtr);	//110 in dec
		    Bitpack (1, (unsigned short *) TxPkt, 2, PktPtr);	//QNELP identifier 
		    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt,
			     8, PktPtr);
		    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt,
			     8, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[0][0],
			     (unsigned short *) TxPkt, 5, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[1][0],
			     (unsigned short *) TxPkt, 6, PktPtr);
		    Bitpack (data_packet.NELP_GAIN_IDX[1][1],
			     (unsigned short *) TxPkt, 6, PktPtr);
		    Bitpack (data_packet.NELP_FID, (unsigned short *) TxPkt,
			     2, PktPtr);

		}		//SPL_HNELP    
	    }			// WB_MODE_BIT==1
	}
	else if (data_packet.MODE_BIT == 1) {
	    //QPPP
	    Bitpack (data_packet.MODE_BIT, (unsigned short *) TxPkt, 1,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 10,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 6,
		     PktPtr);
	    Bitpack (data_packet.Q_delta_lag, (unsigned short *) TxPkt, 4,
		     PktPtr);
	    Bitpack (data_packet.AMP_IDX[0], (unsigned short *) TxPkt, 6,
		     PktPtr);
	    Bitpack (data_packet.AMP_IDX[1], (unsigned short *) TxPkt, 6,
		     PktPtr);
	    Bitpack (data_packet.POWER_IDX, (unsigned short *) TxPkt, 6,
		     PktPtr);
	}
	else {
	    cerr << "MODE_BIT neither 0 nor 1 in Frame #" << decode_fcnt <<
		"\n";
	}

	break;
    case 3:
	//Half Rate Celp
	Bitpack (data_packet.DELAY_IDX, (unsigned short *) TxPkt, 7, PktPtr);
	Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 7, PktPtr);
	Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 7, PktPtr);
	Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt, 8, PktPtr);
	for (i = 0; i < NoOfSubFrames; i++)
	    Bitpack (data_packet.ACBG_IDX[i], (unsigned short *) TxPkt, 3,
		     PktPtr);
	for (i = 0; i < NoOfSubFrames; i++)
	    Bitpack (data_packet.FCB_PULSE_IDX[i][0],
		     (unsigned short *) TxPkt, 10, PktPtr);
	for (i = 0; i < NoOfSubFrames; i++)
	    Bitpack (data_packet.FCBG_IDX[i], (unsigned short *) TxPkt, 4,
		     PktPtr);

	break;
    case 4:
	if (celp_mdct_dec == 0) {	//true for narrowband case too

	    if (data_packet.MODE_BIT == 0) {
		//FCELP
		if (data_packet.WB_MODE_BIT == 1) {
		    Bitpack (data_packet.DELAY_IDX, (unsigned short *) TxPkt,
			     7, PktPtr);
		    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt,
			     6, PktPtr);
		    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt,
			     6, PktPtr);
		    Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt,
			     9, PktPtr);
		    Bitpack (data_packet.LSP_IDX[3], (unsigned short *) TxPkt,
			     7, PktPtr);

		}
		else {		//narrowband case

		    Bitpack (data_packet.MODE_BIT, (unsigned short *) TxPkt,
			     1, PktPtr);
		    Bitpack (data_packet.DELAY_IDX, (unsigned short *) TxPkt,
			     7, PktPtr);
		    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt,
			     6, PktPtr);
		    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt,
			     6, PktPtr);
		    Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt,
			     9, PktPtr);
		    Bitpack (data_packet.LSP_IDX[3], (unsigned short *) TxPkt,
			     7, PktPtr);

		}

		if (data_packet.WB_MODE_BIT == 1) {
		    // RS1
		}
		else {		//narrowband case

		    Bitpack (data_packet.DELTA_DELAY_IDX,
			     (unsigned short *) TxPkt, 5, PktPtr);

		}
		if (data_packet.WB_MODE_BIT == 1) {
		}
		for (i = 0; i < NoOfSubFrames; i++)
		    Bitpack (data_packet.ACBG_IDX[i],
			     (unsigned short *) TxPkt, 3, PktPtr);
		if (data_packet.WB_MODE_BIT == 0) {
		    for (i = 0; i < NoOfSubFrames; i++) {
			Bitpack (data_packet.FCB_PULSE_IDX[i][1],
				 (unsigned short *) TxPkt, 8, PktPtr);
			Bitpack (data_packet.FCB_PULSE_IDX[i][0],
				 (unsigned short *) TxPkt, 8, PktPtr);
			tmp = (data_packet.FCB_PULSE_IDX[i][3] & 0x00FF);

			Bitpack (tmp, (unsigned short *) TxPkt, 8, PktPtr);


			Bitpack (data_packet.FCB_PULSE_IDX[i][2],
				 (unsigned short *) TxPkt, 8, PktPtr);

			tmp =
			    ((data_packet.FCB_PULSE_IDX[i][3] >> 8) & 0x00FF);
			Bitpack (tmp, (unsigned short *) TxPkt, 3, PktPtr);
		    }
		}
		else {		//wideband case


		    short fxindices[NoOfSubFrames][3];


		    for (i = 0; i < NoOfSubFrames; i++) {
			fxindices[i][0] =
			    (unsigned short) ((unsigned short) data_packet.
					      FCB_PULSE_IDX[i][0] | (unsigned
								     short)
					      (data_packet.
					       FCB_PULSE_IDX[i][1] << 8));
			fxindices[i][1] =
			    (unsigned short) data_packet.FCB_PULSE_IDX[i][2];
			fxindices[i][2] =
			    (unsigned short) data_packet.FCB_PULSE_IDX[i][3];
		    }

		    for (i = 0; i < NoOfSubFrames; i++) {
			Bitpack (fxindices[i][0], (unsigned short *) TxPkt,
				 16, PktPtr);
			Bitpack (fxindices[i][1], (unsigned short *) TxPkt, 8,
				 PktPtr);
		    }
		    Bitpack (fxindices[0][2], (unsigned short *) TxPkt, 8,
			     PktPtr);
		    Bitpack (fxindices[1][2], (unsigned short *) TxPkt, 7,
			     PktPtr);
		    Bitpack (fxindices[2][2], (unsigned short *) TxPkt, 7,
			     PktPtr);

		    Bitpack (idxe, (unsigned short *) TxPkt, 3, PktPtr);
		    for (i = 0; i < NoOfSubFrames; i++)
			Bitpack (data_packet.FCBG_IDX[i],
				 (unsigned short *) TxPkt, 4, PktPtr);
		}
		if (data_packet.WB_MODE_BIT == 0)
		    for (i = 0; i < NoOfSubFrames; i++)
			Bitpack (data_packet.FCBG_IDX[i],
				 (unsigned short *) TxPkt, 5, PktPtr);

	    }
	    else if (data_packet.MODE_BIT == 1) {
		//FPPP
		Bitpack (data_packet.MODE_BIT, (unsigned short *) TxPkt, 1,
			 PktPtr);
		Bitpack (data_packet.delayindex, (unsigned short *) TxPkt, 7,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 6,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 6,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt, 9,
			 PktPtr);
		Bitpack (data_packet.LSP_IDX[3], (unsigned short *) TxPkt, 7,
			 PktPtr);
		Bitpack (data_packet.F_ROT_IDX, (unsigned short *) TxPkt, 7,
			 PktPtr);
		Bitpack (data_packet.A_AMP_IDX[0], (unsigned short *) TxPkt,
			 6, PktPtr);
		Bitpack (data_packet.A_AMP_IDX[1], (unsigned short *) TxPkt,
			 6, PktPtr);
		Bitpack (data_packet.A_AMP_IDX[2], (unsigned short *) TxPkt,
			 8, PktPtr);
		Bitpack (data_packet.A_POWER_IDX, (unsigned short *) TxPkt, 8,
			 PktPtr);
		for (i = 0; i < NUM_FIXED_BAND; i++) {
		    if (i < 3)
			Bitpack (data_packet.F_ALIGN_IDX[i],
				 (unsigned short *) TxPkt, 5, PktPtr);
		    else
			Bitpack (data_packet.F_ALIGN_IDX[i],
				 (unsigned short *) TxPkt, 6, PktPtr);
		}
	    }
	    else {
		cerr << "MODE_BIT neither 0 nor 1 in Frame #" << decode_fcnt
		    << "\n";
	    }
	    break;
	}
	else {

	    Bitpack (data_packet.LSP_IDX[0], (unsigned short *) TxPkt, 6,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[1], (unsigned short *) TxPkt, 6,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[2], (unsigned short *) TxPkt, 9,
		     PktPtr);
	    Bitpack (data_packet.LSP_IDX[3], (unsigned short *) TxPkt, 7,
		     PktPtr);

	    for (i = 0; i < MDCT_NWORDS - 1; i++) {
		Bitpack (data_packet.MDCT_IDX[i], (unsigned short *) TxPkt,
			 15, PktPtr);
	    }
	    Bitpack (data_packet.MDCT_IDX[MDCT_NWORDS - 1],
		     (unsigned short *) TxPkt, MDCT_REMAINDER, PktPtr);

	    Bitpack (data_packet.FrameGain_IDX, (unsigned short *) TxPkt, 7,
		     PktPtr);
	    Bitpack (data_packet.NoiseGain_IDX, (unsigned short *) TxPkt, 2,
		     PktPtr);

	    if (data_packet.WB_MODE_BIT == 0) {
		Bitpack (0, (unsigned short *) TxPkt, 1, PktPtr);	//169 th bit is the NB/WB bit ( 0/1)
		Bitpack (1, (unsigned short *) TxPkt, 1, PktPtr);	//170 th bit is the CELP/MDCT bit ( 0/1)
		Bitpack (1, (unsigned short *) TxPkt, 1, PktPtr);	//171 st bit is the EVRC-B/EVRC-WB bit ( 0/1)
	    }

	    break;
	}

    }



    if (args->dtx) {
	/*Spectral tilt part added from FOURGV_FL */

						short nsid = 0;
		float rc[ORDER + 1];

	if ((data_packet.PACKET_RATE == 1) && (lastrateE != 1))
	    dtx_posn_prev_NER = encode_fcnt;

	if ((lpcgflag == 0) && (dtx_nocompute == 2))
	    dtx_nocompute = 0;

	if (lpcgflag == 1)	//maybe moving to a FR frame
	    dtx_nocompute = 1;

	if ((lpcgflag == 0) && (dtx_nocompute == 1))
	    dtx_nocompute++;


	if ((data_packet.PACKET_RATE == 1)
	    && ((encode_fcnt - dtx_posn_prev_NER) > 5) && (dtx_nocompute == 0)) {
	    /* computing the tilt parameter for noise trigger */

	    dtx_flag = 0;

	    lsp2a (pci, lsp, ORDER);

	    //for(i=0;i<LPCORDER;i++) pci_fx[i] = -(pci_fx[i]);

	    a2rc (pci, rc, LPCORDER);


	    if (dtx_ft == 1) {
		dtx_prev_refl0 = rc[0];
		dtx_ft = 0;
		dtx_running_avg = rc[0];
	    }


	    dtx_ra_deltasum += (0.80 * dtx_running_avg + 0.20 * rc[0]) - dtx_running_avg;

	    dtx_prev_refl0 = rc[0];

	    dtx_running_avg = 0.80 * dtx_running_avg + 0.20 * rc[0];

	    if (fabs (dtx_ra_deltasum) > 0.2) {
		dtx_flag = 1;
		dtx_ra_deltasum = 0;
		dtx_running_avg = 0;
		dtx_ft = 1;
		nsid = 0;
		if (noSID == 1)
		    nsid = 1;

	    }
	}
	else
	    dtx_flag = 0;

	unsigned int Isil = 0;

	/*Finished: Added from FOURGV_FL */



	// Wideband Smart blanking Undestand endianees
	// No need to swap bytes
	short tmpbuf = TxPkt[0];

	short rt;

	rt = (short) data_packet.PACKET_RATE;


	Isil = SB_enc.Encode (&rt, &tmpbuf, short (dtx_flag), nsid);


//    SB_enc.Encode(&rt, &tmpbuf);  // Encode does not modify the frame

	// Put the data back where it should be
	rate = data_packet.PACKET_RATE = rt;

#if 1				//bug-fix
	if (rate != 0)
	    TxPkt[0] = tmpbuf;
	else
	    TxPkt[0] = 0;
#endif

    }




    if (args->lag_out)
	fwrite (&delay, sizeof (float), 1, stdout);
    if (data_packet.WB_MODE_BIT == 1) {
/* UB specific stuff */
	if ((rate == 4 && celp_mdct_dec == 0) || rate == 3)
	    P_LB.PitchLag = delay;
	else {
	    if (celp_mdct_dec != 1)
		P_LB.PitchLag = pdelay;	// can hold any value since P_LB.PitchGain set to 0!
	}
    }
    if (args->form_res_out)
	write_samples (1, residual + GUARD, FrameSize);

    for (i = 0; i < ACBMemSize; i++)
	prev_en[i] = Excitation[i];

    for (i = 0, prev_uvgain = 0.0; i < 16; i++)
	prev_uvgain +=
	    Excitation[ACBMemSize - 16 + i] * Excitation[ACBMemSize - 16 + i];
    prev_uvgain = sqrt (prev_uvgain / 16);

    //lastrateE = bit_rate;
    rate = bit_rate;

    prev_dim_and_burstE = dim_and_burstE;
    LAST_PACKET_RATE_E = PACKET_RATE;
    LAST_MODE_BIT_E = MODE_BIT;

    for (i = 0; i < PACKWDSNUM; i++) {
	codeBuf[i] = TxPkt[i];
    }

    if (args->packet_out) {
	write (1, (char *) &rate, sizeof (short));
	for (i = 0; i < PACKWDSNUM; i++)
	    write (1, (char *) (codeBuf + i), sizeof (short));
    }


//printf("Frame is %ld\n",encode_fcnt);  
    encode_fcnt++;
    if (data_packet.WB_MODE_BIT == 1) {
/* computing the tilt parameter */
	float refl[10];

	lsp2a (pci, lsp, ORDER);
	LPC_pred2refl (pci, refl, ORDER);
	P_LB.Tilt = refl[0];
    }
  /*======================================================================*/
    /*         ..Save LSPs.                                                 */
  /*----------------------------------------------------------------------*/
    for (j = 0; j < ORDER; j++) {
	OldOldlspE[j] = OldlspE[j];
	OldlspE[j] = lsp[j];
	Oldlsp_nq[j] = lsp_nq[j];
    }




}
