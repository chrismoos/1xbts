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
#include <math.h>
#include "struct.h"
#include "macro.h"
#include "rom.h"
#include "proto.h"




/* These are defined in silence.cc.  Consider moving them to macro.h to avoid
 * multiple definitions
 */
#define MAX_LOG_ENERGY	(2.6)	/* set to about 2.1 for is-127 equivalent */
#define MIN_LOG_ENERGY	(0.2)
#define LOG_ZERO_ENERGY	(-2.0)
#define MAX_LOG_ENERGY_WB (2.0*6.0)
#define MIN_LOG_ENERGY_WB (2.0*1.0)
#define LOG_ZERO_ENERGY_WB (2.0*-1.0)


/*======================================================================*/
/*         ..For use when eighth rate frames are dropped.               */
/*----------------------------------------------------------------------*/
#define BBG_WINDOW_LEN 104
#define BBG_M 80
//#define BBG_DELAY 24
#define BBG_FFTLEN 128

#define BBG_ALPHA 0.82
#define BBG_BETA1 0.60
#define BBG_BETA1_MINUS1 -0.40

#define BBG_BETA2 0.9925
#define BBG_BETA2_MINUS1 -0.0075
#define BBG_BETA2r 0.794
#define BBG_MIN_LEVEL -15

extern float ran0 (long *seed0);
extern void r_fft_flt (float *ptr, int isign);


/* This should be defined global or in the class so it can be
 * shared with preproc.cc
 */
static int ch_tbl[BBG_NUM_CHAN][2] = {

    {2, 3},
    {4, 5},
    {6, 7},
    {8, 9},
    {10, 11},
    {12, 13},
    {14, 16},
    {17, 19},
    {20, 22},
    {23, 26},
    {27, 30},
    {31, 35},
    {36, 41},
    {42, 48},
    {49, 55},
    {56, 63}
};

/* This can be replaced by the window[] variable in the class, but we need to
 * be sure that init_window() in preproc.cc is called before being used here.
 * For now define a copy here.
 */
static float BBG_window[BBG_WINDOW_LEN] = {
    0.001068, 0.009613, 0.026520, 0.051575,
    0.084259, 0.124084, 0.170319, 0.222198,
    0.278839, 0.339264, 0.402435, 0.467285,
    0.532684, 0.597534, 0.660706, 0.721130,
    0.777771, 0.829651, 0.875885, 0.915710,
    0.948395, 0.973450, 0.990356, 0.998901,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.999969, 0.999969, 0.999969, 0.999969,
    0.998901, 0.990356, 0.973450, 0.948395,
    0.915710, 0.875885, 0.829651, 0.777771,
    0.721130, 0.660706, 0.597534, 0.532684,
    0.467285, 0.402435, 0.339264, 0.278839,
    0.222198, 0.170319, 0.124084, 0.084259,
    0.051575, 0.026520, 0.009613, 0.001068
};

/* Need 128 point FFT, not available in WB */
#define			SIZE			128
#define			SIZE_BY_TWO		64
#define			NUM_STAGE		6

static double phs_tbl[SIZE];	/* holds the complex sinusoids */
static int first = TRUE;


/* FFT/IFFT function for real sequences */
static void c_fft (float *farray_ptr, int isign);
static void fill_tbl ();

