/**********************************************************************
Each of the companies; Qualcomm, and Lucent (hereinafter
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
all Normative portions of the QCELP-13 Variable Rate Speech Codec as
it exists on the date of submittal of this form, should the QCELP-13 be
approved as a Specification or Report by 3GPP2, or as a transposed
Standard by any of the 3GPP2's Organizational Partners, the Source(s)
state(s) that a worldwide license to reproduce, use and distribute the
Software, the license rights to which are held by the Source(s), will
be made available to applicants under terms and conditions that are
reasonable and non-discriminatory, which may include monetary compensation,
and only to the extent necessary for the practice of any or all of the
Normative portions of the QCELP-13 or the field of use of practice of the
QCELP-13 Specification, Report, or Standard.  The statement contained above
is irrevocable and shall be binding upon the Source(s).  In the event
the rights of the Source(s) in and to copyright or copyright license
rights subject to such commitment are assigned or transferred, the
Source(s) shall notify the assignee or transferee of the existence of
such commitments.
*******************************************************************/
#define MATCH_1617  1

/* quantize.c - quantization routines */

#include <math.h>
#include "celp.h"
#include "quantize.h"

#define	PI_QC	3.14159265358979323846


void compute_sns(
    float *R,
    float *lpc,
    float *lsp,
    float *sns
)
{

    int  i, j, k;
    float J[10];
    float P[12], Q[12], Tmp[12], sinLSP[10], cosLSP[10];
    float ftmp;

    P[0]=1;
    P[11]=1;
    Q[0]=1;
    Q[11]=-1;
    for (i=0; i<10; i++) {
	P[i+1]= -lpc[i]-lpc[9-i];
	Q[i+1]= -lpc[i]+lpc[9-i];
    }

    for (i=0; i<10; i++) {
	if (i%2==0) {
	    for (j=0; j<=11; j++) {
		Tmp[j]=P[j];
	    }
	}
	else {
	    for (j=0; j<=11; j++) {
		Tmp[j]=Q[j];
	    }
	}

	/* divide by sub-polynomial */

	/* factor of PI not needed in real DSP, since only */
	/* care about relative sensitivities in quantizer */

	sinLSP[i]=sin(PI_QC*lsp[i])*3.1415927;
	cosLSP[i]=cos(PI_QC*lsp[i])*2;

	for (j=0; j<LPCORDER/2; j++) {
	    J[j]=Tmp[j];
	    Tmp[j+1]+= cosLSP[i]*J[j];
	    Tmp[j+2]-= J[j];

	    J[j]*=sinLSP[i];
	}
	if (i%2==0) {
	    for (j=0; j<LPCORDER/2; j++) {
		J[9-j]=J[j];
	    }
	}
	else {
	    for (j=0; j<LPCORDER/2; j++) {
		J[9-j]= -J[j];
	    }
	}

	/* Compute ii element of J' R J */
	ftmp=0;
	for (j=0; j<10; j++) {
	    ftmp+=J[j]*J[j];
	}
	sns[i]=R[0]*ftmp;

	for (j=1; j<10; j++) {
	    ftmp=0;
	    for (k=j; k<10; k++) {
		ftmp+=J[k]*J[k-j];
	    }
	    sns[i]+=2.0*R[j]*ftmp;
	}
    }
}


