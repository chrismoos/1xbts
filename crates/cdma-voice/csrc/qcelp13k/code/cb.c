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
/* cb.c - perform closed-loop codebook search                         */


#include <math.h>
#include "celp.h"
#include "cb.h"

void compute_cb(
     int                    mode,
     float                  *target,
     struct ENCODER_MEM     *e_mem,
     float                  *lpc,
     struct CBPARAMS        cb_params[MAXCBPERPIT][MAXNUMCB],
     int                    cbsf
)
{
  int   i,j;
  int   cbnum;
  float lpc_ir[LENGTH_OF_IMPULSE_RESPONSE];
  float cb_out[3*FSIZE];
  float *cb_out_shifted;
  float G, Exy, Eyy, err, min_err;
  float Eyyopt[MAXNUMCB];
  float y[FSIZE];
  float subtarget[FSIZE];

  if(mode == QUARTERRATE_UNVOICED){
    fprintf(stderr,"compute_cb(): Illegal mode = %d\n", mode);
    exit(-4);
  }

  for (i=0; i<FSIZE/CBSF[mode]; i++) {
    subtarget[i]=target[i];
  }

  for (i=0; i<LPCORDER; i++) {
    e_mem->dec.wght_syn_filt.pole_coeff[i]=lpc[i];
  }

  /* get LPC impulse response */
  get_impulse_response_pole(lpc_ir, LENGTH_OF_IMPULSE_RESPONSE,
			    &(e_mem->dec.wght_syn_filt));


  for (cbnum=0; cbnum<NUMCB[mode]; cbnum++) {

    if (mode == FULLRATE_VOICED ) { /* Normal search */
      min_err=1000000000;

      /* construct c(n) */
      cb_out_shifted= &cb_out[CBLENGTH];

      for (i= -CBLENGTH+1; i<FSIZE/CBSF[mode]; i++) {
	cb_out_shifted[i] = CODEBOOK[(i-1+CBLENGTH)%CBLENGTH];
      }                             /* complete c(n) in memory */

      initial_recursive_conv(&cb_out_shifted[1],
			     FSIZE/CBSF[mode] -1, lpc_ir);

      for (i=0; i<CBLENGTH; i++) {
	recursive_conv(&cb_out_shifted[-i], lpc_ir,
		       FSIZE/CBSF[mode]);
	Exy=0;
	Eyy=0;
	for (j=0; j<FSIZE/CBSF[mode]; j++) {
	  Exy+=subtarget[j]*cb_out_shifted[-i+j];
	  Eyy+=cb_out_shifted[-i+j]*cb_out_shifted[-i+j];
	}
	if ((fabs(Exy)<.01)||(Eyy<.01)||(fabs(Exy/Eyy)<.01)) {
	  Exy=.1;
	  Eyy=1;
	}

	G=Exy/Eyy;

	err= -2*G*Exy+G*G*Eyy;
	if (err<min_err) {
	  min_err=err;
	  cb_params[cbsf][cbnum].G=G;
	  cb_params[cbsf][cbnum].i=(i+1)%CBLENGTH;
	  Eyyopt[cbnum]=Eyy;
	}
      }
      quantize_G(mode, cb_params[cbsf][cbnum].G,
		 &(cb_params[cbsf][cbnum].G),
		 &(cb_params[cbsf][cbnum].qcode_G),
		 &(cb_params[cbsf][cbnum].qcode_Gsign),
		 e_mem->dec.lastG, cbsf, e_mem->dec.G_pred);

      quantize_i(&(cb_params[cbsf][cbnum].i),
		 &(cb_params[cbsf][cbnum].qcode_i));
      for (j=0; j<FSIZE/CBSF[mode]; j++) {
	y[j]=CODEBOOK[(CBLENGTH-cb_params[cbsf][cbnum].i+j)%CBLENGTH]
	  *cb_params[cbsf][cbnum].G;
      }
    }
    else if (mode == HALFRATE_VOICED) { /* Normal search */
      min_err=1000000000;

      /* construct c(n) */
      cb_out_shifted= &cb_out[CBLENGTH];

      for (i= -CBLENGTH+1; i<FSIZE/CBSF[mode]; i++) {
	cb_out_shifted[i] = CODEBOOK_HALF[(i-1+CBLENGTH)%CBLENGTH];
      }	/* complete c(n) in memory */

      initial_recursive_conv(&cb_out_shifted[1],
			     FSIZE/CBSF[mode] -1, lpc_ir);

      for (i=0; i<CBLENGTH; i++) {
	recursive_conv(&cb_out_shifted[-i], lpc_ir,
		       FSIZE/CBSF[mode]);
	Exy=0;
	Eyy=0;
	for (j=0; j<FSIZE/CBSF[mode]; j++) {
	  Exy+=subtarget[j]*cb_out_shifted[-i+j];
	  Eyy+=cb_out_shifted[-i+j]*cb_out_shifted[-i+j];
	}
	if ((fabs(Exy)<.01)||(Eyy<.01)||(fabs(Exy/Eyy)<.01)) {
	  Exy=.1;
	  Eyy=1;
	}

	G=Exy/Eyy;

	err= -2*G*Exy+G*G*Eyy;
	if (err<min_err) {
	  min_err=err;
	  cb_params[cbsf][cbnum].G=G;
	  cb_params[cbsf][cbnum].i=(i+1)%CBLENGTH;

	  Eyyopt[cbnum]=Eyy;
	}
      }
      quantize_G(mode, cb_params[cbsf][cbnum].G,
		 &(cb_params[cbsf][cbnum].G),
		 &(cb_params[cbsf][cbnum].qcode_G),
		 &(cb_params[cbsf][cbnum].qcode_Gsign),
		 e_mem->dec.lastG, cbsf, e_mem->dec.G_pred);

      quantize_i(&(cb_params[cbsf][cbnum].i),
		 &(cb_params[cbsf][cbnum].qcode_i));
      for (j=0; j<FSIZE/CBSF[mode]; j++) {
	y[j]=CODEBOOK_HALF[(CBLENGTH-cb_params[cbsf][cbnum].i+j)%CBLENGTH]
	  *cb_params[cbsf][cbnum].G;
      }
    }
    else if (mode == EIGHTH){
      fprintf(stderr, "compute_cb: Does not support EIGHTH rate\n");
      exit(-2);
    }

    initial_recursive_conv(y, FSIZE/CBSF[mode], lpc_ir);

    for (j=0; j<FSIZE/CBSF[mode]; j++) {
      subtarget[j]-=y[j];
    }
  }

  if (mode!=QUARTERRATE_UNVOICED) {
    for (j=0; j<(CBSF[mode]/PITCHSF[mode]-cbsf)*FSIZE/CBSF[mode]; j++) {
      y[j]=0;
    }

    for (cbnum=0; cbnum<NUMCB[mode]; cbnum++) {

      if(mode == FULLRATE_VOICED){
	for (j=0; j<FSIZE/CBSF[mode]; j++) {
	  y[j]+=CODEBOOK[(CBLENGTH-cb_params[cbsf][cbnum].i+j)%CBLENGTH]*
	    cb_params[cbsf][cbnum].G;
	}
      }
      else{
	for (j=0; j<FSIZE/CBSF[mode]; j++) {
	  y[j]+=CODEBOOK_HALF[(CBLENGTH-cb_params[cbsf][cbnum].i+j)%CBLENGTH]*
	    cb_params[cbsf][cbnum].G;
	}
      }
    }
  }

}/* end of compute_cb() */