static void
r_fft (float *farray_ptr, int isign)
{

    float ftmp1_real, ftmp1_imag, ftmp2_real, ftmp2_imag;
    int i, j;



    /* If this is the first call to the function, fill up the phase table  */

    if (first == TRUE)
	fill_tbl ();

    /* The FFT part */

    if (isign == 1) {

	/* Perform the complex FFT */

	c_fft (farray_ptr, isign);

	/* First, handle the DC and foldover frequencies */

	ftmp1_real = *farray_ptr;
	ftmp2_real = *(farray_ptr + 1);
	*farray_ptr = ftmp1_real + ftmp2_real;
	*(farray_ptr + 1) = ftmp1_real - ftmp2_real;

	/* Now, handle the remaining positive frequencies */

	for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i) {

	    ftmp1_real = *(farray_ptr + i) + *(farray_ptr + j);
	    ftmp1_imag = *(farray_ptr + i + 1) - *(farray_ptr + j + 1);
	    ftmp2_real = *(farray_ptr + i + 1) + *(farray_ptr + j + 1);
	    ftmp2_imag = *(farray_ptr + j) - *(farray_ptr + i);

	    *(farray_ptr + i) = (ftmp1_real + phs_tbl[i] * ftmp2_real -
				 phs_tbl[i + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + i + 1) = (ftmp1_imag + phs_tbl[i] * ftmp2_imag +
				     phs_tbl[i + 1] * ftmp2_real) / 2.0;
	    *(farray_ptr + j) = (ftmp1_real + phs_tbl[j] * ftmp2_real +
				 phs_tbl[j + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + j + 1) = (-ftmp1_imag - phs_tbl[j] * ftmp2_imag +
				     phs_tbl[j + 1] * ftmp2_real) / 2.0;

	}

    }

    /* The IFFT part */

    else {

	/* First, handle the DC and foldover frequencies */

	ftmp1_real = *farray_ptr;
	ftmp2_real = *(farray_ptr + 1);
	*farray_ptr = (ftmp1_real + ftmp2_real) / 2.0;
	*(farray_ptr + 1) = (ftmp1_real - ftmp2_real) / 2.0;

	/* Now, handle the remaining positive frequencies */

	for (i = 2, j = SIZE - i; i <= SIZE_BY_TWO; i = i + 2, j = SIZE - i) {

	    ftmp1_real = *(farray_ptr + i) + *(farray_ptr + j);
	    ftmp1_imag = *(farray_ptr + i + 1) - *(farray_ptr + j + 1);
	    ftmp2_real = -(*(farray_ptr + i + 1) + *(farray_ptr + j + 1));
	    ftmp2_imag = -(*(farray_ptr + j) - *(farray_ptr + i));

	    *(farray_ptr + i) = (ftmp1_real + phs_tbl[i] * ftmp2_real +
				 phs_tbl[i + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + i + 1) = (ftmp1_imag + phs_tbl[i] * ftmp2_imag -
				     phs_tbl[i + 1] * ftmp2_real) / 2.0;
	    *(farray_ptr + j) = (ftmp1_real + phs_tbl[j] * ftmp2_real -
				 phs_tbl[j + 1] * ftmp2_imag) / 2.0;
	    *(farray_ptr + j + 1) = (-ftmp1_imag - phs_tbl[j] * ftmp2_imag -
				     phs_tbl[j + 1] * ftmp2_real) / 2.0;

	}

	/* Perform the complex IFFT */

	c_fft (farray_ptr, isign);

    }

    return;

}				/* end r_fft () */



/* FFT/IFFT function for complex sequences */

/* The decimation-in-time complex FFT/IFFT is implemented below.
   The input complex numbers are presented as real part followed by
   imaginary part for each sample.  The counters are therefore
   incremented by two to access the complex valued samples. */

static void
c_fft (float *farray_ptr, int isign)
{

    int i, j, k, ii, jj, kk, ji, kj;
    float ftmp, ftmp_real, ftmp_imag;


    /* Rearrange the input array in bit reversed order */

    for (i = 0, j = 0; i < SIZE - 2; i = i + 2) {

	if (j > i) {

	    ftmp = *(farray_ptr + i);
	    *(farray_ptr + i) = *(farray_ptr + j);
	    *(farray_ptr + j) = ftmp;

	    ftmp = *(farray_ptr + i + 1);
	    *(farray_ptr + i + 1) = *(farray_ptr + j + 1);
	    *(farray_ptr + j + 1) = ftmp;

	}

	k = SIZE_BY_TWO;

	while (j >= k) {
	    j -= k;
	    k >>= 1;
	}

	j += k;

    }


    /* The FFT part */

    if (isign == 1) {

	for (i = 0; i < NUM_STAGE; i++) {	/* i is stage counter */

	    jj = (2 << i);	/* FFT size */
	    kk = (jj << 1);	/* 2 * FFT size */
	    ii = SIZE / jj;	/* 2 * number of FFT's */

	    for (j = 0; j < jj; j = j + 2) {	/* j is sample counter */

		ji = j * ii;	/* ji is phase table index */

		for (k = j; k < SIZE; k = k + kk) {	/* k is butterfly top */

		    kj = k + jj;	/* kj is butterfly bottom */

		    /* Butterfly computations */

		    ftmp_real = *(farray_ptr + kj) * phs_tbl[ji] -
			*(farray_ptr + kj + 1) * phs_tbl[ji + 1];

		    ftmp_imag = *(farray_ptr + kj + 1) * phs_tbl[ji] +
			*(farray_ptr + kj) * phs_tbl[ji + 1];

		    *(farray_ptr + kj) =
			(*(farray_ptr + k) - ftmp_real) / 2.0;
		    *(farray_ptr + kj + 1) =
			(*(farray_ptr + k + 1) - ftmp_imag) / 2.0;

		    *(farray_ptr + k) = (*(farray_ptr + k) + ftmp_real) / 2.0;
		    *(farray_ptr + k + 1) =
			(*(farray_ptr + k + 1) + ftmp_imag) / 2.0;

		}

	    }

	}

    }

    /* The IFFT part */

    else {

	for (i = 0; i < NUM_STAGE; i++) {	/* i is stage counter */

	    jj = (2 << i);	/* FFT size */
	    kk = (jj << 1);	/* 2 * FFT size */
	    ii = SIZE / jj;	/* 2 * number of FFT's */

	    for (j = 0; j < jj; j = j + 2) {	/* j is sample counter */

		ji = j * ii;	/* ji is phase table index */

		for (k = j; k < SIZE; k = k + kk) {	/* k is butterfly top */

		    kj = k + jj;	/* kj is butterfly bottom */

		    /* Butterfly computations */

		    ftmp_real = *(farray_ptr + kj) * phs_tbl[ji] +
			*(farray_ptr + kj + 1) * phs_tbl[ji + 1];

		    ftmp_imag = *(farray_ptr + kj + 1) * phs_tbl[ji] -
			*(farray_ptr + kj) * phs_tbl[ji + 1];

		    *(farray_ptr + kj) = *(farray_ptr + k) - ftmp_real;
		    *(farray_ptr + kj + 1) =
			*(farray_ptr + k + 1) - ftmp_imag;

		    *(farray_ptr + k) = *(farray_ptr + k) + ftmp_real;
		    *(farray_ptr + k + 1) = *(farray_ptr + k + 1) + ftmp_imag;

		}

	    }

	}

    }

    return;

}				/* end of c_fft () */



/* Function to fill the phase table values */

static void
fill_tbl (void)
{
    int i;
    double delta_f, theta;

    delta_f = -PI / (double) SIZE_BY_TWO;

    for (i = 0; i < SIZE_BY_TWO; i++) {

	theta = delta_f * (double) i;
	phs_tbl[2 * i] = cos (theta);
	phs_tbl[2 * i + 1] = sin (theta);

    }

    first = FALSE;
    return;
}				/* end fill_tbl () */



static float
get_bgestimate_energy (float *E_bgn)
{
    float tmpF;
    int chan;
    int j1, j2;
    int indx;

    tmpF = pow (10.0, E_bgn[1] / 10.0);

    for (chan = 0; chan < (BBG_NUM_CHAN); chan++) {
	j1 = ch_tbl[chan][0];
	j2 = ch_tbl[chan][1];
	for (indx = j1; indx <= j2; indx++) {
	    tmpF += pow (10.0, E_bgn[chan + 2] / 10.0);
	}
    }
    return (tmpF);
}

void
FGV_MEM::update_bbg_estimate2 (float *lpc, float energy)
{
    static int firstTime = 1;
    float s_m[BBG_FFTLEN];
    float E_ch[BBG_NUM_CHAN + 2];
    int indx;
    float tmpF;
    float energy_comp;
    float walpha = 1.0;

    if (firstTime) {
	/* Move these to init_bbg() */
	firstTime = 0;
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] = -10.0;
	    E_ch_mem[indx] = 0.0;
	    E_bgn_smooth[indx] = -10.0;
	}
    }

    /* Do actual update in this section */
    BBG_frmSkipCnt++;

    /* Obtain impulse response of LPCs */
    {
	float memory[10];
	float accF;
	int j;

	memory[0] = 1.0;
	s_m[0] = 1.0;
	for (indx = 1; indx < 10; indx++) {
	    memory[indx] = 0.0;
	}
	accF = 0;
	for (indx = 1; indx < BBG_FFTLEN; indx++) {
	    accF = 0.0;
	    for (j = 9; j > 0; j--) {
		accF -= lpc[j] * memory[j];
		memory[j] = memory[j - 1];
	    }
	    accF -= lpc[0] * memory[0];
	    memory[0] = accF;
	    s_m[indx] = accF;
	}
    }
    energy_comp = 0;
    for (indx = 0; indx < BBG_FFTLEN; indx++) {
	energy_comp += s_m[indx] * s_m[indx];
    }
    energy_comp =
	10 * log10 (energy * BBG_FFTLEN / Hlength) -
	10.0 * log10 (energy_comp);
    r_fft (s_m, +1);


    /* Now compute magnitude in dB */
    for (indx = 0; indx < BBG_FFTLEN / 2; indx++) {
	tmpF =
	    s_m[2 * indx] * s_m[2 * indx] + s_m[2 * indx + 1] * s_m[2 * indx +
								    1];
	s_m[indx] = 10.0 * log10 (tmpF) + energy_comp;
	if (s_m[indx] < -40.0) {
	    s_m[indx] = -40.0;
	}
    }
    if (BBG_frmSkipCnt < 3) {
	int chan;
	int j1, j2;

	j1 = ch_tbl[0][0];
	for (indx = 0; indx < j1; indx++) {
	    E_ch[indx] = s_m[indx];
	}
	for (chan = 0; chan < BBG_NUM_CHAN; chan++) {
	    j1 = ch_tbl[chan][0];
	    j2 = ch_tbl[chan][1];
	    tmpF = 0;
	    for (indx = j1; indx <= j2; indx++) {
		tmpF += s_m[indx];
	    }
	    tmpF = tmpF / (float) (j2 - j1 + 1);
	    E_ch[chan + 2] = tmpF;
	}
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] = E_ch[indx];
	    if (E_bgn[indx] < BBG_MIN_LEVEL) {
		E_bgn[indx] = BBG_MIN_LEVEL;
		E_ch[indx] = E_bgn[indx];
	    }
	    /* Initialization was done without bin grouping. Fixed here. */
	    E_ch_mem[indx] = E_ch[indx];
	}
    }
    else {
	int chan;
	int j1, j2;

	j1 = ch_tbl[0][0];
	for (indx = 0; indx < j1; indx++) {
	    E_ch[indx] = s_m[indx];
	}
	for (chan = 0; chan < BBG_NUM_CHAN; chan++) {
	    j1 = ch_tbl[chan][0];
	    j2 = ch_tbl[chan][1];
	    tmpF = 0;
	    for (indx = j1; indx <= j2; indx++) {
		tmpF += s_m[indx];
	    }
	    tmpF = tmpF / (float) (j2 - j1 + 1);
	    E_ch[chan + 2] = tmpF;
	}
	walpha = walpha * BBG_ALPHA;

	if (rate == 1) {
	    walpha = walpha * 0.8;
	    for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
		E_ch_mem[indx] =
		    walpha * E_ch_mem[indx] + (1.0 - walpha) * E_ch[indx];
	    }
	}
	else {
	    for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
		E_ch_mem[indx] =
		    walpha * E_ch_mem[indx] + (1.0 - walpha) * (E_ch[indx]);
	    }
	}
    }
    if (rate == 1) {
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] =
		BBG_BETA1 * E_ch_mem[indx] + (1 - BBG_BETA1) * E_ch[indx];
	}
    }
    else {
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    if (E_ch_mem[indx] < E_bgn[indx]) {
		if (E_ch_mem[indx] > BBG_MIN_LEVEL) {
		    E_bgn[indx] = E_ch_mem[indx];
		}
		else {
		    E_bgn[indx] = BBG_MIN_LEVEL;
		    E_ch_mem[indx] = E_bgn[indx];
		}
	    }
	    else {
		/* Prevent noise floor from rising too fast when signal is suspected
		 * to be high energy speech.
		 */
		if ((E_ch_mem[indx] - E_bgn[indx]) > 12) {
		    E_bgn[indx] += 0.0078;
		}
		else {
		    E_bgn[indx] += 0.0234;
		}
	    }
	}
    }
    bbg_dtx_likely_bgnenergy = get_bgestimate_energy (E_bgn);
    if ((energy / bbg_dtx_likely_bgnenergy >
	 pow (10.0, (36.1236 + 5.0) / 20.0))) {
	bbg_dtx_likely_flag = 0;
    }
    else {
	bbg_dtx_likely_flag = 1;
    }

}