void quantize_lpc(
    int                 mode,
    float               lpc[LPCORDER],
    float               *lsp,
    float               *qlsp,
    float               *R,
    struct LPCPARAMS    *lpc_params,
    struct ENCODER_MEM  *e_mem
)
{
    int   i,j, k;
    float err, minerr=1E20;
    int minj;
    float dlsp[3];
    float *VQ;
    float suberr;
    float sns[LPCORDER];

    if (LSP_QTYPE[mode]==VQT) {
	/* Pairwise differential LSP VQ using optimal LSP sensitivities */
	compute_sns(R, lpc, lsp, sns);

	for (i=0; i<LPCORDER/2; i++) {
	    if (i==0) {
		dlsp[0]=lsp[0];
	    }
	    else {
		dlsp[0]=lsp[2*i]-qlsp[2*i-1];
	    }
	    dlsp[1]=lsp[2*i+1]-lsp[2*i];

	    switch (i) {
	    case 0:
		VQ=&LSPVQ0[0][0];
		break;
	    case 1:
		VQ=&LSPVQ1[0][0];
		break;
	    case 2:
		VQ=&LSPVQ2[0][0];
		break;
	    case 3:
		VQ=&LSPVQ3[0][0];
		break;
	    case 4:
		VQ=&LSPVQ4[0][0];
		break;
	    }
	    for (j=0; j<LSPVQSIZE[i]; j++) {
		err=0;
		suberr=0;
		for (k=0; k<2; k++) {
		    suberr+=(dlsp[k]-VQ[2*j+k]);
		    err+=suberr*suberr*sns[k+2*i];
		}

		if ((j==0)||(minerr>err)) {
		    minerr=err;
		    minj=j;
		}
	    }
	    lpc_params->qcode_lsp[i]=minj;
	    if (i==0) {
		qlsp[0]=VQ[2*minj];
	    }
	    else {
		qlsp[2*i]=qlsp[2*i-1]+VQ[2*minj];
	    }
	    qlsp[2*i+1]=qlsp[2*i]+VQ[2*minj+1];
	}
	unquantize_lsp(mode, qlsp, &(e_mem->dec.last_qlsp[0]),
		       lpc_params->qcode_lsp, &(e_mem->dec));
    }
    else {
	for (i=0; i<LPCORDER; i++) {

#if MATCH_1617
	    err=lsp[i] - (float)(i+1)/(float)(LPCORDER+1) - (e_mem->dec.pred_qlsp[i]);
#else
	    err=lsp[i] - (float)(i+1)/(float)(LPCORDER+1)
	      - LSP_DPCM_DECAY[mode]*
		(e_mem->dec.last_qlsp[i]- (float)(i+1)/(float)(LPCORDER+1));
#endif

	    lin_quant(&(lpc_params->qcode_lsp[i]),
		      MIN_DELTA_LSP[mode][i], MAX_DELTA_LSP[mode][i],
		      NUM_LSP_QLEVELS[mode][i], err);

	}
	unquantize_lsp(mode, qlsp, &(e_mem->dec.last_qlsp[0]),
		       lpc_params->qcode_lsp, &(e_mem->dec));
    }
}/* end of quantize_lpc() */

void check_lsp_stab(float *qlsp)
{
    INTTYPE i;

    if (qlsp[0]<LSP_SPREAD_FACTOR) qlsp[0]=LSP_SPREAD_FACTOR;
    for (i=1; i<LPCORDER; i++) {
	if (qlsp[i]-qlsp[i-1]<LSP_SPREAD_FACTOR) {
	    qlsp[i]=qlsp[i-1]+LSP_SPREAD_FACTOR;
	}
    }
    if (qlsp[LPCORDER-1]>1.0-LSP_SPREAD_FACTOR) {
	qlsp[LPCORDER-1]=1.0-LSP_SPREAD_FACTOR;
    }
    for (i=LPCORDER-2; i>=0; i--) {
	if (qlsp[i+1]-qlsp[i]<LSP_SPREAD_FACTOR) {
	    qlsp[i]=qlsp[i+1]-LSP_SPREAD_FACTOR;
	}
    }

}

