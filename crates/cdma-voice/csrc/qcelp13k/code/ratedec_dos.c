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
/* ratedec.c - selects encoding rate to use */


#include"celp.h"
#include"math.h"
#include "coderate.h"

/* calculates various features for voiced/unvoiced classification
   and rate decision */

void compute_features(
    float                *pitch_cor,
    int                  *pitch,
    float                *resid,
    float                *speech,
    struct UNV_FEATURES  *unv_features,
    float                *acf,
    float                *lpc,
    float                *lsp,
    float                *band_energy
)
{

/* detect a clearly unvoiced segment of speech using the following features*/
/* or a subset thereof:                                                 */
/* nacf of residual, zero crossings of speech, energy level, spectral   */
/* tilt etc.                                                            */

   float acf0_0;
   float acf0_0p, dc;
   int zero,i;
   float resid_energy, diff_lsp_tmp;
   acf0_0 = EPS;

   /* calculate nacf function */

   for(i=0;i<FSIZE/DECIMATE_F;i++)
     acf0_0 += resid[PMAX/DECIMATE_F+i]*resid[PMAX/DECIMATE_F+i];

   acf0_0p = acf0_0;
   for(i=0;i<FSIZE/DECIMATE_F;i++)
     acf0_0 += resid[PMAX/DECIMATE_F+i-pitch[0]]*
                                  resid[PMAX/DECIMATE_F+i-pitch[0]];

   acf0_0 *= 0.5;

   if(acf0_0 == 0.0)  acf0_0 = 1000000.;

   unv_features->nacf0 = pitch_cor[0]/acf0_0;
   unv_features->open_pitch = pitch[0];

   unv_features->nacf_frame = unv_features->nacf0;

   /* calculate zero-crossings */
   dc = 0.0;
   for(i=0;i<FSIZE;i++)  /* strip out any dc that survives the front */
     /* end hpf (this shouldn't be necessary with*/
     /* a good HPF up front                      */
     dc += speech[i];

   dc /= FSIZE;

   zero =0;
   for(i=0;i<FSIZE-1;i++) {
     if((speech[i]-dc)*(speech[i+1]-dc) < 0.0) zero++;
   }

   unv_features->zero = zero;

   /* calculate differential residual energy: might be used for more than  */
   /* unvoiced classification                                              */

     /*unv_features->res_energy_diff0 =
       10*log10(acf0_0p/unv_features->last_res_energy);*/

   /* calculate prediction gain from acf and lpc coefficients */
   /* Re[0] = acf[0] - sum over k {acf[k]*lpc[k]              */

   resid_energy = acf[0];
   for(i=1;i<= LPCORDER; i++)
     resid_energy -= acf[i]*lpc[i-1]; /* is the sign on the lpc's correct */

   /*    there is a hamming window     */
   /*    factor in here that will make */
   /*    this smaller by about 3 dB    */

   unv_features->pred_gain = 10*log10(acf[0]/resid_energy);

   if(unv_features->pred_gain < 0.0) unv_features->pred_gain = 0.0;

   unv_features->pred_gain_frame = (100.0/160.)*unv_features->pred_gain+
				    (60.0/160.0)*unv_features->last_pred_gain;

   /* now store memory for next time */
   /*unv_features->last_res_energy = resid_energy;*/
   unv_features->last_nacf = unv_features->nacf0;
   unv_features->pred_gain_diff = -(unv_features->pred_gain_frame-
                                   unv_features->last_pred_gain_frame);
   unv_features->last_pred_gain = unv_features->pred_gain;
   unv_features->last_pred_gain_frame = unv_features->pred_gain_frame;

   /* calculate the difference in lsp's from the last frame */

   /*unv_features->diff_lsp =0.0;*/
   diff_lsp_tmp =0.0;
   for(i=0;i< LPCORDER; i++)
     diff_lsp_tmp += (lsp[i]-unv_features->last_lsp[i])*
       (lsp[i]-unv_features->last_lsp[i]);

   unv_features->diff_lsp = 0.625*diff_lsp_tmp + 0.375*unv_features->last_diff_lsp;
   unv_features->last_diff_lsp = diff_lsp_tmp;

   for(i=0;i< FREQBANDS; i++)
     unv_features->band_energy[i] = band_energy[i];

   for(i=0;i< LPCORDER; i++)
     unv_features->last_lsp[i] = lsp[i];

}/* end of compute_features() */