void
FGV_MEM::update_bgestimate (float *bufF, int rate, float walpha)
{
    static int firstTime = 1;
    float s_m[BBG_FFTLEN];
    float E_ch[BBG_NUM_CHAN + 2];
    float *s_m_ptr;
    float *win_ptr;
    int indx;
    float tmpF;
    float comp_fac;

    if (firstTime) {
	/* Move these to init_bbg() */
	firstTime = 0;
	for (indx = 0; indx < BBG_DELAY; indx++) {
	    bbg_window_overlap[indx] = 0;
	}
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] = -10.0;
	    E_ch_mem[indx] = 0.0;
	    E_bgn_smooth[indx] = -10.0;
	}
    }

    /* Do actual update in this section */
    BBG_frmSkipCnt++;

    /* Process first "subframe" of 80 samples */
    s_m_ptr = &s_m[0];
    win_ptr = &BBG_window[0];
    for (indx = 0; indx < BBG_DELAY; indx++) {
	(*s_m_ptr++) = bbg_window_overlap[indx] * (*win_ptr++);
    }
    for (indx = 0; indx < BBG_M; indx++) {
	(*s_m_ptr++) = bufF[indx] * (*win_ptr++);
    }
    for (indx = 0; indx < BBG_FFTLEN - (BBG_M + BBG_DELAY); indx++) {
	(*s_m_ptr++) = 0;
    }
    for (indx = 0; indx < BBG_DELAY; indx++) {
	bbg_window_overlap[indx] = bufF[BBG_M - BBG_DELAY + indx];
    }
