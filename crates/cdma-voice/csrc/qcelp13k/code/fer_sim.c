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
/*============================================================================*/
/*  Lucent Network Wireless Systems EVRC Error Model (modified from Qualcomm    */
/*  CDMA frame-error model).                                                  */
/*----------------------------------------------------------------------------*/
/*============================================================================*/
/*         ..Includes.                                                        */
/*----------------------------------------------------------------------------*/
#include  <stdio.h>
#include  <string.h>
#include  <stdlib.h>
#include  <time.h>
#include  <math.h>

#include  "fer_sim.h"

/*============================================================================*/
/*         ..Globals.                                                         */
/*----------------------------------------------------------------------------*/

/* strings for rates */
char rate_char[NRATES_IN][5] =
{
  {'9','6','0','0','\0'},
  {'4','8','0','0','\0'},
  {'2','4','0','0','\0'},
  {'1','2','0','0','\0'}
};

char event_char[4][10];
char rate_out_char[6][25];

/* array of frame lengths */
int frame_length[5] = {lengthFull,lengthHalf,lengthQuarter,lengthEighth,lengthFull};

/* transition probability matrices */
double p[NSTATES][NRATES_IN][NRATES_OUT];

/* matrix of events corresponding to various transitions of p[] */
int event_matrix[NRATES_IN][NRATES_OUT] =
{
    {CORRECT,  RANDOM,  RANDOM,  RANDOM, ERASURE, MODIFIED, MODIFIED},
    { RANDOM, CORRECT,  RANDOM,  RANDOM, ERASURE,   RANDOM, MODIFIED},
    { RANDOM,  RANDOM, CORRECT,  RANDOM, ERASURE,   RANDOM, MODIFIED},
    { RANDOM,  RANDOM,  RANDOM, CORRECT, ERASURE,   RANDOM, MODIFIED}
};

/* state transition matrix  -- gives resultant state (good or bad) for
     each rate transition */
int state_matrix[NRATES_IN][NRATES_OUT] =
{
    {GOOD,  BAD,  BAD,  BAD,  BAD,  BAD,  BAD},
    {GOOD, GOOD,  BAD,  BAD,  BAD,  BAD,  BAD},
    {GOOD, GOOD, GOOD,  BAD,  BAD,  BAD,  BAD},
    {GOOD, GOOD,  BAD, GOOD,  BAD,  BAD,  BAD}
};

/* matrix of output rates corresponding to various transitions of p[] */
int rate_out_matrix[NRATES_IN][NRATES_OUT] =
{
       {FULL,    HALF, QUARTER,  EIGHTH,      -1,  FULL_ERRS,     FULL},
       {FULL,    HALF, QUARTER,  EIGHTH,      -1,  FULL_ERRS,     HALF},
       {FULL,    HALF, QUARTER,  EIGHTH,      -1,  FULL_ERRS,  QUARTER},
       {FULL,    HALF, QUARTER,  EIGHTH,      -1,  FULL_ERRS,   EIGHTH}
};



/* variables, arrays for collection of statistics for the log file */

/* rate statistics  */
double ave_rate_in;     /* average rate (bps) of encoded file */
int encoded_rates[NRATES_IN];  /* number of frames of each rate in encoded file */
int output_rates[NRATES_OUT];  /* number of transitions to the seven processed
                                   frame categories (Note: not equivalent to the output
                                   rate codes of which there are only six)  */

int history[NRATES_IN][NRATES_OUT];   /* record of transitions for log file */

int event_list[NEVENTS][5];    /* records frame index, input/output frame category,
                                  event, and error mask index (if applicable)  */

int nevents;        /* event counter */

int fer_sim_seed = 0;           /* random number generator initial seed */

int rate_in;        /* input rate code for array indexing */

int rate_out;       /* output rate code for array indexing */

int rate_code;      /* output rate for file */

int state=GOOD;     /* state variable for internal Markov process  */

int event;          /* switch position (correct, erasure, modified, random) */