void band_energy_fcn(
    float               *R,
    float               *energy,
    struct ENCODER_MEM  *e_mem
)
{
    int i,j;

    /* find the energy in each band   */
    for (j=0; j<FREQBANDS; j++) {
      energy[j] = e_mem->r_filt[j][0]*R[0];
      for(i=1;i<FILTERORDER;i++) {
	energy[j]+=2.0*e_mem->r_filt[j][i]*R[i];
      }
    }
}

void update_hist_cnt(
    struct ENCODER_MEM  *e_mem,
    int                 rate
)
{
  /* this routine will keep track of the number of frames in each rate */
  /* and will update 2 histograms: target_snr's when frame is full rate */
  /* and target_snr's when frame is 1/2 rate                           */

  if(rate == FULLRATE_VOICED){
    if(e_mem->target_snr <= e_mem->target_snr_thr){
      e_mem->full_cnt += 1;

    }
    else
      {
	e_mem->full_force += 1;
      }
  }
  else if(rate == HALFRATE_VOICED){
    if(e_mem->target_snr > e_mem->target_snr_thr){
      e_mem->half_cnt += 1;
    }
    else
      {
	e_mem->half_force += 1;
      }
  }
  else if(rate == QUARTERRATE_UNVOICED){
    e_mem->quarter_cnt += 1;
  }
  else{
    fprintf(stderr,"Illegal rate/mode assignment: %d\n", rate);
    exit(-4);
  }

  if(rate == FULLRATE_VOICED || rate == HALFRATE_VOICED){
    if(e_mem->target_snr <= e_mem->target_snr_thr){

      /* note that hist_below[0] contains count of full or half frames that */
      /* were less than 1 db below threshold, hist_below[3] contains count*/
      /* of frames declared full that were  greater than 3 db below */
      /* the threshold                                                 */

      if((e_mem->target_snr_thr - e_mem->target_snr) >= 3){
	e_mem->hist_below[3] += 1;
      }
      else
	e_mem->hist_below[(int)(e_mem->target_snr_thr - e_mem->target_snr)] += 1;
    }
    else{
      /* note that hist_above3[0] contains count of full and half frames that */
      /* were less than 1 db above threshold, hist_above[3] contains count*/
      /* of frames declared full that were greater than 3 db above */
      /* the threshold                                                 */

      if((e_mem->target_snr - e_mem->target_snr_thr) >= 3){
	e_mem->hist_above[3] += 1;
      }
      else
	e_mem->hist_above[(int)(e_mem->target_snr - e_mem->target_snr_thr)] += 1;
    }
  }
}/* end of update_hist_cnt() */


void adjust_rate_down(
    struct ENCODER_MEM  *e_mem
)
{
/* look at histogram of target_snr's when frame was declared full and   */
/* adjust threshold so that a larger % of full rate frames get declared */
/* half                                                                 */

    int n_to_half;
    int total_frames,i;
/* calculate what the number of full_rate frames should have been to achieve*/
/* the desired rate in the last stat block                                  */
    n_to_half = (e_mem->full_cnt*FULLRATE_BPS + e_mem->half_cnt*HALFRATE_BPS
                + e_mem->quarter_cnt*QUARTER_BPS + e_mem->full_force*FULLRATE_BPS
                + e_mem->half_force*HALFRATE_BPS-e_mem->avg_rate_thr*STATWINDOW)/HALFRATE_BPS;

/* now step back in the histogram until you find the drop in snr threshold */
/* that is necessary to achieve the desired rate reduction                 */

    i = 0;
    total_frames = 0;
    while(total_frames < n_to_half && i < 4 ){
        total_frames += e_mem->hist_below[i];
	i++;
    }
    e_mem->target_snr_thr -= i; /* reduce the threshold by integer db's*/

}/* end of adjust_rate_down() */