/* EMCZ: The rate==2 section needs a fix.  Because it is possible to
 * receive no rate==1/8 frames at all, nelp could reduce energy to zero
 * (look at ER_exct_nrg_avg).  This can be solved in two ways, modifying
 * nelp to ignore ER_exct_nrg_avg when the previous frame was rate==0,
 * or generate ER_exct_nrg_avg based on suppressed eighth rate frames.
 * The latter approach needs generating an excitation equivalent for
 * suppressed eighth rate frames, but the code to do that may be
 * necessary to do a partial re-encode instead of a full re-encode of
 * rate==1 frames anyway.
 */
    if (data_packet.WB_MODE_BIT == 1) {
	if (rate == 1) {
	    /* There seems to be a 4-6dB drop in energy when an eighth rate
	     * packet is decoded, reencoded, and then decoded again.
	     * Compensate for that before reenconding. */
	    comp_fac = 2.0;
	}
	else if (rate == 2) {
	    /* NELP reduces energy of first nelp frame right after an eighth
	     * rate frame.  Compensate for that reduction in energy.  Since
	     * attn_fac is 1.0 for other frames, this can be done on all
	     * nelp frames.
	     */
	    comp_fac = 1.0 / attn_fac;
	}
	if (rate == 1 || rate == 2) {
//fprintf(stderr, "rate=%d, attn_fac = %e comp_fac = %.2f\n", rate, attn_fac, comp_fac);
	    s_m_ptr = &s_m[0];
	    for (indx = 0; indx < BBG_M; indx++) {
		(*s_m_ptr++) = comp_fac * (*s_m_ptr);
	    }
	    for (indx = 0; indx < BBG_DELAY; indx++) {
		bbg_window_overlap[indx] =
		    comp_fac * bbg_window_overlap[indx];
	    }
	}
	/* Compute fft of real signal */
	r_fft (s_m, +1);

    }
    else {
	if (rate == 1) {
	    /* There seems to be a 4-6dB drop in energy when an eighth rate
	     * packet is decoded, reencoded, and then decoded again.
	     * Compensate for that before reenconding. */
	    s_m_ptr = &s_m[0];
	    for (indx = 0; indx < BBG_M; indx++) {
		(*s_m_ptr++) = 2 * (*s_m_ptr);
	    }
	    for (indx = 0; indx < BBG_DELAY; indx++) {
		bbg_window_overlap[indx] = 2 * bbg_window_overlap[indx];
	    }
	}

	/* Compute fft of real signal */
	r_fft_flt (s_m, +1);

    }

    /* Now compute magnitude in dB */
    for (indx = 0; indx < BBG_FFTLEN / 2; indx++) {
	tmpF =
	    s_m[2 * indx] * s_m[2 * indx] + s_m[2 * indx + 1] * s_m[2 * indx +
								    1];
	if (tmpF < 0.0001) {
	    tmpF = 0.0001;
	}
	s_m[indx] = 10.0 * log10 (tmpF);
    }
    if (BBG_frmSkipCnt < 3) {
	int chan;
	int j1, j2;

	j1 = ch_tbl[0][0];
	for (indx = 0; indx < j1; indx++) {
	    E_ch[indx] = s_m[indx];
	}
	for (chan = 0; chan < BBG_NUM_CHAN; chan++) {
	    j1 = ch_tbl[chan][0];
	    j2 = ch_tbl[chan][1];
	    tmpF = 0;
	    for (indx = j1; indx <= j2; indx++) {
		tmpF += s_m[indx];
	    }
	    tmpF = tmpF / (float) (j2 - j1 + 1);
	    E_ch[chan + 2] = tmpF;
	}
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] = E_ch[indx];
	    if (E_bgn[indx] < BBG_MIN_LEVEL) {
		E_bgn[indx] = BBG_MIN_LEVEL;
		E_ch[indx] = E_bgn[indx];
	    }
	    /* Initialization was done without bin grouping. Fixed here. */
	    E_ch_mem[indx] = E_ch[indx];
	}
    }
    else {
	int chan;
	int j1, j2;

	j1 = ch_tbl[0][0];
	for (indx = 0; indx < j1; indx++) {
	    E_ch[indx] = s_m[indx];
	}
	for (chan = 0; chan < BBG_NUM_CHAN; chan++) {
	    j1 = ch_tbl[chan][0];
	    j2 = ch_tbl[chan][1];
	    tmpF = 0;
	    for (indx = j1; indx <= j2; indx++) {
		tmpF += s_m[indx];
	    }
	    tmpF = tmpF / (float) (j2 - j1 + 1);
	    E_ch[chan + 2] = tmpF;
	}
	walpha = walpha * BBG_ALPHA;

	if (rate == 1) {
	    walpha = walpha * 0.6;
	    for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
		E_ch_mem[indx] =
		    walpha * E_ch_mem[indx] + (1.0 - walpha) * E_ch[indx];
	    }
	}
	else {
	    for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
		E_ch_mem[indx] =
		    walpha * E_ch_mem[indx] + (1.0 - walpha) * (E_ch[indx] +
								20 *
								log10 (1.5));
	    }
	}
    }

    if (rate == 1) {
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    E_bgn[indx] =
		BBG_BETA1 * E_ch_mem[indx] + (1 - BBG_BETA1) * E_ch[indx];
	}
    }
    else {
	for (indx = 0; indx < BBG_NUM_CHAN + 2; indx++) {
	    if (E_ch_mem[indx] < E_bgn[indx]) {
		if (E_ch_mem[indx] > BBG_MIN_LEVEL) {
		    E_bgn[indx] = E_ch_mem[indx];
		}
		else {
		    E_bgn[indx] = BBG_MIN_LEVEL;
		    E_ch_mem[indx] = E_bgn[indx];
		}
	    }
	    else {
		/* Prevent noise floor from rising too fast when signal is suspected
		 * to be high energy speech.
		 */
		if ((E_ch_mem[indx] - E_bgn[indx]) > 12) {
		    E_bgn[indx] += 0.0078;
		}
		else {
		    E_bgn[indx] += 0.0234;
		}
	    }
	}
    }

}