int frame_cnt;      /* frame counter  */

/* file pointers */
FILE    *log_fp = stdout;
FILE    *trans_fp = NULL;
char    *trans_fname = NULL;

/*============================================================================*/
/*         ..Functions.                                                       */
/*----------------------------------------------------------------------------*/
/* unif. random number generator on [0,1) with resolution ~1e-10  */
double dran1(int*);


void  initialize_data()
{

	/* initialize character arrays */
	strcpy(event_char[0]," CORRECT");
	strcpy(event_char[1]," ERASURE");
	strcpy(event_char[2],"MODIFIED");
	strcpy(event_char[3],"  RANDOM");

	strcpy(rate_out_char[0],"   9600     ");
	strcpy(rate_out_char[1],"   4800     ");
	strcpy(rate_out_char[2],"   2400     ");
	strcpy(rate_out_char[3],"   1200     ");
	strcpy(rate_out_char[4],"9600 w/ errs");

}

int  prob_to_cum()
{
	int i,j,k;
	double x;

	/* form cumulative probabilities from density  */
	for(k=0;k<NSTATES;k++) {
#if 0
		fprintf(stderr,"\n");
#endif
		for(i=0;i<NRATES_IN;i++) {
			for(j=1;j<NRATES_OUT;j++) {
#if 0
				if (j == 1) {
					fprintf(stderr,"%10.7f",p[k][i][j-1]);
				}
				fprintf(stderr,"%10.7f",p[k][i][j]);
#endif
				p[k][i][j] += p[k][i][j-1];
#if 0
				if (j == (NRATES_OUT-1)) {
					fprintf(stderr,"%12.7f\n",p[k][i][j]);
				}
#endif
			}
		}
	}
	/* check probs sum to one (or close enough)  */
	for(k=0;k<NSTATES;k++) {
		for(i=0;i<NRATES_IN;i++) {
			x = p[k][i][NRATES_OUT-1]-1.0;
			x = fabs(x);
#if 0
			fprintf(stderr,"|1-sum| = %15.8e\n",x);
#endif
			if( x > 1e-5) {
				printf("Probabilities do not sum to one, rate in = %d, state = %d\n",i,k);
				fprintf(log_fp,"Probabilities do not sum to one, rate in = %d, state = %d\n",i,k);
				return(0);
			}
		}
	}
	return(1);

}

void  initialize_stats()
{
	int i,j;

	for(i=0;i<NRATES_IN;i++)
    {
		for(j=0;j<NRATES_OUT;j++)
            history[i][j]=0;
    }

	nevents = 0;
}


void  trans (int r)
{
	double x;
	int i,decision;

    x = dran1(&fer_sim_seed);     /* unif. random [0,1) */

	for(i=0;i<NRATES_OUT;i++) {
		if(x < p[state][r][i]) break;
	}
	decision = i;

	/* set codes */
	rate_out = rate_out_matrix[r][decision];
	event = event_matrix[r][decision];
	state = state_matrix[r][decision];

	history[r][decision]++;
}

#define M1 259200
#define IA1 7141
#define IC1 54773
#define M2 134456
#define IA2 8121
#define IC2 28411
#define M3 243000
#define IA3 4561
#define IC3 51349

double dran1(int* idum)
{
	static long ix1,ix2,ix3;
	static double r[98];
	double temp;
	static int iff=0;
	int j;

	if (*idum < 0 || iff == 0) {
		iff=1;
		ix1=(IC1-(*idum)) % M1;
		ix1=(IA1*ix1+IC1) % M1;
		ix2=ix1 % M2;
		ix1=(IA1*ix1+IC1) % M1;
		ix3=ix1 % M3;
		for (j=1;j<=97;j++) {
			ix1=(IA1*ix1+IC1) % M1;
			ix2=(IA2*ix2+IC2) % M2;
			r[j]=((double)ix1+(double)ix2/M2)/M1;
		}
		*idum=1;
	}
	ix1=(IA1*ix1+IC1) % M1;
	ix2=(IA2*ix2+IC2) % M2;
	ix3=(IA3*ix3+IC3) % M3;
	j=1 + ((97*ix3)/M3);
	temp=r[j];
	r[j]=((double)ix1+(double)ix2/M2)/M1;
	return temp;
}

