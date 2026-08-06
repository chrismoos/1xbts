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
/* pitch.c - perform pitch functions, including open loop pitch */
/*    estimation, and closed loop pitch search                  */

#include <math.h>
#include "celp.h"

#define FR_LAG_SEGMENTS             3
/* MODIFICATION: const-qualified (read-only tables) */
static const int FRL_RANGE[FR_LAG_SEGMENTS+1]= {MINLAG, 45, 85, MAXLAG+1};
static const int FRL_RES[FR_LAG_SEGMENTS]    = {2, 2, 2};

/* For CELP+ Fractional Pitch */
static const int FRAC[NUMMODES] = {NO,NO,NO,YES,YES};  /* make fractional pitch */


void set_lag_range(
    struct PITCHPARAMS  *pitch_params,
    int                 *qcode_min_lag
)
{
    pitch_params->min_lag=MINLAG;
    pitch_params->max_lag=MAXLAG;
    quantize_min_lag(pitch_params->min_lag, qcode_min_lag);
}

float small_sinc(float val)
{
    float sval;

    if (val==0) {
	return(1.0);
    }
    sval=sin(3.1415927*val)/(3.1415927*val);
    sval*=(0.5+0.5*cos(3.1415927*val/2.0));
    return(sval);
}

void compute_pitch(
    int                   mode,
    float                 *target,
    struct CONTROL        *control,
    struct ENCODER_MEM    *e_mem,
    float                 *lpc,
    struct PITCHPARAMS    *pitch_params
)
{
  int   i,j,l;
  int   qcode_b;
  float lpc_ir[LENGTH_OF_IMPULSE_RESPONSE];
  float pitch_out[MAXLAG+FSIZE+FR_INTERP_FILTER_LENGTH];
  float *pitch_out_shifted;
  float delta;
  float Exy[MAX_FR_RESOLUTION*(MAXLAG+2)+1], Eyy[MAX_FR_RESOLUTION*(MAXLAG+2)+1];
  float err[MAX_FR_RESOLUTION*(MAXLAG+2)+1], min_err;
  float y[FSIZE];
  float Exx;
  int   seg;

  if(mode != QUARTERRATE_UNVOICED && mode != EIGHTH){
    /* do pitch search unless in 1/4 or 1/8 rate mode */

    for (i=0; i<MAX_FR_RESOLUTION*(MAXLAG+2); i++) {
      Exy[i]=0;
    }

    Exx=0;
    for (i=0; i<FSIZE/PITCHSF[mode]; i++) {
      Exx+=target[i]*target[i];
    }

    min_err=1.5*Exx;

    for (i=0; i<LPCORDER; i++) {
	e_mem->dec.wght_syn_filt.pole_coeff[i]=lpc[i];
    }
                                  /* get LPC impulse response */
    get_impulse_response_pole(lpc_ir, LENGTH_OF_IMPULSE_RESPONSE,
			      &(e_mem->dec.wght_syn_filt));

    pitch_out_shifted= &pitch_out[MAXLAG+2];

                                  /* construct p(n) */
    for (i =pitch_params->min_lag;
	 i<=pitch_params->max_lag+2; i++) {
	pitch_out_shifted[-i] = e_mem->dec.pitch_filt.memory[i-1];
    }
    for (i =pitch_params->min_lag-1;
	 (i>=pitch_params->min_lag- FSIZE/PITCHSF[mode]+1)&&(i>0); i--) {
	pitch_out_shifted[-i] = e_mem->dec.pitch_filt.memory[i-1];
    }

    for (i =0; i<FSIZE/PITCHSF[mode]; i++) {
	pitch_out_shifted[i]=0;
    }                             /* complete p(n) in memory */

    pitch_params->lag=0;

    initial_recursive_conv(&pitch_out_shifted[3-pitch_params->min_lag],
			   FSIZE/PITCHSF[mode]-1, lpc_ir);

    for (i=pitch_params->min_lag-2; i<=pitch_params->max_lag+2; i++) {
      recursive_conv(&pitch_out_shifted[-i], lpc_ir,
		     FSIZE/PITCHSF[mode]);

      Exy[MAX_FR_RESOLUTION*i]=0;
      Eyy[MAX_FR_RESOLUTION*i]=0;

      if (i>=FSIZE/PITCHSF[mode]) {
	for (j=0; j<FSIZE/PITCHSF[mode]; j++) {
	  Exy[MAX_FR_RESOLUTION*i]+=target[j]*pitch_out_shifted[-i+j];
	  Eyy[MAX_FR_RESOLUTION*i]+=
	    pitch_out_shifted[-i+j]*pitch_out_shifted[-i+j];
	}
      }
      else {
	for (j=0; j<FSIZE/PITCHSF[mode]; j++) {
	  y[j]=pitch_out_shifted[-i+j];
	}
	for (j=i; j<FSIZE/PITCHSF[mode]; j++) {
	  y[j]+=pitch_out_shifted[-2*i+j];
	}
	for (j=0; j<FSIZE/PITCHSF[mode]; j++) {
	  Exy[MAX_FR_RESOLUTION*i]+=target[j]*y[j];
	  Eyy[MAX_FR_RESOLUTION*i]+=y[j]*y[j];
	}
      }

      if (Eyy[MAX_FR_RESOLUTION*i]>1) {
	err[MAX_FR_RESOLUTION*i]=Exx+
	  -Exy[MAX_FR_RESOLUTION*i]*Exy[MAX_FR_RESOLUTION*i]/
	    Eyy[MAX_FR_RESOLUTION*i];
      }
      else {
	err[MAX_FR_RESOLUTION*i]=Exx;
      }

      if ((err[MAX_FR_RESOLUTION*i] < min_err) && (Exy[MAX_FR_RESOLUTION*i] > 0) &&
	  (i >= pitch_params->min_lag) && (i <= pitch_params->max_lag)) {
	min_err=err[MAX_FR_RESOLUTION*i];
	pitch_params->lag=i;
	pitch_params->frac=0;
      }
    }

    if(FRAC[mode] == YES && control->fractional_pitch){
      for (seg=0; seg<FR_LAG_SEGMENTS; seg++) {
	if (FRL_RES[seg]!=1) {
	  for (i=FRL_RANGE[seg]; i<FRL_RANGE[seg+1]; i++) {
	    for (delta= 0.0; delta<=0.5; delta+=1.0/(float)(FRL_RES[seg])){
	      j=delta*MAX_FR_RESOLUTION;
	      if (delta<0) {
		Exy[i*MAX_FR_RESOLUTION+j]=0;
		for (l=-2; l<2; l++) {
		  Exy[i*MAX_FR_RESOLUTION+j]+=
		    small_sinc((float)(l-delta))*
		      Exy[(i+l)*MAX_FR_RESOLUTION];
		}
		Eyy[i*MAX_FR_RESOLUTION+j]=
		  (1+delta)*Eyy[i*MAX_FR_RESOLUTION]
		    -delta*Eyy[(i-1)*MAX_FR_RESOLUTION];
	      }
	      else if (delta>0) {
		Exy[i*MAX_FR_RESOLUTION+j]=0;
		for (l=-2; l<2; l++) {
		  Exy[i*MAX_FR_RESOLUTION+j]+=
		    small_sinc((float)(-l-delta))*
		      Exy[(i-l)*MAX_FR_RESOLUTION];
		}
		Eyy[i*MAX_FR_RESOLUTION+j]=
		  (1-delta)*Eyy[i*MAX_FR_RESOLUTION]
		    +delta*Eyy[(i+1)*MAX_FR_RESOLUTION];
	      }

	      if (Eyy[MAX_FR_RESOLUTION*i+j]>1) {
		err[MAX_FR_RESOLUTION*i+j]=Exx
		  -Exy[MAX_FR_RESOLUTION*i+j]
		    *Exy[MAX_FR_RESOLUTION*i+j]/
		      Eyy[MAX_FR_RESOLUTION*i+j];
	      }
	      else {
		err[MAX_FR_RESOLUTION*i+j]=Exx;
	      }

	      if ((err[MAX_FR_RESOLUTION*i+j]<min_err)&&
		  (Exy[MAX_FR_RESOLUTION*i+j]>0)){
		if(i < MAXLAG || j != 2){/* must be a legal lag value */
		  if(i < 140 || j != 2){
		    min_err=err[MAX_FR_RESOLUTION*i+j];
		    pitch_params->lag=i;
		    pitch_params->frac=j;
		  }
		}
	      }
	    }
	  }
	}
      }

    }  /* end of fractional pitch search */

    /* BUG FIX - sab Protect against divide by zero */
    if( Eyy[MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac] < 0.000001 )
    {
#       if 0
            fprintf(stdout,"Bad condition in compute_pitch():\n");
            i = MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac;
            fprintf(stdout,"%3d: Exy = %g Eyy = %g\n",i,Exy[i],Eyy[i]);
#       endif

        pitch_params->lag = 0;
    }

    if (pitch_params->lag==0) {
      if(e_mem->frame_num > 0)
	fprintf(stderr,
		"compute_pitch: Warning no minimum found, lag = 0\n");

	pitch_params->frac=0;
	quantize_b(0.0, &(pitch_params->b), &qcode_b);
	pitch_params->qcode_b=qcode_b;

    }
    else {
#if 0
fprintf(stderr,"\nBefore quantize_b:");
fprintf(stderr,"  Exy = %g\n",
Exy[MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac]);
fprintf(stderr,"  Eyy = %g\n",
Eyy[MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac]);

for( i=0 ; i < MAX_FR_RESOLUTION*(MAXLAG+2)+1 ; i++ )
  fprintf(stdout,"%3d: Exy = %g Eyy = %g\n",i,Exy[i],Eyy[i]);
fflush(stdout);
i = MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac;
fprintf(stdout,"%3d: Exy = %g Eyy = %g\n",i,Exy[i],Eyy[i]);
#endif

      quantize_b(Exy[MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac]
		 /Eyy[MAX_FR_RESOLUTION*pitch_params->lag+pitch_params->frac],
		 &(pitch_params->b), &qcode_b);

      if(pitch_params->b == 0.0)
	pitch_params->lag = 0;
      pitch_params->qcode_b=qcode_b;
    }

    quantize_lag(&(pitch_params->lag), &(pitch_params->qcode_lag),
		 &(pitch_params->frac),
		 &(pitch_params->qcode_frac));

  }
  else
      /* must be a 1/4 or 1/8 rate frame, we want to disable the pitch loop */
    {
      pitch_params->lag=0;
      pitch_params->frac=0;
      quantize_b(0.0, &(pitch_params->b), &qcode_b);
      pitch_params->qcode_b=qcode_b;
      quantize_lag(&(pitch_params->lag), &(pitch_params->qcode_lag),
		   &(pitch_params->frac), &(pitch_params->qcode_frac));
     }
}

float hammsinc(float input)
{
    float output;

    output=sin(3.1415927*input/(float)(MAX_FR_RESOLUTION))
      /(3.1415927*input/(float)(MAX_FR_RESOLUTION));
    output*=.5+.46*cos(3.1415927*input/
		       (float)(MAX_FR_RESOLUTION*FR_INTERP_FILTER_LENGTH/2));

    return(output);
}

void initial_recursive_conv(
    float *resid,
    int   length,
    float *impulse_response
)
{
    int i;

    for (i= length-1; i>=0; i--) {
	recursive_conv(&resid[i], impulse_response,
		       length-i);
    }
}

void recursive_conv(
    float *resid,
    float *impulse_response,
    int   length
)
{
    int i;

    if (resid[0]==0) return;

    for (i=1; (i<length)&&(i<LENGTH_OF_IMPULSE_RESPONSE); i++) {
	resid[i]+=resid[0]*impulse_response[i];
    }
}