/*
 * Generates noise using estimated background noise shape
 */
//static void gen_bg_fromestimate(float *bufF)
void
FGV_MEM::gen_bg_fromestimate (float *bufF)
{
    int indx;
    static long seed0 = 19267;
    static float X_dec_ph[2];
    static float X_dec_overlap[BBG_DELAY];
    float X_dec[BBG_FFTLEN];
    float tmpF;
    int chan;
    int j1, j2;

    /* SMOOTH E_BGN BEFORE RE-GENERATING COMFORT NOISE */
    if (BBG_frmSkipCnt2 < 10)	// Wait for first true estimate
    {
	BBG_frmSkipCnt2++;
	for (indx = 0; indx < (BBG_NUM_CHAN + 2); indx++) {
	    E_bgn_smooth[indx] =
		BBG_BETA2r * E_bgn_smooth[indx] + (1 -
						   BBG_BETA2r) * E_bgn[indx];
	}
    }
    else {
	for (indx = 0; indx < (BBG_NUM_CHAN + 2); indx++) {
	    E_bgn_smooth[indx] =
		BBG_BETA2 * E_bgn_smooth[indx] + (1 -
						  BBG_BETA2) * E_bgn[indx];
	}
    }

    X_dec[0] = 0;
    X_dec[1] = 0;
    tmpF = ran0 (&seed0);
    X_dec_ph[0] = cos (2 * PI * tmpF);
    X_dec_ph[1] = sin (2 * PI * tmpF);
/* USE SMOOTHED E_BGN FOR RE-GENERATING COMFORT NOISE */
    tmpF = pow (10.0, E_bgn_smooth[1] / 20.0);
    X_dec[2] = tmpF * X_dec_ph[0];
    X_dec[2 + 1] = tmpF * X_dec_ph[1];

    for (chan = 0; chan < (BBG_NUM_CHAN); chan++) {
	j1 = ch_tbl[chan][0];
	j2 = ch_tbl[chan][1];
	for (indx = j1; indx <= j2; indx++) {
	    tmpF = ran0 (&seed0);
	    X_dec_ph[0] = cos (2 * PI * tmpF);
	    X_dec_ph[1] = sin (2 * PI * tmpF);
	    tmpF = pow (10.0, E_bgn_smooth[chan + 2] / 20.0);
	    X_dec[2 * indx] = tmpF * X_dec_ph[0];
	    X_dec[2 * indx + 1] = tmpF * X_dec_ph[1];
	}
    }
    if (data_packet.WB_MODE_BIT == 1)
	r_fft (X_dec, -1);
    else
	r_fft_flt (X_dec, -1);


    if (BBG_frmSkipCnt < 1)	// Wait for first true estimate
    {
	for (indx = 0; indx < (BBG_M + BBG_DELAY); indx++) {
	    X_dec[indx] = 12.0 * (ran0 (&seed0) - 0.5);
	}
    }
    for (indx = 0; indx < BBG_M + BBG_DELAY; indx++) {
	X_dec[indx] = X_dec[indx] * BBG_window[indx];
    }
    for (indx = 0; indx < BBG_DELAY; indx++) {
	X_dec[indx] += X_dec_overlap[indx];
	X_dec_overlap[indx] = X_dec[indx + BBG_M];
    }

    for (indx = 0; indx < BBG_M; indx++) {
	tmpF = (X_dec[indx] < -32768.0) ? (-32768.0) : (X_dec[indx]);
	tmpF = ((tmpF + 0.5) > 32767.0) ? (32767.0) : (tmpF + 0.5);
	bufF[indx] = tmpF;
    }

}


