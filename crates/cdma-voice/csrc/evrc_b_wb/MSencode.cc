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

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/


#include <stdio.h>
#include <math.h>
#include <stdlib.h>
#define DIMS 6

#define MAXFLOAT  (3.40282347e+38F)

void
return_M_Least (float *inp, int n_cols, float *codebook, int num_grp,
		float *weight, int M, short *least)
{
    float *distance, mindist1, *distmeas;
    int i, j, k;

    distance = (float *) calloc (1, num_grp * sizeof (float));




    mindist1 = MAXFLOAT;
    for (i = 0; i < num_grp; i++) {
	distance[i] = 0;
	for (j = 0; j < n_cols; j++) {
	    distance[i] =
		distance[i] +
		(weight[j] * (inp[j] - codebook[n_cols * i + j]) *
		 (inp[j] - codebook[n_cols * i + j]));
	}
	if (distance[i] < mindist1) {
	    mindist1 = distance[i];
	    least[0] = i;
	}
    }

    distance[least[0]] = MAXFLOAT;

    for (k = 1; k < M; k++) {
	mindist1 = MAXFLOAT;
	for (i = 0; i < num_grp; i++) {
	    if (distance[i] < mindist1) {
		mindist1 = distance[i];
		least[k] = i;
	    }
	}
	distance[least[k]] = MAXFLOAT;
    }

    free (distance);
}



void
singlevectortest (float *inp, int dimen, int cb_size, short *index,
		  float *weight, float *recon, float *codebook)
{
    int j, k, kval1, m = 1, n = 2;
    float ermin1, er;


    ermin1 = MAXFLOAT;
    for (k = 0; k < cb_size; k = k + 1) {
	er = 0;
	for (j = 0; j < dimen; j = j + 1) {
	    er = er +
		(weight[j] * (inp[j] - codebook[dimen * k + j]) *
		 (inp[j] - codebook[dimen * k + j]));

	}

	if (er < ermin1) {
	    ermin1 = er;
	    kval1 = k;
	}
    }
    index[0] = kval1;
    for (j = 0; j < dimen; j++) {
	recon[j] = codebook[dimen * index[0] + j];
    }


}
void
singlevectortest_gain (float *inp, int dimen, int cb_size, short *index,
		       float *weight, float *recon, float *codebook)
{
    int j, k, kval1, m = 1, n = 2, M = 4, flag;
    float ermin1, er, meanU, meanQ;
    short least[4];

    return_M_Least (inp, dimen, codebook, cb_size, weight, M, least);

    meanU = 0;
    for (j = 0; j < dimen; j++) {
	meanU += inp[j];
	recon[j] = codebook[dimen * least[0] + j];
    }

    index[0] = least[0];

    flag = 0;
    for (k = 0; k < M; k++) {
	if (flag == 0) {
	    meanQ = 0;
	    for (j = 0; j < dimen; j++)
		meanQ += codebook[dimen * least[k] + j];
	    if (meanQ <= 1.1 * meanU) {
		flag = 1;
		for (j = 0; j < dimen; j++)
		    recon[j] = codebook[dimen * least[k] + j];
		index[0] = least[k];
	    }


	}

    }



}



void
MSencode (float *inp, float *weight, short *index1, short *index2,
	  float *reconst, int n_cols, int num_grp1, int num_grp2, const int M,
	  float *codebook1, float *codebook2)
{

    int i, j, k;

    short *best_match_ind, index_d[1];

    float *Mdist, Mdist_min, *inp2, **inp1;

    FILE *find1, *find2;

    float *recon;

    recon = (float *) calloc (n_cols, sizeof (float));


    best_match_ind = (short *) calloc (1, num_grp1 * sizeof (int));
    inp2 = (float *) calloc (1, n_cols * sizeof (float));



    Mdist = (float *) calloc (1, M * sizeof (float));



    return_M_Least (inp, n_cols, codebook1, num_grp1, weight, M,
		    best_match_ind);

    Mdist_min = MAXFLOAT;

    for (k = 0; k < M; k++) {

	Mdist[k] = 0.0;

	for (j = 0; j < (n_cols); j++) {
	    inp2[j] = inp[j] - codebook1[n_cols * (best_match_ind[k]) + j];

	}

	singlevectortest (inp2, n_cols, num_grp2, &index_d[0], weight,
			  &(recon[0]), codebook2);

	for (j = 0; j < (n_cols); j++) {

	    Mdist[k] =
		Mdist[k] + (inp[j] -
			    codebook1[(n_cols * best_match_ind[k]) + j] -
			    recon[j])
		* (inp[j] - codebook1[(n_cols * best_match_ind[k]) + j] -
		   recon[j]) * weight[j];

	}



	if (Mdist[k] < Mdist_min) {
	    Mdist_min = Mdist[k];
	    *index1 = best_match_ind[k];
	    *index2 = index_d[0];
	}
    }
    for (j = 0; j < n_cols; j++) {
	reconst[j] =
	    codebook1[(n_cols * (*index1)) + j] +
	    codebook2[(n_cols * (*index2)) + j];
    }

    free (best_match_ind);
    free (Mdist);
    free (inp2);
    free (recon);

}

void
MSdecode (int index1, int index2, float *reconst, int n_cols,
	  float *codebook1, float *codebook2)
{
    int j;

    for (j = 0; j < n_cols; j++) {
	reconst[j] =
	    codebook1[(n_cols * (index1)) + j] +
	    codebook2[(n_cols * (index2)) + j];
    }
}

void
SingleStageDecode (int index, int n_cols, float *reconst, float *codebook)
{
    int j;

    for (j = 0; j < n_cols; j++) {
	reconst[j] = codebook[(n_cols * (index)) + j];
    }


}
