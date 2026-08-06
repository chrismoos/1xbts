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
/* target.c - create and update the "desired" waveform for the searches */

#include <stdio.h>
#include "celp.h"

void create_target_speech(
    int    mode,
    float  *input,
    float  *resid,
    float  *target,
    float  lpc[2][MAXSF][2][LPCORDER],
    struct ZERO_FILTER *err_filt,
    struct POLE_FILTER *wght_filt
)
{
    int   i,j;
    int   sf_ptr;

    sf_ptr=0;
    for (i=0; i<PITCHSF[mode]; i++)
    {
	for (j=0; j<LPCORDER; j++) {
	    err_filt->zero_coeff[j]= -lpc[PITCH][i][NOT_WGHTED][j];
	    wght_filt->pole_coeff[j]=lpc[PITCH][i][WGHTED][j];
	}
/*
for (j=0; j<LPCORDER; j++)
 fprintf(stderr,"%8g ",err_filt->zero_coeff[j]);
fprintf(stderr,"\n");
for (j=0; j<LPCORDER; j++)
 fprintf(stderr,"%9g ",wght_filt->pole_coeff[j]);
fprintf(stderr,"\n");
fprintf(stdout,"sf_ptr = %3d num = %2d order = %d\n",
 sf_ptr,FSIZE/PITCHSF[mode],err_filt->order);
*/
        do_zero_filter(&input[sf_ptr], &resid[sf_ptr], FSIZE/PITCHSF[mode],
		       err_filt, UPDATE);

	do_pole_filter(&resid[sf_ptr], &target[sf_ptr], FSIZE/PITCHSF[mode],
		       wght_filt, UPDATE);

	sf_ptr+=FSIZE/PITCHSF[mode];

    }
}



void update_target_pitch(
    int                mode,
    float              *pw_speech,
    float              *target,
    float              *lpc,
    float              *targ_ene,
    struct DECODER_MEM *d_mem
)
{
    int   i, j;
    float zir[FSIZE];

    for (i=0; i<LPCORDER; i++) {
	d_mem->wght_syn_filt.pole_coeff[i]= lpc[i];
    }

    get_zero_input_response_pole(zir, FSIZE/PITCHSF[mode],
				       &(d_mem->wght_syn_filt));

    for (i=0; i<CBSF[mode]/PITCHSF[mode]; i++) {
	targ_ene[i]=0;
	for (j=0; j<FSIZE/CBSF[mode]; j++) {
	    targ_ene[i]+=pw_speech[j+i*FSIZE/CBSF[mode]]*
	      pw_speech[j+i*FSIZE/CBSF[mode]];
	}
    }

    for (i=0; i<FSIZE/PITCHSF[mode]; i++) {
	target[i]=pw_speech[i]-zir[i];
    }

}

void update_target_cb(
    int                 mode,
    float               *pw_speech,
    float               *target,
    float               *lpc,
    struct PITCHPARAMS  *pitch_params,
    struct DECODER_MEM  *d_mem
)
{
    int   i;
    float zir_p[FSIZE], pitch_est[FSIZE];

    d_mem->pitch_filt.delay=pitch_params->lag;
    d_mem->pitch_filt.coeff=pitch_params->b;
    d_mem->pitch_filt.frac=pitch_params->frac;

    get_zero_input_response_pole_1_tap_interp(zir_p, FSIZE/CBSF[mode],
	   &(d_mem->pitch_filt));

    for (i=0; i<LPCORDER; i++) {
	d_mem->wght_syn_filt.pole_coeff[i]= lpc[i];
    }

    do_pole_filter(zir_p, pitch_est, FSIZE/CBSF[mode],
		   &(d_mem->wght_syn_filt), NO_UPDATE);

    for (i=0; i<FSIZE/CBSF[mode]; i++) {
	target[i]=pw_speech[i]-pitch_est[i];
    }

}/* end of update_target_cb() */

#if 0
void create_target_cdbk(
    float               *form_resid,
    struct POLE_FILTER  *filter;
    float               *lpc,
    float               *target;
    int                 length
)
{

/* called before the codebook search, it creates a target signal */
/* from the new formant residual: remember the formant residual */
/* has been updated after the pitch search by subtracting the   */
/* pitch filter outputs from the original residual              */
    float *tmpbuf;
    int i;


    tmpbuf=(float *)(calloc((unsigned)length+MEM_L, sizeof(float)));
    for (i=0; i<LPCORDER; i++) {
	filter->pole_coeff[i]= lpc[i];
    }

    for (i=0; i<length; i++) {
	tmpbuf[i]=form_resid[i];
    }
    for (i=length; i<length+MEM_L; i++) { /* allow target filter to extend */
	                                  /* beyond vector length          */
	tmpbuf[i]=0.0;
    }

    do_pole_filter(tmpbuf,target, length+MEM_L,
		   filter, NO_UPDATE);

    free((char *) tmpbuf);
}

void update_target_filter(
    float              *form_resid,
    float              *y,
    float              *lpc,
    struct POLE_FILTER *filter,
    float              *target,
    int                length
)
{
  /* updated form_resid by subtracting off codebook contribution*/
  /* target is the filtered form_resid*/

  int i;

    for (i=0; i<length; i++) {
	form_resid[i] -= y[i];
    }

    for (i=0; i<LPCORDER; i++) {
	filter->pole_coeff[i]= lpc[i];
    }

    do_pole_filter(form_resid,target,length,filter, UPDATE);

}
#endif


void update_form_resid_mems(
    float               *input,
    struct ZERO_FILTER  *err_filt
)
{
    INTTYPE   i;

    for (i=0; i<LPCORDER; i++) {
	err_filt->memory[i]= input[FSIZE-1-i];
    }
}