static float speechBufHist[GUARD + LOOKAHEAD_LEN];

void
FGV_MEM::bbg_eighth_rate_encode (float inBufF[SPEECH_BUFFER_LEN],
				 short *outBuf16)
{
    register int i, j;
    float lpc_noise[ORDER];
    float lsp_noise[ORDER];
    float lspQ_noise[ORDER];
    float H_noise[Hlength];
    float lpc_i[ORDER];
    float lsp_i[ORDER];
    short lspindx[2];
    int sf;
    int sfSize;
    int indx;
    float sum;
    float scale;
    float Sum[3];
    float tmp;
    int best;
    float lresidual[SubFrameSize];
    float speechBuf[GUARD + FrameSize + LOOKAHEAD_LEN];
    float maxEnergy;
    float resmem[ORDER];
    float tmpScratch[ORDER + 1];
    float tmpRs[17];
    float D;

    PktPtr[0] = 16;
    PktPtr[1] = 0;
    for (indx = 0; indx < PACKWDSNUM; indx++) {
	RxPkt[indx] = 0;
    }
    clearBitTotal ();

    /* Process Data */
    for (j = 0; j < FrameSize; j++) {
	speechBuf[j + GUARD + LOOKAHEAD_LEN] = inBufF[j];
    }

    /* Make sure HPspeech - HPspeech+2*GUARD have the right memory */
    for (j = 0; j < GUARD + LOOKAHEAD_LEN; j++) {
	speechBuf[j] = speechBufHist[j];
    }

    /* Update ConstHPspeech for next frame */
    for (j = 0; j < GUARD + LOOKAHEAD_LEN; j++) {
	speechBufHist[j] = speechBuf[j + GUARD + LOOKAHEAD_LEN];
    }


    /* Run lpcanalysis on generated noise */
    lpcanalys (lpc_noise, tmpScratch, speechBuf, ORDER,
	       LPC_ANALYSIS_WINDOW_LENGTH, tmpRs, HammingWindow_bbg);
    /* Bandwidth expansion */
    weight (lpc_noise, lpc_noise, _Gamma_4, ORDER);
    /* Convert to lsp */
    a2lsp (lsp_noise, lpc_noise, ORDER);
    /* Quantize lsps using eighth rate quantizer */

    if (data_packet.WB_MODE_BIT == 1) {
	enc_lsp_vq_10 (lsp_noise, lspindx, lspQ_noise);

	for (indx = 0; indx < 2; indx++) {
	    Bitpack (lspindx[indx], (unsigned short *) RxPkt, 5, PktPtr);
	}

	maxEnergy = 0.0;
	for (j = 0; j < FrameSize; j++) {
	    maxEnergy += speechBuf[j + GUARD] * speechBuf[j + GUARD];
	}
	maxEnergy = log ((maxEnergy + 1.0) / FrameSize);
    }
    else {			//narrowband case
	lspmaq (lsp_noise, ORDER, 1, 2, nsub8, nsize8,
		0.5, lspQ_noise, lspindx, 0, lsptab8);
	for (indx = 0; indx < 2; indx++) {
	    Bitpack (lspindx[indx],
		     (unsigned short *) RxPkt, lognsize8[indx], PktPtr);
	}

	for (indx = 0; indx < ORDER; indx++) {
	    resmem[indx] = 0;
	}

	lsp2a (lpc_i, OldlspD, ORDER);

	for (sf = 0; sf < NoOfSubFrames; sf++) {
	    // This should be moved inside if statement below, and use sf+1 for sf<2
	    Interpol (lsp_i, OldlspD, lspQ_noise, sf, ORDER);
	    //using lspQ_noise above instead of the previously used lsp_noise
	    // to ensure the LSP used to compute the residual signal and the
	    //impulse response energy are the same
	    lsp2a (lpc_i, lsp_i, ORDER);
	    if (sf < 2) {
		sfSize = SubFrameSize - 1;
	    }
	    else {
		sfSize = SubFrameSize;
	    }
	    GetResidual (lresidual,
			 &inBufF[sf * (SubFrameSize - 1)], lpc_i,
			 resmem, ORDER, sfSize);

	    scale = pow (10.0, FIXED_EIGHTH_RATE_ATTEN_AMT / 20.0);
	    scale = scale * 1.1;

	    sum = 0;
	    for (indx = 0; indx < sfSize; indx++) {
		sum += fabs (lresidual[indx]);
	    }
	    sum = sum / sfSize;
	    sum = sum * 2;	//EMCZ manual adjust for testing only
	    sum = sum * (0.707 / 0.656);
	    if (sum < 1) {
		sum = 1;
	    }
	    Sum[sf] = log10 (sum / scale);
	}

	maxEnergy = Sum[0];
	for (sf = 1; sf < NoOfSubFrames; sf++) {
	    if (Sum[sf] > maxEnergy) {
		maxEnergy = Sum[sf];
	    }
	}

    }
    sum = 1e30;
    for (i = 0; i < NUM_Q_LEVELS; i++) {
	if (data_packet.WB_MODE_BIT == 1)
	    D = maxEnergy - (MIN_LOG_ENERGY_WB +
			     (float) i * (MAX_LOG_ENERGY_WB -
					  MIN_LOG_ENERGY_WB) /
			     (float) NUM_Q_LEVELS);
	else
	    D = maxEnergy - (MIN_LOG_ENERGY +
			     (float) i * (MAX_LOG_ENERGY -
					  MIN_LOG_ENERGY) /
			     (float) NUM_Q_LEVELS_NB);


	tmp = D * D;
	if (tmp < sum) {
	    best = i;
	    sum = tmp;
	}
    }
    if (data_packet.WB_MODE_BIT == 1)
	Bitpack (best, (unsigned short *) RxPkt, NUM_EQ_BITS_WB, PktPtr);
    else
	Bitpack (best, (unsigned short *) RxPkt, NUM_EQ_BITS, PktPtr);

    for (indx = 0; indx < PACKWDSNUM; indx++) {
	outBuf16[indx] = RxPkt[indx];
    }
}