void unquantize_lsp(
    int                 mode,
    float               *qlsp,          /* output unquantized lsp's */
    float               *last_qlsp,     /* input - unquantized last lsp */
    int                 *qcode,
    struct DECODER_MEM  *d_mem
)
{
  int   i;
  float err;
  float *VQ;
  float smooth;
  float lsp_spread_factor;

  if (LSP_QTYPE[mode]==VQT) {
    for (i=0; i<LPCORDER/2; i++) {
      switch (i) {
      case 0:
	VQ=&LSPVQ0[0][0];
	break;
      case 1:
	VQ=&LSPVQ1[0][0];
	break;
      case 2:
	VQ=&LSPVQ2[0][0];
	break;
      case 3:
	VQ=&LSPVQ3[0][0];
	break;
      case 4:
	VQ=&LSPVQ4[0][0];
	break;
      }
      if (i==0) {
	qlsp[0]=VQ[2*qcode[i]];
      }
      else {
	qlsp[2*i]=qlsp[2*i-1]+VQ[2*qcode[i]];
      }

      qlsp[2*i+1]=qlsp[2*i]+VQ[2*qcode[i]+1];
    }
    for (i=0; i<LPCORDER; i++){/* subtract off bias */
#if MATCH_1617
      d_mem->pred_qlsp[i] = 0.90625 * (qlsp[i] - (float)(i+1)/(float)(LPCORDER+1));
#else
      d_mem->pred_qlsp[i] = qlsp[i] - (float)(i+1)/(float)(LPCORDER+1);
#endif
    }
  }
  else {/* EIGHTH rate */
    for (i=0; i<LPCORDER; i++) {
      if(qcode[i] == 1)
	err = 0.02;
      else
	err = -0.02;

#if MATCH_1617
      qlsp[i] = err + d_mem->pred_qlsp[i];
      d_mem->pred_qlsp[i] = LSP_DPCM_DECAY[mode] * qlsp[i];
#else
      qlsp[i] = err + LSP_DPCM_DECAY[mode] * d_mem->pred_qlsp[i];
      d_mem->pred_qlsp[i] = qlsp[i];
#endif

      /* add in bias */
      qlsp[i]+=  (float)(i+1)/(float)(LPCORDER+1);
    }
  }

  if(mode == EIGHTH){
    /* Check for stability and perform LSP spreading if necessary */
    lsp_spread_factor = LSP_SPREAD_FACTOR;

    if (qlsp[0]<lsp_spread_factor) qlsp[0]=lsp_spread_factor;
    for (i=1; i<LPCORDER; i++) {
      if (qlsp[i]-qlsp[i-1]<lsp_spread_factor) {
	qlsp[i]=qlsp[i-1]+lsp_spread_factor;
      }
    }
    if (qlsp[LPCORDER-1]>1.0-lsp_spread_factor) {
      qlsp[LPCORDER-1]=1.0-lsp_spread_factor;
    }
    for (i=LPCORDER-2; i>=0; i--) {
      if (qlsp[i+1]-qlsp[i]<lsp_spread_factor) {
	qlsp[i]=qlsp[i+1]-lsp_spread_factor;
      }
    }

    if (d_mem->low_rate_cnt<10) {
      smooth=0.125;
    }
    else{
      smooth = 0.9;
    }
    for (i=0; i<LPCORDER; i++){
      qlsp[i]=  smooth*last_qlsp[i]+ (1.0-smooth)*qlsp[i];
    }
  }

}/* end of unquantize_lsp() */

void unquantize_lpc(
    int                 mode,
    float               *qlsp,           /* output unquantized lsp */
    struct DECODER_MEM  *d_mem,
    struct LPCPARAMS    *lpc_params
)
{
  unquantize_lsp(mode, qlsp, d_mem->last_qlsp, lpc_params->qcode_lsp, d_mem);
}


/********************************************************
  lsp_erasure_check()
  checks LSP's for FULL, HALF, and QUARTER rate packets
  to determine if frame needs to be erased due to a possibly
  bad rate decision.

********************************************************/
int lsp_erasure_check(
    int    mode,
    int    *qcode
)
{
  int   i;
  float *VQ;
  float  qlsp[LPCORDER];  /* reconstructed/unquantized lsp's */

#if DO_DSP_INTEGER_MATH
  float vtmp;
#endif

    for (i=0; i<LPCORDER/2; i++) {
      switch (i) {
      case 0:
	VQ=&LSPVQ0[0][0];
	break;
      case 1:
	VQ=&LSPVQ1[0][0];
	break;
      case 2:
	VQ=&LSPVQ2[0][0];
	break;
      case 3:
	VQ=&LSPVQ3[0][0];
	break;
      case 4:
	VQ=&LSPVQ4[0][0];
	break;
      }
      if (i==0) {
#if DO_DSP_INTEGER_MATH
	vtmp = rnd1617(VQ[2*qcode[i]]*QUANT14BIT);
	qlsp[0]=vtmp/(float)QUANT14BIT;
#else
	qlsp[0]=VQ[2*qcode[i]];
#endif
      }
      else {
#if DO_DSP_INTEGER_MATH
	vtmp = rnd1617(VQ[2*qcode[i]]*QUANT14BIT);
	qlsp[2*i]=qlsp[2*i-1]+vtmp/(float)QUANT14BIT;
#else
	qlsp[2*i]=qlsp[2*i-1]+VQ[2*qcode[i]];
#endif
      }
#if DO_DSP_INTEGER_MATH
      vtmp = rnd1617(VQ[2*qcode[i]+1]*QUANT14BIT);
      qlsp[2*i+1]=qlsp[2*i]+vtmp/(float) QUANT14BIT;
#else
      qlsp[2*i+1]=qlsp[2*i]+VQ[2*qcode[i]+1];
#endif
    }
  if(mode == FULLRATE_VOICED || mode == HALFRATE_VOICED){
    /* check for conditions that require an ERASURE */
    if(qlsp[9] <= 0.66 || qlsp[9] >= 0.985){
      return(1);
    }
    for(i = 4; i < 10; i++){
      if(fabs(qlsp[i] - qlsp[i-4]) < 0.0931)
	return(1);
    }
  }
  else if(mode == QUARTERRATE_UNVOICED){
    if(qlsp[9] <= 0.70 || qlsp[9] >= 0.97)
      return(1);
    for(i = 3; i < 10; i++){
      if(fabs(qlsp[i] - qlsp[i-3]) < 0.08)
	return(1);
    }
  }
  else{
    fprintf(stderr,"lsp_erasure_check: illegal mode = %d\n", mode);
    exit(-1);
  }
  return(0);  /* Do Not Erase Packet */

}/* end of lsp_erasure_check() */


