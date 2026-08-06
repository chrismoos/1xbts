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
/* snr.c - compute the snr values                               */

#include <math.h>
#include "celp.h"

void update_snr(
     int         type,
     float       *signal,
     float       *sig_and_noise,
     struct SNR  *snr
)
{
  int i;
  float frame_signal_energy=EPS, frame_noise_energy = EPS;

  snr->num_frames[type]++;

  for (i=0; i<FSIZE; i++) {
    frame_signal_energy+= signal[i]*signal[i];
    frame_noise_energy+= (signal[i]-sig_and_noise[i])*
      (signal[i]-sig_and_noise[i]);
  }

  snr->signal_energy[type]+=frame_signal_energy;
  snr->noise_energy[type]+=frame_noise_energy;

#if 0
fprintf(stderr,"frame_signal_energy = %g\n", frame_signal_energy);
fprintf(stderr,"frame_noise_energy  = %g\n\n", frame_noise_energy);
#endif

  if(frame_signal_energy > 0.0 && frame_noise_energy > 0.0)
    snr->seg_snr[type]+=10.0*log_10(frame_signal_energy/frame_noise_energy);
  else
    fprintf(stderr,"update_snr: Warning: frame_signal_energy = %f, frame_noise_energy = %f\n", frame_signal_energy, frame_noise_energy);

}

void compute_snr(
     struct SNR     *snr,
     struct CONTROL *control
)
{
  float e_snr, e_seg_snr, d_snr, d_seg_snr;

  e_snr=10.0*log_10(snr->signal_energy[ENCODER]/snr->noise_energy[ENCODER]);
  printf("\nEncoder SNR    = %f db\n", e_snr);

  e_seg_snr=snr->seg_snr[ENCODER]/(float)(snr->num_frames[ENCODER]);
  printf("Encoder SegSNR = %f db\n", e_seg_snr);

  if (control->output_encoder_speech==YES) {
    printf("No decoding done.\n");
  }
  else if (control->pf_flag==PF_ON) {
    printf("Decoder SNR meaningless since postfilter is on.\n");
  }
  else {
    d_snr=
      10.0*log_10(snr->signal_energy[DECODER]/snr->noise_energy[DECODER]);
    printf("Decoder SNR    = %f db\n", d_snr);

    d_seg_snr=snr->seg_snr[DECODER]/(float)(snr->num_frames[DECODER]);
    printf("Decoder SegSNR = %f db\n", d_seg_snr);

    if (d_snr!=e_snr) {
      printf("*** Decoder SNR different from Encoder SNR ***\n");
    }
  }

}


void target_reduction(
     float *after_search,
     float *before_search,
     float *energy_before,
     float *energy_after,
     int   length
)
{

  int i;

  *energy_after = 0.0;
  *energy_before = 0.0;
  for(i=0;i<length;i++){
    *energy_before += before_search[i]*before_search[i];
    *energy_after += after_search[i]*after_search[i];
  }


}

void save_target(
     float *target,
     float *gt_target,
     int   length
)
{
  int i;
  for(i=0;i<length;i++)gt_target[i]=target[i];
}

void save_pitch(
     struct PITCHPARAMS  *pitch_params,
     float               *pitch_lag,
     float               *pitch_gain
)
{

  *pitch_lag = pitch_params->lag+(float)(pitch_params->frac)/
                                         (float)(MAX_FR_RESOLUTION);
  *pitch_gain = pitch_params->b;

}

void compute_target_snr(
    int                 mode,
    struct ENCODER_MEM  *e_mem
)
{

  int i;
  float Ex=EPS, Ey=EPS;

  for(i = 0; i < CBSF[mode]; i++){
    Ex += e_mem->codebook_target_energy[i];
    Ey += e_mem->codebook_target_energy_after[i];
  }

  e_mem->target_snr = 10*log_10(Ex/Ey);
}
