/**********************************************************************/
/* QCELP Variable Rate Speech Codec - Simulation of TIA IS96-A, service */
/*     option one for TIA IS95, North American Wideband CDMA Digital  */
/*     Cellular Telephony.                                            */
/*                                                                    */
/* (C) Copyright 1993, QUALCOMM Incorporated                          */
/* QUALCOMM Incorporated                                              */
/* 10555 Sorrento Valley Road                                         */
/* San Diego, CA 92121                                                */
/*                                                                    */
/* Note:  Reproduction and use of this software for the design and    */
/*     development of North American Wideband CDMA Digital            */
/*     Cellular Telephony Standards is authorized by                  */
/*     QUALCOMM Incorporated.  QUALCOMM Incorporated does not         */
/*     authorize the use of this software for any other purpose.      */
/*                                                                    */
/*     The availability of this software does not provide any license */
/*     by implication, estoppel, or otherwise under any patent rights */
/*     of QUALCOMM Incorporated or others covering any use of the     */
/*     contents herein.                                               */
/*                                                                    */
/*     Any copies of this software or derivative works must include   */
/*     this and all other proprietary notices.                        */
/**********************************************************************/
/* lsp.c - lsp routines */

#include"math.h"
#include"struct.h"

#define NUM_LSP_BINS    800
#define NO_ROOT_FOUND     0

lpc2lsp(lpc,lsp,order)
float *lpc,*lsp;
INTTYPE   order;
{
    INTTYPE i;
    float p[LPCORDER+2],q[LPCORDER+2];

    lpc2pq(lpc,p,q,order);
    pq2lsp(p,q,lsp,order);

}

lsp2lpc(lsp,lpc,order)
float *lsp,*lpc;
INTTYPE order;
{
    INTTYPE i;
    float p[LPCORDER+2],q[LPCORDER+2];

    lsp2pq(lsp,p,q,order);
    pq2lpc(p,q,lpc,order);
}

lpc2pq(lpc,p,q,order)
float *lpc,*p,*q;
INTTYPE   order;
{
    INTTYPE i;

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

pq2lpc(p,q,lpc,order)
float *p,*q,*lpc;
INTTYPE order;
{
    INTTYPE i;

    for(i=1; i<=order/2; i++) {
	lpc[i-1]= -(p[i]+q[i])/2.0;
	lpc[order-i]= -(p[i]-q[i])/2.0;
    }
}

pq2lsp(p,q,lsp,order)
float *p,*q,*lsp;
INTTYPE order;
{
    INTTYPE i,indx;
    float wp[LPCORDER+2],rsearch();

    wp[0] = 0.0;			/* artificial root at 0 */
    indx = 1;
    for(i=0; i<NUM_LSP_BINS; i++) {
	wp[indx] = rsearch(p,order/2,(float)i/(float)(2*NUM_LSP_BINS),
			   (float)(i+1)/(float)(2*NUM_LSP_BINS),
			   NUM_LSP_BINS);
	if(wp[indx] != NO_ROOT_FOUND) indx++;
    }

    wp[indx++]= 0.5;
    if(indx > (order/2+2)) {
	/* should never happen */
	printf("Error: too many P roots in LSP search\n");
    }
    if(indx < (order/2+2)) {
	/* should never happen */
	printf("Error: too few P roots in LSP search\n");
    }

    /* search for roots of Q'(w) (m/2 real roots) */
    /* between the roots of P'(w) */
    indx = 0;
    for(i=1; indx<order;) {
	lsp[indx++] = wp[i++];	/* interleave the roots of P' and Q' */
	lsp[indx++] = rsearch(q,order/2, wp[i-1], wp[i], NUM_LSP_BINS);
	if(lsp[indx-1] == NO_ROOT_FOUND) {
	    /* should never happen */
	    printf("Error: Q root missing in LSP search\n");
	}
    }

}

lsp2pq(lsp,p,q,order)
float *lsp,*p,*q;
INTTYPE   order;
{
    INTTYPE   i;
    float long_seq[100];
    float short_seq[3];
    INTTYPE   long_length, short_length;

    short_length=3;
    long_length=2;

    short_seq[0]=1;
    short_seq[2]=1;
    long_seq[0]=1;
    long_seq[1]=1;

    for (i=0; i<order; i+=2) {
	short_seq[1]= -2.0*cos(2.0*PI*lsp[i]);
	convolve(short_seq, short_length, long_seq, long_length,
		 long_seq, &long_length);
    }
    for (i=1; i<=order/2; i++) {
	p[i]=long_seq[i];
    }

    long_length=3;
    long_seq[0]= 1;
    long_seq[1]= -2.0*cos(2.0*PI*lsp[1]);
    long_seq[2]= 1;

    for (i=3; i<order; i+=2) {
	short_seq[1]= -2.0*cos(2.0*PI*lsp[i]);
	convolve_odd(short_seq, short_length, long_seq, long_length,
		 long_seq, &long_length);
    }

    for (i=1; i<=order/2; i++) {
	q[i]=long_seq[i]-long_seq[i-1];
    }
}


/* symmetric convolution */
convolve_odd(in1, l1, in2, l2, out, lout)
float *in1, *in2, *out;
INTTYPE   l1, l2, *lout;
{
    INTTYPE   i,j;
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

/* symmetric convolution */
convolve(in1, l1, in2, l2, out, lout)
float *in1, *in2, *out;
INTTYPE   l1, l2, *lout;
{
    INTTYPE   i,j;
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


float rsearch(r,sub_order,lo,hi,num_bins)
float *r;
float lo,hi;
INTTYPE sub_order,num_bins;
{
    float slo, shi, smid, lsp_poly_eval();
    INTTYPE i;
    float mid;

    slo = lsp_poly_eval(r,sub_order,lo,num_bins);
    shi = lsp_poly_eval(r,sub_order,hi,num_bins);

    if (shi==0) {            /* if root EXACTLY at shi, return the value */
	return(hi);
    }
    if (slo==0) {            /* if root EXACTLY at slo, return no root,  */
	return(NO_ROOT_FOUND); /* since this root was found last time... */
    }

    if (slo*shi > 0.0) {
	return(NO_ROOT_FOUND);                  /* root does not exist   */
    }

    for(i=0; i<10; i++) {	                /* iteration limit of 10 */
	mid = (lo+hi)/2.0;	      /* divide midway between lo and hi */
	if((mid-lo)*num_bins<1) {		/* end of iteration      */
	                   /* linearly interpolate for better estimation */
	    return((float)lo + (fabs(slo)/fabs(shi-slo))*(hi-lo));
	}

	smid = lsp_poly_eval(r,sub_order,mid,num_bins);
	if(smid == 0.0) return(mid);	                    /* rare case */
                                 /* pick the interval with a sign change */
	if(slo*smid < 0.0) {hi = mid; shi = smid;}
	if(smid*shi < 0.0) {lo = mid; slo = smid;}
    }

    return(lo);			                /* should not reach this */
}

/* evaluate the LSP "polynomial" - not really a polynomial, since */
/* we're using the expanded cosine form   -- integer index to cos */
float lsp_poly_eval(r,order,indx,num_bins)
float *r;
float indx;
INTTYPE order,num_bins;
{
    INTTYPE i;
    float f;

    f = cos((double)(order*2.0*PI*indx));
    for (i=1; i<order; i++) {
	f+= r[i] * cos((double)(2.0*PI*(order-i)*indx));
    }
    f+= 0.5 * r[order];

    return(f);
}
