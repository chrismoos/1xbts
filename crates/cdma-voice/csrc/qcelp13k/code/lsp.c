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
/* lsp.c - lsp to lpc and vice versa routines                         */


#include"math.h"
#include"celp.h"
#include"lsp.h"

void lpc2lsp(
    float *lpc,
    float *lsp,
#if PREV_LSP_FIX
    float *prev_lsp,
#endif
    int   order
)
{

    float p[LPCORDER+2],q[LPCORDER+2];

    lpc2pq(lpc,p,q,order);

#if PREV_LSP_FIX
    pq2lsp(p,q,lsp,prev_lsp,order);
#else
    pq2lsp(p,q,lsp,order);
#endif

}

void lsp2lpc(
    float *lsp,
    float *lpc,
    int   order
)
{

    float p[LPCORDER+2],q[LPCORDER+2];

    lsp2pq(lsp,p,q,order);
    pq2lpc(p,q,lpc,order);
}

void lpc2pq(
    float *lpc,
    float *p,
    float *q,
    int   order
)
{
    int i;

    for (i=1; i<=order/2; i++) {
	p[i]= -lpc[i-1] -lpc[order-i];
    }
    for (i=1; i<=order/2; i++) {
	q[i]= -lpc[i-1] +lpc[order-i];
    }

    p[0]= 1.0;
    q[0]= 1.0;

    for (i=1; i<=order/2; i++) {
	p[i]-= p[i-1];
    }
    for (i=1; i<=order/2; i++) {
	q[i]+= q[i-1];
    }
}

void pq2lpc(
    float *p,
    float *q,
    float *lpc,
    int   order
)
{
    int i;

    for(i=1; i<=order/2; i++) {
	lpc[i-1]= -(p[i]+q[i])/2.0;
	lpc[order-i]= -(p[i]-q[i])/2.0;
    }
}

void pq2lsp(
    float *p,
    float *q,
    float *lsp,
#if PREV_LSP_FIX
    float *prev_lsp,
#endif
    int   order
)
{
    int i,indx;
    float wp[LPCORDER+2];


    wp[0] = 0.0;			/* artificial root at 0 */
    indx = 1;
    for(i=0; i<NUM_LSP_BINS; i+=BINS_PER_SEARCH)
    {
        wp[indx] = rsearch(p,order/2,(float)i/(float)NUM_LSP_BINS,
			   (float)(i+BINS_PER_SEARCH)/(float)(NUM_LSP_BINS),
			   NUM_LSP_BINS);
        if(wp[indx] != NO_ROOT_FOUND) indx++;

#if PREV_LSP_FIX
        if( indx >= (order/2+2) )
        {
            break;
        }
#endif

    }

    wp[indx++]= 1.0;
    if(indx > (order/2+2))
    {
        fprintf(stderr,"\nToo many P roots\n");

#if PREV_LSP_FIX
        for( i=0 ; i < LPCORDER ; i++ )
        {
            lsp[i] = prev_lsp[i];
        }
        return;
#endif
    }
    if(indx < (order/2+2))
    {
        fprintf(stderr,"\nToo few P roots: indx = %d order = %d\n", indx, order);
#if PREV_LSP_FIX
        for( i=0 ; i < LPCORDER ; i++ )
        {
            lsp[i] = prev_lsp[i];
        }
        return;
#endif
    }

    /* search for roots of Q'(w) (m/2 real roots) */
    /* between the roots of P'(w) */
    indx = 0;
    for(i=1; indx<order;)
    {
        lsp[indx++] = wp[i++];  /* interleave the roots of P' and Q' */
        lsp[indx++] = rsearch(q,order/2, wp[i-1], wp[i], NUM_LSP_BINS);
        if(lsp[indx-1] == NO_ROOT_FOUND)
        {
            fprintf(stderr,"\nQ root missing\n");

#if PREV_LSP_FIX
            for( i=0 ; i < LPCORDER ; i++ )
            {
                lsp[i] = prev_lsp[i];
            }
            return;
#endif
        }
    }


    return;
}

