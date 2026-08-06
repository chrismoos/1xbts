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
/* lpc.c - Computes the Linear Preditive Coefficients using	*/
/*	Durbin's recursion.					*/

/* MODIFICATION: <malloc.h> -> <stdlib.h> for portability (macOS / BSD). */
#include <stdlib.h>
#include <math.h>
#include"celp.h"

void durbin(
    float *R,
    float *lpc,
    int   order
)
{
    int   i, j;

#if USE_CALLOC
    float *k, *E, *a;

    k  =(float *)(calloc((unsigned)order      , sizeof(float)));
    E  =(float *)(calloc((unsigned)order+1    , sizeof(float)));
    a  =(float *)(calloc((unsigned)order*order, sizeof(float)));
#else
    float   k[LPCORDER];
    float   E[LPCORDER+1];
    float   a[LPCORDER*LPCORDER];
#endif

    E[0]=R[0];

    if (R[0]<0.0001) {
	for (i=0; i<order; i++) {
	    k[i]=0;
	}
	for (i=0; i<order*order; i++) {
	    a[i]=0;
	}
	R[0] = 10.0;
	for (i=1; i<order; i++) {
	  R[i]=0.0;
	}
    }

    else {
	for (i=1; i<=order; i++) {
	    k[i-1]=0;
	    for (j=1; j<=i-1; j++) {
		k[i-1]+=a[(i-2)*order+(j-1)]*R[i-j];
	    }
	    k[i-1]= (R[i]-k[i-1])/E[i-1];

	    a[(i-1)*order+(i-1)]=k[i-1];
	    for (j=i-1; j>=1; j--) {
		a[(i-1)*order+(j-1)]=
		  a[(i-2)*order+(j-1)]
		    -k[i-1]*a[(i-2)*order+(i-j-1)];
	    }

	    E[i]=(1-k[i-1]*k[i-1])*E[i-1];
	}
    }

    for (i=0; i<order; i++) {
	lpc[i]=a[(order-1)*order+i];
    }

#if USE_CALLOC
    free((char*)a);
    free((char*)E);
    free((char*)k);
#endif

}

/* compute_autocorr - computes the "shift"th autocorrelation of	*/
/* 	"signal" of windowsize "length".			*/

float compute_autocorr(
    float   *signal,
    int     length,
    int     shift
)
{
	int	i;
	float	R;

	R=0;
	for (i=0; i<length-shift; i++) {
		R+= signal[i]*signal[i+shift];
	}
	return(R);
}

void compute_lpc(
    float            *speech,
    int              windowsize,
    int              windowtype,
    int              order,
    float            *lpc,
    float            *R,
    struct  CONTROL  *control
)
{
	int	i;

#if USE_CALLOC
	float	*wspeech;
	float   *lsp;

	wspeech=(float *)(calloc((unsigned)windowsize, sizeof(float)));

	lsp=(float *)(calloc((unsigned)order      , sizeof(float)));
#else
    float   wspeech[LPCSIZE];
    /*float   lsp[LPCORDER];*/
#endif

	/* Window the speech signal		*/
	switch (windowtype) {
	case HAMMING:
	    HAMMINGwindow(speech, wspeech, windowsize);
	    break;
	default:
	    printf("Unknown window type in lpc.c\n");
	    usage(control);
	}

	for (i=0; i<=FILTERORDER; i++) {
		R[i]=compute_autocorr(wspeech, windowsize, i);
	}

	durbin(R, lpc, order);

#if USE_CALLOC
	free((char*)lsp);
	free((char*)wspeech);
#endif

}

void interp_lpcs(
    int              mode,
    float            *prev_lsp,
    float            *curr_lsp,
    float            lpc[2][MAXSF][2][LPCORDER],
    int              type,
    struct  CONTROL  *control
)
{
    int   i,j, pitchsf;
    float wght_factor;
    float tmp_lsp[LPCORDER];
    float current_center;
    float interp_factor;

    pitchsf = PITCHSF[mode];

    for(i=0; i< pitchsf; i++) {
	current_center=(float)(i+0.5)*(float)(FSIZE/pitchsf);
	interp_factor= current_center+(float)(FSIZE)-(float)(LPC_CENTER);
	interp_factor/=(float)(FSIZE);
	if (interp_factor>1.0) interp_factor=1.0;

	for (j=0; j<LPCORDER; j++) {
	    tmp_lsp[j]=curr_lsp[j]*interp_factor
	      +prev_lsp[j]*(1-interp_factor);
	}
	lsp2lpc(tmp_lsp, lpc[PITCH][i][NOT_WGHTED], LPCORDER);

	wght_factor=1;
	for (j=0; j<LPCORDER; j++) {
	    wght_factor*=BWE_FACTOR;
	    lpc[PITCH][i][NOT_WGHTED][j]*=wght_factor;
	}

	if (type==BOTH) {
	    wght_factor=1;
	    for (j=0; j<LPCORDER; j++) {
		wght_factor*=control->per_wght;
		lpc[PITCH][i][WGHTED][j]
		  =lpc[PITCH][i][NOT_WGHTED][j]*wght_factor;
	    }
	}
      }
}

/*k_to_lpc(k, lpc, order)
float *k, *lpc;
int   order;
{
    int   i,j;
    float *a;

    a  =(float *)(calloc((unsigned)order*order, sizeof(float)));

    for (i=0; i<order; i++) {

	a[i*order+i]=k[i];
	for (j=0; j<i; j++) {
	    a[i*order+j]=
	      a[(i-1)*order+j]
		-k[i]*a[(i-1)*order+(i-j-1)];
	}
    }

    for (i=0; i<order; i++) {
	lpc[i]=a[(order-1)*order+i];
    }

    free((char*)a);

}*/



/* HAMMINGwindow - perform Hamming windowing    */

void HAMMINGwindow(
    float *input,
    float *output,
    int   length
)
{
    int i;
    extern const int HAMMING_TAB[];
/*
    for (i=0; i<length; i++) {
	output[i]=input[i]*(.54-.46*cos(2.0*(double)i*PI/(double)(length-1)));
    }
*/
    for (i=0; i<length/2; i++) {
        output[i]=input[i]*(float)HAMMING_TAB[i]/16384.0;
	output[length-1-i]=input[length-1-i]*(float)HAMMING_TAB[i]/16384.0;
    }
}