void quantize_min_lag(
    int  min_lag,
    int  *qcode_min_lag
)
{
    *qcode_min_lag= min_lag-MINLAG+1;
}

void unquantize_min_lag(min_lag, qcode_min_lag)
int  *min_lag;
int  qcode_min_lag;
{
    *min_lag= qcode_min_lag+MINLAG-1;
}

void lin_quant(
    int   *qcode,
    float min,
    float max,
    int   num_levels,
    float input
)
{
    *qcode=(int)((num_levels-1)*(input-min)/(max-min)+0.5);
    if (*qcode>=num_levels) *qcode=num_levels-1;
    if (*qcode<0) *qcode=0;

}

void lin_unquant(
    float *output,
    float min,
    float max,
    int   num_levels,
    int   qcode
)
{
    *output=(max-min)*(float)(qcode)/(float)(num_levels-1) + min;
}

void unquantize_i(
    int   *i,
    int   *qcode_i
)
{
    *i=*qcode_i;
}

void quantize_i(
    int   *i,
    int   *qcode_i
)
{
    *qcode_i= *i;
    unquantize_i(i, qcode_i);
}

void unquantize_lag(
    int   *lag,
    int   *qcode_lag,
    int   *frac,
    int   *qcode_frac
)
{

  if(*qcode_lag == 0){
    *lag = MAXLAG;
    *frac = 0;
  }
  else if(*qcode_frac == 0){
    *lag=*qcode_lag + 16;
    *frac = 0;
  }
  else if(*qcode_frac == 1){
    *lag=*qcode_lag + 16;
    *frac = 2;
  }
  else{
    printf("unquantize_lag: qcode_lag %d and qcode_frac %d\n", *qcode_lag, *qcode_frac);
    exit(-4);
  }
}

/* quantize/encode pitch lag and pitch lag fraction bit */

void quantize_lag(
    int   *lag,
    int   *qcode_lag,
    int   *frac,
    int   *qcode_frac
)
{
  if(*lag == 0){
    *qcode_lag = 0;
    *qcode_frac = 0;
  }
  else if(*lag > 16 && *frac == 0 ){
    *qcode_lag= *lag - 16;
    *qcode_frac = 0;
  }
  else if( (*lag > 16 && *lag < MAXLAG) && *frac == 2 ){
    *qcode_frac= 1;
    *qcode_lag= *lag - 16;
  }
  else{
    fprintf(stderr,"quantize_lag: Illegal value for *lag = %d\n", *lag);
    exit(-4);
  }

    unquantize_lag(lag, qcode_lag, frac, qcode_frac);
}

void unquantize_b(
    float *q_b,
    int   *qcode_b,
    int   lag
)
{

  if(*qcode_b < 0 || *qcode_b > 7){
    fprintf(stderr,"unquantize_b: Illegal *qcode_b = %d\n", *qcode_b);
    exit(-2);
  }

  if(*qcode_b == 0){
    if(lag == 0)
      *q_b = 0.0;
    else
      *q_b = 0.25;
  }
  else{
    *q_b = 0.25*(*qcode_b + 1);
  }

}