#undef M1
#undef IA1
#undef IC1
#undef M2
#undef IA2
#undef IC2
#undef M3
#undef IA3
#undef IC3

/*===================================================================*/
/*  fer_sim()                                                        */
/*-------------------------------------------------------------------*/
void fer_sim(int *rate)
{
    int     i,j,k,ix;
    short   fer_flag;

    static short    first_time = 1;


    if( first_time )
    {
        first_time = 0;

        /*...initialize data...*/
        initialize_data();

        /*...set seed...*/
        if (fer_sim_seed == 0) {
            fer_sim_seed = (int)(time(NULL) & 0x0fff);
        }

        if ((trans_fp=fopen(trans_fname,"r")) == NULL)
        {
            fprintf(stderr,"ERROR:  Cannot open file \"%s\" for reading.\n",trans_fname );
            fprintf(log_fp,"ERROR:  Cannot open file \"%s\" for reading.\n",trans_fname );
            exit(1);
        }
        else
        {
            fprintf(log_fp,"FER Transition Prob. File              : \"%s\"\n\n",trans_fname);
        }

        /* record initial parameters in log file */
        fprintf(log_fp,"Random number generator initial seed : %d\n",fer_sim_seed);

        /* read transition probability matrices */
        for(i=0;i<NSTATES;i++)
        {
            for(j=0;j<NRATES_IN;j++)
            {
                for(k=0;k<NRATES_OUT;k++)
                {
                    ix = fscanf(trans_fp,"%lf",&p[i][j][k]);
                    if(ix!=1)
                    {
                        printf("Error in read of transition matrices file\n");
                        fprintf(log_fp,"Error in read of transition matrices file\n");
                        exit(1);
                    }
                }
            }
        }

        /* convert probability transition matrices to cumulative values */
        if(!prob_to_cum())
        {
            fprintf(stderr,"Problem with prob_to_cum()\n");
            fprintf(log_fp,"Problem with prob_to_cum()\n");
        }


        /* clear rate and transition stat matrices */
        initialize_stats();

    } /* end if( first_time ) */



    /* read encoded data */
    /* convert code to internal value */
    if(*rate==fullRate) rate_in = FULL;
    else if(*rate==halfRate) rate_in = HALF;
    else if(*rate==quarRate) rate_in = QUARTER;
    else if(*rate==eighRate) rate_in = EIGHTH;
    else if(*rate==fullProb) rate_in = FULL_ERRS;
    else rate_in = -1;   /* other codes invalid */

    fer_flag = 0;

    if (rate_in == -1)
    {
        fer_flag = 1;
    }
    else if (rate_in == -2)
    {
        fer_flag = 1;
        fprintf(stderr,"Encoded data file format error -- invalid rate specified in %d-th frame\n",frame_cnt);
        fprintf(log_fp,"Encoded data file format error -- invalid rate specified in %d-th frame\n",frame_cnt);
    }


    trans(rate_in);  /* determine rate_out, event from random number and  */
		                 /* transition probability matrix */

    /* Force RANDOM and MODIFIED to be an ERASURE (for simplicity) */
    if(event==RANDOM)           /* random data for the given frame length */
    {
        event = ERASURE;
    }
    else if(event==MODIFIED)    /* XOR data with a randomly selected mask */
    {
        event = ERASURE;
    }
    else if( fer_flag == 1 )
    {
        event = ERASURE;
    }


    /* determine output rate code */
    if(event==ERASURE) rate_code = erasRate;
    else if(rate_out==FULL) rate_code = fullRate;
    else if(rate_out==HALF) rate_code = halfRate;
    else if(rate_out==QUARTER) rate_code = quarRate;
    else if(rate_out==EIGHTH) rate_code = eighRate;
    else if(rate_out==FULL_ERRS) rate_code = fullProb;

    *rate = rate_code;

}