/*
 * Updates the background noise envelope shape
 */
//void update_bbg_estimate(short *buf16, int rate, float walpha)
void
FGV_MEM::update_bbg_estimate (float *bufF, int rate, float walpha)
{
    update_bgestimate (bufF, rate, walpha);
    update_bgestimate (&bufF[SPEECH_BUFFER_LEN / 2], rate, walpha);
}


void
FGV_MEM::gen_encode_bbg (short *buf16)
{
    float bufF[SPEECH_BUFFER_LEN];

    gen_bg_fromestimate (bufF);
    gen_bg_fromestimate (&bufF[80]);

    bbg_eighth_rate_encode (bufF, buf16);
}


void
FGV_MEM::init_bbg ()
{
    unsigned int indx;

    /* Clear out memory for bbg_eighth_rate_encode */
    for (indx = 0; indx < GUARD; indx++) {
	speechBufHist[indx] = 0;
    }
    BBG_frmSkipCnt = 0;
    BBG_frmSkipCnt2 = 0;
    bbg_dtx_likely_flag = 0;
    /* IF HAMMINGWINDOW IS SHARED WITH ENCODER, REMOVE THIS BLOCK OF CODE */
    {
	float factor;
	short k;

	factor = AUTO_PI2 / (float) LPC_ANALYSIS_WINDOW_LENGTH;


#define GAMMA   8.4		/* length 320, center at 240 */
#define LAMBDA  0.0

	double maxw;

	/* Get base window */
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    HammingWindow_bbg[k] =
		(0.5 + LAMBDA / 2) - (0.5 - LAMBDA / 2) * cos (factor * k);

	/* Exponentially warp the base window */
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    HammingWindow_bbg[k] =
		(HammingWindow_bbg[k] -
		 LAMBDA) * exp (GAMMA * k / LPC_ANALYSIS_WINDOW_LENGTH);

	/* Find max and normalize */
	maxw = HammingWindow_bbg[0];
	for (k = 1; k < LPC_ANALYSIS_WINDOW_LENGTH; k++)
	    if (HammingWindow_bbg[k] > maxw)
		maxw = HammingWindow_bbg[k];
	for (k = 0; k < LPC_ANALYSIS_WINDOW_LENGTH; k++) {
	    HammingWindow_bbg[k] =
		(1.0 - LAMBDA) * HammingWindow_bbg[k] / maxw + LAMBDA;
	}
	for (k = 0; k < ORDER + 7; k++)
	    w[k] =
		exp (-0.5 * pow (2 * 3.1416 * F0 * (float) k / 8000.0, 2.0));
	w[0] = 1.00003;


    }
}