void adjust_rate_up(
    struct ENCODER_MEM  *e_mem
)
{
/* look at histogram of target_snr's when frame was declared 1/2  and   */
/* adjust threshold so that a smaller % of full rate frames get declared */
/* half                                                                 */

    int n_to_full;
    int total_frames,i;
/* calculate what the number of full_rate frames should have been to achieve*/
/* the desired rate in the last stat block                                  */
    n_to_full = -(e_mem->full_cnt*FULLRATE_BPS + e_mem->half_cnt*HALFRATE_BPS
                + e_mem->quarter_cnt*QUARTER_BPS + e_mem->full_force*FULLRATE_BPS
                + e_mem->half_force*HALFRATE_BPS-e_mem->avg_rate_thr*STATWINDOW)/HALFRATE_BPS;

/* now step back in the histogram until you find the inc in snr threshold */
/* that is necessary to achieve the desired rate increase                 */

    i = 0;
    total_frames = 0;
    while(total_frames  < n_to_full && i < 4 ){
        total_frames += e_mem->hist_above[i];
	i++;
    }
    e_mem->target_snr_thr += i; /* reduce the threshold by integer db's*/

} /* end of adjust_rate_up() */


void select_mode1(
    int                 *rate,
    float               *acf,
    struct CONTROL      *control,
    struct ENCODER_MEM  *e_mem
)
{

   int i, k;
   /* this is the acf of the noise reducing prediction filter */
   float R_interp[32];



   /* Section 2.4.4.1 */
   for(i=0; i<LPCORDER+LPC_ORDER_NOISE-1; i++) {
     R_interp[i] = acf[i];
   }

   /* now filter the input speech by bandpass filters and */
   /* derive the necessary bandenergies */

   band_energy_fcn(R_interp, e_mem->band_power, e_mem);


   /* now our threshold comparison and background noise estimation */
   /* is done for all frequency bands                              */
   /* rate thresholds are calculated below, but not saved */

   for(k=0; k<FREQBANDS;k++) {
     e_mem->band_rate[k]=EIGHTH;
     for (i=0; i<2; i++) {
       /* since we changed SMSNR to 0.6 from 0.8 */
       if (e_mem->band_power[k]>THRESH_SNR[k][e_mem->snr_map[k]][i]*
	                                         e_mem->band_noise_sm[k])
	 {
	   if(e_mem->band_rate[k] == EIGHTH)
	     e_mem->band_rate[k]=HALFRATE_VOICED;
	   else if(e_mem->band_rate[k] == HALFRATE_VOICED)
	     e_mem->band_rate[k]=FULLRATE_VOICED;
	 }
     }
   }  /* do threshold comparisons for full and half rates  */

   /* use 10log10(e_mem->band_power[k]/band_noise_sm[k]) */
   /* as features that could possibly be used for unvoiced detection */

   for(i=0;i<FREQBANDS;i++){
     e_mem->features.band_snr_inst[i] = (100./160.)*10.0*log10
       (e_mem->band_power[i]/e_mem->band_noise_sm[i])+
	 (60.0/160)*e_mem->features.band_snr_inst_last[i];

     e_mem->features.band_snr_inst_last[i] = 10.0*log10
       (e_mem->band_power[i]/e_mem->band_noise_sm[i]);
   }

   /* the maximum rate for all bands is chosen as the rate */
   *rate = e_mem->band_rate[0];
   for(k=1;k< FREQBANDS;k++) {
     if(e_mem->band_rate[k] > *rate) *rate = e_mem->band_rate[k];
   }
   e_mem->features.rate[0] = e_mem->band_rate[0];
   e_mem->features.rate[1] = e_mem->band_rate[1];

   /* Section 2.4.4.1.3 Calculating Hangover Frames as Function of SNR */
   if((e_mem->num_full_frames > FULL_THRESH) ||
      e_mem->hangover_in_progress == 1) {
     /* only use hangover after so many full rate frames in a row*/
     if(*rate != FULLRATE_VOICED) {
       e_mem->hangover+=1;
       if (e_mem->hangover <= hangover[e_mem->snr_map[0]]) {
	 *rate = FULLRATE_VOICED;
	 e_mem->hangover_in_progress = 1;
       }
       else {
	 e_mem->hangover_in_progress = 0;
       }
     }
     else {
       e_mem->hangover_in_progress = 0;
     }
   }

   if((*rate == FULLRATE_VOICED) && (e_mem->hangover_in_progress == 0)) {
     e_mem->hangover=0;
     e_mem->num_full_frames+=1;
   }
   else {
     e_mem->num_full_frames = 0;
   }
   /* end of hangover algorithm     */

   /* assuming only 1/8,1/2,and full rates pluse some 1/2 and full rates modes */
   /* if last rate is full force 1/2 rate before going to 1/8                  */

   /* Section 2.4.4.1.4 Constraints on Rate Selection */
   if((e_mem->last_rate_1st_stage == FULLRATE_VOICED) && *rate == EIGHTH)
     *rate = HALFRATE_VOICED;

   if(e_mem->frame_num == 0)
     *rate = FULLRATE_VOICED;

   if (*rate>control->max_rate) {
     *rate=control->max_rate;
   }
   if (*rate<control->min_rate) {
     *rate=control->min_rate;
   }

   e_mem->last_rate_1st_stage = *rate;
   e_mem->last_rate = *rate;

}/* end of select_mode1() */