/***********************************************************************
*
* compute_cb_gain()
*
************************************************************************/
/* For QUARTERRATE_UNVOICED */

void compute_cb_gain(
     int                    mode,
     struct ENCODER_MEM     *e_mem,
     struct CBPARAMS        cb_params[MAXCBPERPIT][MAXNUMCB],
     float                  *speech,
     int                    cbsf
)
{
  int   j;
  int   cbnum;
  float Eyy;
  float factor;

  if(mode != QUARTERRATE_UNVOICED){
    fprintf(stderr,"compute_cb_gain(): Illegal mode = %d\n", mode);
    exit(-4);
  }

  for (cbnum=0; cbnum<NUMCB[mode]; cbnum++) {
    Eyy=0;

    /* get the gain from the speech itself and not the prediction residual */
    /* and scale it down by the prediction gain from the entire frame      */

    for (j=0; j<FSIZE/CBSF[mode]; j++) {
      Eyy+=speech[j]*speech[j];
    }

    factor = pow(10.,(double)(-.1*e_mem->features.pred_gain));
    Eyy *= factor;

    Eyy/=(float)(FSIZE/CBSF[mode]);

    Eyy=G_FACTOR*sqrt(Eyy);

    quantize_G(mode, Eyy, &(cb_params[cbsf][cbnum].G),
	       &(cb_params[cbsf][cbnum].qcode_G),
	       &(cb_params[cbsf][cbnum].qcode_Gsign),
	       e_mem->dec.lastG, cbsf, e_mem->dec.G_pred);
    }

}/* end of compute_cb_gain() */