void lsp2pq(
    float *lsp,
    float *p,
    float *q,
    int   order
)
{
    int   i;
    float long_seq[100];
    float short_seq[3];
    int   long_length, short_length;

    short_length=3;
    long_length=2;

    short_seq[0]=1;
    short_seq[2]=1;
    long_seq[0]=1;
    long_seq[1]=1;

    for (i=0; i<order; i+=2) {
	short_seq[1]= -2.0*cos(PI*lsp[i]);
	convolve(short_seq, short_length, long_seq, long_length,
		 long_seq, &long_length);
    }
    for (i=1; i<=order/2; i++) {
	p[i]=long_seq[i];
    }

    long_length=3;
    long_seq[0]= 1;
    long_seq[1]= -2.0*cos(PI*lsp[1]);
    long_seq[2]= 1;

    for (i=3; i<order; i+=2) {
	short_seq[1]= -2.0*cos(PI*lsp[i]);
	convolve_odd(short_seq, short_length, long_seq, long_length,
		 long_seq, &long_length);
    }

    for (i=1; i<=order/2; i++) {
	q[i]=long_seq[i]-long_seq[i-1];
    }
}


/* symmetrical convolution */
void convolve_odd(
    float *in1,
    int   l1,
    float *in2,
    int   l2,
    float *out,
    int   *lout
)
{
    int   i,j;
    float ret[100];

    for (i=0; (i<l1)&&(i<l2); i++) {
	ret[i]=0;
	for (j=0; j<=i; j++) {
	    ret[i]+=in1[j]*in2[i-j];
	}
    }

    for (i=l1; i<(l1+l2)/2; i++) {
	ret[i]=0;
	for (j=0; j<l1; j++) {
	    ret[i]+=in1[j]*in2[i-j];
	}
    }

    *lout=(l1+l2)-1;
    for (i=0; i<(*lout)/2; i++) {
	out[i]=ret[i];
	out[(*lout-1)-i]=ret[i];
    }
    out[*lout/2]=ret[*lout/2];

}

/* symmetrical convolution */
void convolve(
    float *in1,
    int   l1,
    float *in2,
    int   l2,
    float *out,
    int   *lout
)
{
    int   i,j;
    float ret[100];

    for (i=0; (i<l1)&&(i<l2); i++) {
	ret[i]=0;
	for (j=0; j<=i; j++) {
	    ret[i]+=in1[j]*in2[i-j];
	}
    }

    for (i=l1; i<(l1+l2)/2; i++) {
	ret[i]=0;
	for (j=0; j<l1; j++) {
	    ret[i]+=in1[j]*in2[i-j];
	}
    }

    *lout=(l1+l2)-1;
    for (i=0; i<(*lout)/2; i++) {
	out[i]=ret[i];
	out[(*lout-1)-i]=ret[i];
    }

}


float rsearch(
    float *r,
    int   sub_order,
    float lo,
    float hi,
    int   num_bins
)
{
    float slo,shi,smid;
    int i;
    float mid;

if( hi < lo )
{
    fprintf(stderr,"rsearch(): hi < lo: %f < %f\n",hi,lo);
}

    slo = lspdct(r,sub_order,lo);
    shi = lspdct(r,sub_order,hi);

    if(slo*shi > 0.0)
    {
        return(NO_ROOT_FOUND);    /* root does not exist */
    }
    if (slo==0.0)
    {
        return(lo);               /* got it this time */
    }
    if (shi==0.0)
    {
        return(NO_ROOT_FOUND); /* get it next time */
    }

    for(i=0; i<10; i++)
    {	/* iteration limit of 10 */
        mid = (lo+hi)/2.0;  /* divide midway between lo and hi */
        if((mid-lo)*num_bins<1)         /* end of iteration */
        {
            /* linearly interpolate for better estimation */

            return((float)lo + (fabs(slo)/fabs(shi-slo))*(hi-lo));
        }

        smid = lspdct(r,sub_order,mid); /* evaluate dct */
        if(smid == 0.0)
            return(mid);    /* rare case */
        /* pick the interval with a sign change */
        if(slo*smid < 0.0)
        {
            hi = mid; shi = smid;
        }
        if(smid*shi < 0.0)
        {
            lo = mid; slo = smid;
        }
    } /* end for() */

    return(lo);			/* should not reach this */
}

/* take dct of the LSP polynomial -- integer index to cos table */
float lspdct(
    float *r,
    int   order,
    float indx
)
{
    float f;
    int i;
    f = cos((double)(order*PI*indx));

    for(i=1; i<order; i++) f += r[i] * cos((double)(PI*(order-i)*indx));
    f += 0.5 * r[order];

    return(f);
}