#if 0
void fer_stats();
{
    int     i;
    FILE    *log_fp;

    log_fp = stdout;


	/* collect and record statistics in log file */
     /* marginal distributions */
	for(i=0;i<NRATES_IN;i++) {
		encoded_rates[i] = 0;
		for(j=0;j<NRATES_OUT;j++) encoded_rates[i] += history[i][j];
	}
	for(i=0;i<NRATES_OUT;i++) {
		output_rates[i] = 0;
		for(j=0;j<NRATES_IN;j++) output_rates[i] += history[j][i];
	}


	fprintf(log_fp,"\nNumber of frames = %6d\n",frame_cnt);

	fprintf(log_fp,"\nNumber of frames of various rates at input\n");
	fprintf(log_fp,"Type    Number\n9600  %6d\n4800  %6d\n2400  %6d\n1200  %6d\n",
		encoded_rates[0],encoded_rates[1],encoded_rates[2],encoded_rates[3]);
	fprintf(log_fp,"\nNumber of various rate decisions at output\n");
	fprintf(log_fp,"Type                  Number\n9600  %20d\n4800  %20d\n2400  %20d\n1200  %20d\n",
		output_rates[0],output_rates[1],output_rates[2],output_rates[3]);
	fprintf(log_fp,"Erasures  %16d\nProb 9600 w errs  %8d\nCorr rate w errs    %6d\n",
		output_rates[4],output_rates[5],output_rates[6]);

	fprintf(log_fp,"\n\nMatrix of transitions\n");
	fprintf(log_fp,"                                    RX Decisions \n");
	fprintf(log_fp,"               9600       4800       2400    1200   erasure     prob.    correct\n");
	fprintf(log_fp,"                                                                9600     w/ errs\n");
	fprintf(log_fp,"                                                              w/ errs\n");
	fprintf(log_fp,"Tx Rates\n");
	for(i=0;i<NRATES_IN;i++)  {
		fprintf(log_fp,"  %s   ",rate_char[i]);
		for(j=0;j<NRATES_OUT;j++)fprintf(log_fp," %8d ",history[i][j]);
		fprintf(log_fp,"\n");
	}

	/* print event history */
	fprintf(log_fp,"\nEvent list\nFrame index   TX Rate    Event      RX Rate         Mask Index\n");
	for(i=0;i<nevents;i++) {
		fprintf(log_fp,"%8d         %s   %s",
			event_list[i][0],
			rate_char[event_list[i][1]],
			event_char[event_list[i][2]]
		);
		if(event_list[i][2]!=ERASURE)  fprintf(log_fp,"   %s",rate_out_char[event_list[i][3]]);
		if(event_list[i][2]==MODIFIED) fprintf(log_fp," %12d",event_list[i][4]);
		fprintf(log_fp,"\n");

		fprintf(event_fp,"%-8d  ",event_list[i][0]);
		if(event_list[i][2]==ERASURE) {
			fprintf(event_fp,"ERASURE");
		} else if (event_list[i][3] == 4) {
			fprintf(event_fp,"ERASURE");
		} else if (event_list[i][3] == 2) {
			fprintf(event_fp,"ERASURE");
		} else if (event_list[i][1] != event_list[i][3]) {
			fprintf(event_fp,"RANDOM");
		} else {
			fprintf(event_fp,"ERASURE");
		}
#if 0  /*...for debugging errmod program (remove when masking files)...*/
		switch (event_list[i][1]) {
		case FULL:
			fprintf(event_fp," 8");
			break;
		case HALF:
			fprintf(event_fp," 4");
			break;
		default:
			fprintf(event_fp," 1");
			break;
		}
#endif
		fprintf(event_fp,"\n");
	}

}  /* end of main */

#endif