void quantize_b(
    float unq_b,
    float *q_b,
    int   *qcode_b
)
{
  float tmp;
  int lag;

  tmp = unq_b/0.25;
  if(round_flt(tmp) <= 0){
    *qcode_b = 0;
    lag = 0;
  }
  else{
    lag = 1;
    if(tmp >= 8.0){
      *qcode_b = 7;
    }
    else
      *qcode_b = round_flt(tmp) - 1;
  }
  if(*qcode_b < 0){
    fprintf(stderr,"quantize_b: *qcode_b (%d) < 0\n", *qcode_b);
    exit(-3);
  }
  unquantize_b(q_b, qcode_b, lag);

}

void unquantize_G(
    int   mode,
    float *q_G,
    int   *qcode_G,
    int   *qcode_Gsign,
    float *lastG,
    int   cbsf,
    int   *G_pred
)
{
    int i;
    float pred;

    for (i=GPRED_ORDER-1; i>0; i--) {
	G_pred[i]=G_pred[i-1];
    }

    if (G_QUANT_TYPE[mode]==LINEAR) {
      if ((mode!=FULLRATE_VOICED)||((cbsf+1)%4 !=0)) {
	lin_unquant(q_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode], *qcode_G);
	for (i=GORDER-1; i>= 1; i--) {
	  lastG[i] = lastG[i-1];
	}
	lastG[0] = *q_G;
	G_pred[0] = (int) *q_G;

	*q_G= GA[((int) *q_G)+6];

	if (*qcode_Gsign==NEGATIVE) {
	  *q_G*= -1;
	}
      }
      else {
	pred=0.0;
	for (i=0; i<GORDER; i++) {
	  pred+=lastG[i]*GPRED_COEFF[i];
	}
	pred = (int) (pred/GORDER); /* truncation */

	/* limit the prediction value */
	if(pred > MAX_GPRED)
	  pred = MAX_GPRED;
	else if(pred < MIN_GPRED)
	  pred = MIN_GPRED;

	lin_unquant(q_G, RELMING, RELMAXG,
		    RELNUMBER_OF_G_LEVELS[mode], *qcode_G);
	*q_G+= pred;
	for (i=GORDER-1; i>= 1; i--) {
	  lastG[i] = lastG[i-1];
	}
	lastG[0] = *q_G;
	G_pred[0] = (int) *q_G;

	*q_G= GA[((int) *q_G)+6];

	if (*qcode_Gsign==NEGATIVE) {
	  *q_G*= -1;
	}
      }
    }
    if (G_QUANT_TYPE[mode]==DELTA) {
      pred=0;
      for (i=0; i<GORDER; i++) {
	pred+=lastG[i]*GPRED_COEFF[i];
      }

      lin_unquant(q_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode], *qcode_G);
      *q_G+=pred;

      for (i=GORDER-1; i>= 1; i--) {
	lastG[i] = lastG[i-1];
      }
      lastG[0] = *q_G;
      G_pred[0] = (int) *q_G;

      *q_G= GA[((int) *q_G)+6];

      if (*qcode_Gsign==NEGATIVE) {
	*q_G*= -1;
      }
    }
}/* end of unquantize_G() */