int last_solid_voiced = 0;

void select_mode2(
    int                 *rate,
    float               *form_resid,
    float               *acf,
    struct CONTROL      *control,
    struct ENCODER_MEM  *e_mem,
    float               *speech,
    float               *lpc,
    float               *lsp
)
{


   int i, lag;
   int pitch[5];
   int maxlag;
   int unvoiced;
   float max_cor;
   float tmpbuf[FSIZE];

   /* this is the acf of the noise reducing prediction filter */
   float cor[PMAX/DECIMATE_F+1];
   float discrim;
   float resid[(FSIZE+PMAX)/DECIMATE_F];
   float pitch_cor[PITCH_NUM+1];

   /* Section 2.4.4.2 */
   /* now filter the prediction residual and decimate by a factor of 2 */
   /* for computing NACF as a periodicity measure		       */

   do_fir_linear_filter(form_resid, tmpbuf, FSIZE, &(e_mem->decimate), UPDATE);
   for (i=0; i<PMAX/DECIMATE_F; i++) {
     resid[i]=e_mem->resid_mem[i];
   }

   for (i=0; i<FSIZE/DECIMATE_F; i++) {
     resid[i+PMAX/DECIMATE_F]=tmpbuf[i*DECIMATE_F];
   }

   for (i=0; i<PMAX/DECIMATE_F; i++) {
     e_mem->resid_mem[i]=resid[i+FSIZE/DECIMATE_F];
   }

   /* now do open loop pitch prediction */
   maxlag = PMIN/DECIMATE_F;
   max_cor = 0.0;
   for (lag=PMIN/DECIMATE_F; lag<PMAX/DECIMATE_F; lag++) {
     cor[lag] = 0.0;
     for(i=0;i<FSIZE/DECIMATE_F;i++)
       cor[lag] += resid[PMAX/DECIMATE_F+i]*resid[PMAX/DECIMATE_F+i-lag];
     if (cor[lag] > max_cor) {
       max_cor = cor[lag];
       maxlag = lag;
     }
   }
   pitch[0] = maxlag;
   pitch_cor[0] = max_cor;

   /* Calculate ACF and other unvoiced detection parameters */
   compute_features(&pitch_cor[0],&pitch[0],resid,speech,
		&(e_mem->features),acf,lpc,lsp,e_mem->band_power);

   /* Updating Smooth Energy estimate Esmb(i)(k), Eq. (2.4.4.1-3) */
   for(i=0;i<FREQBANDS;i++) {
     e_mem->frame_energy_sm[i] = SMSNR*e_mem->frame_energy_sm[i]
                                    + (1.0-SMSNR)*e_mem->band_power[i];
   }
   /************************************************************************/
   /* Section 2.4.4.2.2 Updating background noise */

   /* Now do the threshold adaptation   */
   /* don't adapt thresholds unless NACF < NACF_ADAP_BGN_THR over ADP frames*/
   /* we're assuming the background noise here doesn't have periodicity in */
   /* the range of typical speech                                          */

   if (e_mem->features.nacf0 < NACF_ADAP_BGN_THR)
     e_mem->adaptcount++;
   else
     e_mem->adaptcount = 0;

   if(e_mem->adaptcount > ADP) {
     /* change the predictor filter for the noise whitening */
     /* noise                                            */
     /* update our background noise estimate */
     for(i=0;i<FREQBANDS;i++) {

       /* just increase background noise until it matches */
       /* lowest stationary period background noise       */
       if(e_mem->band_noise_sm[i]+1 < e_mem->band_noise_sm[i]*INC_FACTOR)
	 e_mem->band_noise_sm[i] *= INC_FACTOR;
       else
	 e_mem->band_noise_sm[i] += 1.0;
     }
     e_mem->adaptcount = ADP + 1;
   }
   else{

   /*if SNR map est. is valid & above SNR_MAP_THRESHOLD let background noise */
   /* estimate slowly inch up like it does in IS-96.  This will allow */
   /* the rate decision to perform as well as IS-96 in babble noise and */
   /* other non-stationary noises    */

     if(e_mem->snr_stat_once ==  1) {  /* snr is valid  */
       for(i=0;i<FREQBANDS;i++){
	 if( (e_mem->snr_map[i] > SNR_MAP_THRESHOLD) &&
	    (e_mem->band_noise_sm[i] < e_mem->frame_energy_sm[i])){
	   if(e_mem->band_noise_sm[i]+1.0 < e_mem->band_noise_sm[i]*IS96_INC)
	     e_mem->band_noise_sm[i] *= IS96_INC;
	   else
	     e_mem->band_noise_sm[i] += 1.0;
	 }
       }
     }
   }

   /* if input energy is lower than noise estimate, */
   /* reduce background noise estimate immediately  */
   for(i=0;i<FREQBANDS;i++) {
     /* Update Background Noise Estimate */
     if(e_mem->frame_energy_sm[i] < e_mem->band_noise_sm[i]) {
       e_mem->band_noise_sm[i] = e_mem->frame_energy_sm[i];
     }
   }

   for(i=0;i<FREQBANDS;i++) {
     if (e_mem->band_noise_sm[i]>HIGH_THRESH_LIM)
       e_mem->band_noise_sm[i]=HIGH_THRESH_LIM;

     if (e_mem->band_noise_sm[i] < LOWEST_LEVEL[i])
       e_mem->band_noise_sm[i] = LOWEST_LEVEL[i];
   }
   /* end of updating backgound noise energy */
   /***************************************************************/


   /***************************************************************/
   /* Section 2.4.4.2.2 Updating Signal Energy Estimate */
   /* pitchrun keeps track of the number of frames in a row with periodicity*/

   if (e_mem->features.nacf0 > NACF_SOLID_VOICED)
     e_mem->pitchrun += 1;
   else
     e_mem->pitchrun = 0;

   if (e_mem->pitchrun > STATVOICED) {  /* decrease the signal energy   */
	                                 /* if we're in a voiced segment */
     e_mem->snr_stat_once = 1;   /* confidence that we have seen speech */

     /* indicates a stationary voiced segment       */
     for(i=0;i<FREQBANDS;i++) {
       e_mem->signal_energy[i] *= SCALE_DOWN_ENERGY; /* *= 0.97 */
     }
   }

   /* assume that the highest energy sounds are speech signals and */
   /* thus the highest energy frames contain speech and can be used */
   /* to define the SNR */

   for(i=0;i<FREQBANDS;i++) {
     /* Update Signal Energy Estimate */
     if(e_mem->frame_energy_sm[i] > e_mem->signal_energy[i])
       e_mem->signal_energy[i] = e_mem->frame_energy_sm[i];

   }

   /* end updating signal energy estimate */
   /************************************************************************/
#if 1
   /* update SNR estimates */
   for(i=0;i<FREQBANDS;i++) {
     e_mem->snr[i] = e_mem->signal_energy[i]/e_mem->band_noise_sm[i];
   }

   /* now map the snr into a set of thresholds mult factors to use */

   if(e_mem->snr_stat_once != 0) {
     /* we have seen some speech and are confident in our SNR measure */
     e_mem->snr_stat_once = 1;

     /* UPDATE THIS FOR GENERAL SNR MAPS! */
     for(i=0;i<FREQBANDS;i++) {
       e_mem->snr_map[i] =(int) rint((10*log10(e_mem->snr[i])-20.0)/5.0);
       if(e_mem->snr_map[i] < 0) e_mem->snr_map[i]=0;
       if(e_mem->snr_map[i] > 7) e_mem->snr_map[i]=7;
     }
   }
   else {
     /* we haven't seen speech, aren't confident in our estimate of the */
     /* signal energy and will use a nominal energy of */
     /* VOICE_INITIAL dB (-18dbm0) for our signal energy */

     for(i=0;i<FREQBANDS;i++) {
       if(i == 0)
	 e_mem->snr_map[i]= (int) rint((VOICE_INITIAL-
				10*log10(e_mem->band_noise_sm[i])-20.0)/5.0);
       else
	 e_mem->snr_map[i]= (int) rint((VOICE_INITIAL_HI-
				10*log10(e_mem->band_noise_sm[i])-20.0)/5.0);


       if(e_mem->snr_map[i] < 0) e_mem->snr_map[i]=0;
       if(e_mem->snr_map[i] > 7) e_mem->snr_map[i]=7;
     }
   }

#endif

   /* Second Stage of Rate Determination Algorithm, Section 2.4.4.3 */
   if(control->reduced_rate_flag > 0 && control->reduced_rate_flag < 4){

   /* using the features of prediction gain, zero crossings, nacf, and */
   /* the target snr from the previous frame make a decision to :      */
   /* 1.) go down to 1/2 rate                                          */
   /* 2.) use 1/4 rate unvoiced mode (no pitch codebook)               */
   /* 3.) use 1/4 rate voiced mode for low amplitude sounds  */

     unvoiced = 0;
     if(control->unvoiced_off == NO){
       /* is frame unvoiced                                                */
       /* gather features and make a voiced/unvoiced decision */

       /* generate linear discriminant to make the voiced/unvoiced */
       /* decision                                                 */

       if((e_mem->features.nacf_frame > 0.5 && e_mem->features.zero < 80) ||
	  (e_mem->features.nacf_frame < 0.25 && e_mem->features.zero < 45 ) ||
	    e_mem->features.pred_gain >= 15.0){
	 unvoiced = 0;
       }
       else{
	 discrim = dis_coef[0]*(e_mem->features.nacf_frame)+
	   dis_coef[1]*(e_mem->features.zero)+
	     dis_coef[3];
	 if(discrim < 0.0){
	   unvoiced = 1;
	   if(*rate != EIGHTH){
	     if(control->reduced_rate_flag == 1){
	       /* reduced rate = 3/4 so unvoiced gets coded at 1/2 rate */
	       *rate = HALFRATE_VOICED;
	     }
	     else
	       { /* reduced rate mode 2 or 3) */
		 if(last_solid_voiced)
		   *rate = HALFRATE_VOICED;
		 else
		   *rate = QUARTERRATE_UNVOICED;
	       }
	   }
	 }
       }
       /* Update last_solid_voiced for next time */
       if(e_mem->features.nacf_frame >= 0.5 && e_mem->features.zero < 60)
	 last_solid_voiced = 1;
       else
	 last_solid_voiced = 0;
     } /* end of unvoiced off switch */


     /*** Keep track of frame energy  ***/
     /* Section 2.4.4.3.2 TEMPORALLY MASKED FRAME DETECTION */

     e_mem->features.frame_energy_db = 10*log10(5.0/8.0*acf[0]+
				      3./8.0*e_mem->features.frame_energy_last);

     /* keep a running track of the current signal energy with a time */
     /* constant of 8 frames or 160 msec                              */

     /*  e_mem->features.voiced_energy is Eavg */
     e_mem->features.voiced_energy = .8825*e_mem->features.voiced_energy +
                                      (1-.8825)*(e_mem->features.frame_energy_db);

     e_mem->features.frame_energy_last = acf[0];

     /* taking advantage of the masking of low amplitude speech
	from recent high energy */
     /* speech if the current speech frame is less than 9 db from the
	running average   */
     /* then code at 1/2 rate, if less than 15 db code at 1/4 rate voiced  */
     /* do all this if the frame isn't unvoiced nor eighth rate      */

     if(*rate != EIGHTH && unvoiced == 0){

       if(e_mem->features.voiced_energy-e_mem->features.frame_energy_db
	                                                     > THR_SIL_1_2 &&
       e_mem->features.diff_lsp < THR_DIFFLSP){
	 if (control->reduced_rate_flag == 1)
	   *rate = HALFRATE_VOICED;
	 else if(e_mem->features.voiced_energy - e_mem->features.frame_energy_db
	                                                    > THR_SIL_1_4)
	   *rate = QUARTERRATE_UNVOICED;
	 else
	   *rate = HALFRATE_VOICED;
       }
       else if(control->reduced_rate_flag == 3 ){/* mode 3 */

	 if(*rate == FULLRATE_VOICED){/* Stationary Mode */

	 /* Section 2.4.4.3.3 STATIONARY VOICED FRAME DETECTION */
	 /* if the previous frame was well matched (target_snr > T) and the */
	 /* current frame shows good periodicity with the past frame and the*/
	 /* prediction gain hasn`t dropped precipitously then               */
	 /* bump the rate down to 1/2 rate                                  */

	   if((e_mem->target_snr > e_mem->target_snr_thr) &&
	      (e_mem->features.nacf_frame > THR_NACF_BUMPUP) &&
	      (e_mem->features.pred_gain_diff < THR_PRED_GAIN )){
	     *rate = HALFRATE_VOICED;
	   }
	 }
	 else{/* This is in the SPEC, however it looks redundant */
	   *rate = HALFRATE_VOICED;
	 }
       }/* end of if (control->reduced_rate_mode == 3) */
     }
   } /* end of if(control->reduced_rate_flag > 0 && control->reduced_rate_flag < 4) */
   else if(control->reduced_rate_flag == 4) {
     if (*rate == FULLRATE_VOICED){ /* force all FULLRATE frames to HALFRATE */
       *rate = HALFRATE_VOICED;
     }
   }
   if (*rate>control->max_rate) {
     *rate=control->max_rate;
   }
   if (*rate<control->min_rate) {
     *rate=control->min_rate;
   }

   /* keep track of the running average rate for voiced frames */
   /* Don't care about EIGHT rate frames */
   if(*rate != EIGHTH){
     /* If long-term average rate is out of acceptable bonds and short term */
     /* rates aren't moving in the correct direction then take action on   */
     /* thresholds                                                         */
     if(e_mem->block_cnt > 0){
       /* update histogram of target_snr for full rates and 1/2 rates */
       /* and update counts for the type of frame                     */

       update_hist_cnt(e_mem, *rate);
       e_mem->block_cnt -=1;
     }
     else  /* the block has expired so make threshold adjustments */
       {
	 e_mem->block_cnt = STATWINDOW;  /*reset counter for the next window*/
	 /* reset histogram and frame counts for next block                 */

	 e_mem->avg_rate = (float)(e_mem->full_cnt*FULLRATE_BPS+
				   e_mem->half_cnt*HALFRATE_BPS+
				   e_mem->full_force*FULLRATE_BPS+
				   e_mem->half_force*HALFRATE_BPS+
		           e_mem->quarter_cnt*QUARTER_BPS)/(float)(STATWINDOW);

	 if(control->reduced_rate_flag == 3){
	   /* was average rate high or low for the last block of frames       */
	   /* if it was out of bounds then adjust threshold, else leave as is */
	   if(e_mem->avg_rate > (1.+RATEM)*e_mem->avg_rate_thr)
	     adjust_rate_down(e_mem);
	   if(e_mem->avg_rate < (1.-RATEM)*e_mem->avg_rate_thr)
	     adjust_rate_up(e_mem);

	   fprintf(stderr,"avg_rate = %f, target_snr_thr = %f\n",
		   e_mem->avg_rate, e_mem->target_snr_thr);

	   if(e_mem->target_snr_thr > HIGH_BND_SNR)
	     e_mem->target_snr_thr = HIGH_BND_SNR;
	   if(e_mem->target_snr_thr < LOW_BND_SNR)
	     e_mem->target_snr_thr = LOW_BND_SNR;
	 }

	 /* keep the total numbers for the file  */

	 e_mem->full_force_t += e_mem->full_force;
	 e_mem->half_force_t += e_mem->half_force;
	 e_mem->full_cnt_t += e_mem->full_cnt;
	 e_mem->half_cnt_t += e_mem->half_cnt;
	 e_mem->quarter_cnt_t += e_mem->quarter_cnt;
	 e_mem->total_speech_frames += STATWINDOW;

	 /* zero out rate counters */
	 e_mem->full_force = e_mem->full_cnt =
	   e_mem->half_force = e_mem->half_cnt = e_mem->quarter_cnt= 0;

	 for(i=0;i< 4;i++)
	   e_mem->hist_below[i] = e_mem->hist_above[i] = 0;
       }
   }
   e_mem->last_rate = *rate;

  if(e_mem->last_rate_1st_stage == EIGHTH && e_mem->last_rate > EIGHTH){
    fprintf(stderr,"error rate after stage 2: rate1=%d rate=%d\n",
	    e_mem->last_rate_1st_stage, e_mem->last_rate);
  }

}/* end of select_mode2() */