void quantize_G(
    int   mode,
    float unq_G,
    float *q_G,
    int   *qcode_G,
    int   *qcode_Gsign,
    float *lastG, /* only needed for FULLRATE every fourth CB subframe */
    int   cbsf,
    int   *G_pred
)
{
    int i;
    float pred;

    if (G_QUANT_TYPE[mode]==LINEAR) {
      if ((mode!=FULLRATE_VOICED)||((cbsf+1)%4 !=0)) {
	if (unq_G>0.01) {
	  lin_quant(qcode_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode],
            20*log_10((double)unq_G) );

	  *qcode_Gsign=POSITIVE;
	}
	else if (unq_G< -0.01){
	  lin_quant(qcode_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode],
            20*log_10((double)(-unq_G)) );
	  *qcode_Gsign=NEGATIVE;
	}
	else {
	  lin_quant(qcode_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode],
            20*log_10((double)(0.01)) );
	  *qcode_Gsign=NEGATIVE;

	  if(mode == EIGHTH || mode == QUARTERRATE_UNVOICED){
	    *qcode_Gsign=POSITIVE;
	  }
	}
      }
      else {
	pred=0;
	for (i=0; i<GORDER; i++) {
	  pred+=lastG[i]*GPRED_COEFF[i];
	}
	pred = (int) (pred/GORDER); /* truncation */

	/* limit the prediction value */
	if(pred > MAX_GPRED)
	  pred = MAX_GPRED;
	else if(pred < MIN_GPRED)
	  pred = MIN_GPRED;

	if (unq_G>0.01) {
	  lin_quant(qcode_G, RELMING, RELMAXG,
		    RELNUMBER_OF_G_LEVELS[mode],
            ((int) 20*log_10((double)unq_G)) - pred  );
	  *qcode_Gsign=POSITIVE;
	}
	else if (unq_G< -0.01){
	  lin_quant(qcode_G, RELMING, RELMAXG,
		    RELNUMBER_OF_G_LEVELS[mode],
            ((int) 20*log_10((double)(-unq_G)))- pred  );
	  *qcode_Gsign=NEGATIVE;
	}
	else {
	  lin_quant(qcode_G, RELMING, RELMAXG,
		    RELNUMBER_OF_G_LEVELS[mode],
            ((int) 20*log_10((double)(0.01))) - pred  );
	  *qcode_Gsign=NEGATIVE;
	}
      }
    }
    if (G_QUANT_TYPE[mode]==DELTA) {
      pred=0;
      for (i=0; i<GORDER; i++) {
	pred+=lastG[i]*GPRED_COEFF[i];
      }
      if (unq_G>0) {
	lin_quant(qcode_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode],
          20*log_10((double)unq_G)-pred );
	*qcode_Gsign=POSITIVE;
      }
      else {
	lin_quant(qcode_G, MING[mode], MAXG[mode], NUMBER_OF_G_LEVELS[mode],
          20*log_10((double)(-unq_G))-pred );
	*qcode_Gsign=NEGATIVE;
      }
    }

    unquantize_G(mode, q_G, qcode_G, qcode_Gsign, lastG, cbsf, G_pred);

}/* quantize_G() */

void unquantize_G_8th(
    float     *q_G,          /* reconstructed G value */
    INTTYPE   *qcode_G,      /* quantization code for Gain */
    int       *G_pred        /* predictor */
)
{
    INTTYPE   i;
    float pred;
    INTTYPE   ind, G_dB;

    if(*qcode_G < 0 || *qcode_G > 3){
      fprintf(stderr,"Illegal qcode_G: %d\n", *qcode_G);
      exit(-4);
    }

    pred=0;
    for (i=0; i<GPRED_ORDER; i++) {
	pred+=G_pred[i];
    }

    pred/=(float)(GPRED_ORDER);

    G_dB=round_flt(pred);

    G_dB -= 1;
    if(G_dB < 4)
      G_dB = 4;
    else if(G_dB > 58)
      G_dB = 58;

    for (i=GPRED_ORDER-1; i>0; i--) {
	G_pred[i]=G_pred[i-1];
    }
    G_pred[0] = G_dB+QG8[*qcode_G];

    ind = G_dB+6;  /* 6 is offset required for GA[] */
    *q_G= GA[ind+QG8[*qcode_G]];

}/* end of unquantize_G_8th() */


void quantize_G_8th(
    float     unq_G,            /* input Gain value before quantization  */
    float     *q_G,             /* recontructed CB Gain */
    INTTYPE   *qcode_G,         /* quant. code for CB Gain */
    int       *G_pred           /* prediction for CB Gain */
)
{
    INTTYPE   i;
    INTTYPE   ind, G_dB;
    float pred;
    float G;
    float min_error, err;

    pred=0;
    for (i=0; i<GPRED_ORDER; i++) {
	pred+=G_pred[i];
    }
    pred/=(float)(GPRED_ORDER);

    G_dB=round_flt(pred);

    G_dB -= 1;
    if(G_dB < 4)
      G_dB = 4;
    else if(G_dB > 58)
      G_dB= 58;
    ind = G_dB + 6;
    G=unq_G;

    min_error=1E20; /* SAB: changed from 1000000; */
    *qcode_G=0;
    for (i=0; i<4; i++) {
	err=(G-GA[ind+QG8[i]])*(G-GA[ind+QG8[i]]);
	if (min_error>err) {
	    min_error=err;
	    *qcode_G=i;
	}
    }
    unquantize_G_8th(q_G, qcode_G, G_pred);

}/* end of quantize_G_8th() */


int round_flt(float x)
{
    INTTYPE  i;

    i=(INTTYPE)x;
    if( x>0 )
    {
      if( (x-((float)i))>=0.5 ) ++i;
    }
    else
    {
      if( (((float)i)-x)>0.5 ) --i;
    }
    return(i);

}/* end of round_flt() */
