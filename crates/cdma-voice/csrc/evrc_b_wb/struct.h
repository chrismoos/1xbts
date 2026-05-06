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

#ifndef _struct_h_
#define _struct_h_

#include "defines.h"
#include "macro.h"
#include "hpf80.h"
#include "upgrades.h"
#include "WB_Buffer_sizes.h"

#include "macro_new.h"
//#include "WB_Buffer_sizes_8thRate.h" 
#include "WB_Analysis_struc.h"
//added by gautam


#include "WBSmartBlanking.h"



typedef struct
{
    float band_noise_sm[FREQBANDS];
    int last_rate;		/* rate decisision after 2nd stage select_mode2() */
    int last_rate_1st_stage;	/* rate decision after 1st stage */
    int last_rate_2nd_stage;	/* rate decision after 2nd stage */
    int num_full_frames;
    int hangover, hangover_in_progress;
    int band_rate[FREQBANDS];
    float band_power[4];
    float signal_energy[FREQBANDS];
    float frame_energy_sm[FREQBANDS];
    int adaptcount;
    int pitchrun;
    float band_power_last[FREQBANDS];
    int snr_stat_once;
    int snr_map[FREQBANDS];
    float snr[FREQBANDS];
    float r_filt[FREQBANDS_NB][FILTERORDER];
    int frame_num;
}
ENCODER_MEM;






struct PACKET
{

    unsigned short LSP_IDX[4];
    unsigned short PACKET_RATE;	//4-Full;3-Half;2-Quarter;1-Eighth
    unsigned short MODE_BIT;	//for the bit rate
    unsigned short DELAY_IDX;
    unsigned short DELTA_DELAY_IDX;
    unsigned short DELAY_ADJ_IDX[4];

    unsigned short ACBG_IDX[3];

    unsigned short FCB_PULSE_IDX[3][4];	//3=NoOfSubFrames
    unsigned short FCBG_IDX[3];
    unsigned short NELP_GAIN_IDX[2][2];
    unsigned short NELP_FID;
    unsigned short SILENCE_GAIN;

    short Q_delta_lag;		/*Delta pitch for QPPP */
    short POWER_IDX;		/*Codebook index for the power quantization for QPPP */
    short AMP_IDX[2];		/*Codebook index for the Amplitude quantization for QPPP */
    unsigned short LSF_IDX[4];	/*Codebook index for the LSF codebooks */

    short delayindex;		/*Pitch for PPP */
    unsigned short F_ROT_IDX;	/*Codebook index for the global alignment for FPPP */
    short A_POWER_IDX;		/*Codebook index for the absolute power quantization for FPPP */
    short A_AMP_IDX[3];		/*Codebook index for the absolute amplitude quantization for FPPP */
    unsigned short F_ALIGN_IDX[17];	/*Codebook index for the alignment shifts for FPPP */
    unsigned short VC_IDX;	/* Voicing cutoff index */

    unsigned short MDCT_IDX[MAX_MDCT_NWORDS];

    unsigned short FrameGain_IDX;
    unsigned short NoiseGain_IDX;
    unsigned short WB_MODE_BIT;

    unsigned short Celp_Mdct_Flag;	//celp or MDCT flag


};


struct PARAM_FRAME_4GVLB
{
    int Mode;
    float Tilt;
    float PitchGain;
    float PitchLag;
    float whtnd_la[160];
};







struct VLB
{
    int ptr;
    float sig_ene_sm[2];
    float sig_ene[2];
    float noise_ene[2];
    float bn_update_factor;
    float bn_min[2][2];
    float bn_max;
    int decay_flag;
    int next_va;
    float prev_snr[2];
    float curr_snr[2];
    float next_snr[2];
};

struct VLBPARAMS
{
    double R[BPFORDER + 1];
    double E[LPCORDER + 1];
    float lpc[LPCORDER];
    int S;
    float band_ene[2];
};

struct POLEZERO_FILTER
{
    int order;
    int numord, denord;
    int ptr;
    double *mem;
};

struct POLE_FILTER
{
    int order;
    int ptr;
    double *mem;
};

struct ZERO_FILTER
{
    int order;
    int ptr;
    float *mem;
};

struct PITCH_FILTER
{
    int order;
    int ptr;
    double *mem;
};

//moved here from filterbank.h


/* structures containing filter states */
struct STATE_ANA_LB
{
    float mem_ana_filt_lb1[ANA_FILT_LEN_LB - 1];
    float mem_ana_filt_lb2[ANA_FILT_LEN_LB - 1];
    ZERO_FILTER state_ana_filt_lb1;
    ZERO_FILTER state_ana_filt_lb2;
};

struct STATE_ANA_HB
{
    float state_ana_filt_hb_1[2 * ALLPASSSECTIONS];
    float state_ana_filt_hb_2[LEN_DN_HB - 2];
    float state_ana_filt_hb_3[2 * ALLPASSSECTIONS_STEEP + 1];
    float state_ana_filt_hb_4[2 * ALLPASSSECTIONS_STEEP + 1];
    double mem_ana_filt_hb_iir[1];
    POLEZERO_FILTER state_ana_filt_hb_iir;
};

struct STATE_SYN_LB
{
    float buf_LB_delay[LB_SYN_DELAY];	/* for low-band overlap-add delay on decoder side */
    float mem_syn_filt_lb[SYN_FILT_LEN_LB - 1];
    float mem_syn_filt_lb_tmp[SYN_FILT_LEN_LB - 1];
    double mem_syn_filt_lb_iir[2];
    ZERO_FILTER state_syn_filt_lb;
    POLEZERO_FILTER state_syn_filt_lb_iir;
};

struct STATE_SYN_HB
{
    float state_syn_filt_hb_0[2 * ALLPASSSECTIONS_STEEP];
    float state_syn_filt_hb_1[2 * ALLPASSSECTIONS_STEEP];
    float state_syn_filt_hb_2[LEN_UP_HB];
    float state_syn_filt_hb_3[2 * ALLPASSSECTIONS + 1];
    double mem_syn_filt_hb_iir1[2];
    double mem_syn_filt_hb_iir2[2];
    POLEZERO_FILTER state_syn_filt_hb_iir1;
    POLEZERO_FILTER state_syn_filt_hb_iir2;
};


/*======================================================================*/
/*         ..Typedefs.                                                  */
/*----------------------------------------------------------------------*/
typedef long int32;
typedef short int16;

typedef struct
{

    char *input_filename;
    char *output_filename;
    int16 encode_only;
    int16 decode_only;
    int16 max_rate;
    int16 max_rate_default;
    int16 min_rate;
    int16 min_rate_default;
    int16 post_filter;
    int16 post_filter_default;
    int16 noise_suppression;
    int16 noise_suppression_default;
    int16 ibuf_len;
    int16 obuf_len;

    int16 highpass_filter;
    int16 olr_calibration;

    int16 fullrate_coding_method;

    int16 unquantized_lsp;
    int16 partial_file_processing;
    int16 starting_frame_num;
    int16 num_frames;
    int16 form_res_out;
    int16 qform_res_out;
    int16 mform_res_out;
    int16 target_speech_out;
    int16 rate_out;
    int16 lag_out;
    int16 nacf_out;
    int16 unquantized_prototype;
    int16 unquantized_zero_rate;
    int16 unquantized_quarter_rate;
    int16 unquantized_half_rate;
    int16 unquantized_full_rate;
    FILE *rfileP;
    char *rate_filename;
    int16 packet_out;
    int16 vad_out;
    int16 forced_count;
    int16 forced_rate[10];
    int16 forced_frame[10];
    int16 accshift_out;
    int16 avg_rate_control;
    float PPP_to_CELP_threshold;
    float avg_rate_target;
    int16 ratewin;
    int operating_point;
    char pattern[10];
    FILE *erasure_file;
    FILE *signalling_file;
    int16 verbose;
    int16 lowband_decout;	//decoder outputs only the lowband

    //Time-Warping: Added for enabling/disabling time-warping
    int time_warping;

    //Added for Support of Phase Matching/Warping
    int phase_matching;
    int double_erasures_pm;
    //End:Added for Support of Phase Matching/Warping
    int Fsop;
    int Fsinp;
    int dtx;

}
EvrcArgs;

class DTFS
{

  public:
    float a[MAXLAG_WI];
    float b[MAXLAG_WI];

    int lag;
      DTFS ();			//constructor
    DTFS operator= (DTFS);
    DTFS operator+ (DTFS);
    DTFS operator- (DTFS);
    DTFS operator* (float);
    int operator== (DTFS X2);
    float operator^ (DTFS);
    void print ();
    void print_time ();
    void print_time (float lband, float hband);
    void to_fs (const float *, int);
    void fs_inv (float *, int, float);
    void phaseShift (float);
    void zeroPadd (int);
    void zeroInsert (int);
    float alignment (DTFS, float);
    unsigned int glbl_alignment_QPPP (DTFS, float, int *, unsigned int);
    float alignment_fine (DTFS, float);
    float alignment_extract (DTFS, float, const float *);
    float alignment_full (DTFS, int);
    unsigned int glbl_alignment_FPPP (DTFS, int);
    float alignment_weight (DTFS, float, const float *, const float *);
    double freq_corr (DTFS, float, float);
    void transform (DTFS, const float *, float *, int);
    void zeroFilter (const float *, int);
    void poleFilter (const float *, int);
    float getEngy ();
    float getEngy (float, float);
    float setEngy (float);
    float getSpEngyFromResAmp (float, float, const float *);
    void deNormalizeResAmp (float, float, float, float, float, const float *);
    void randph ();
    void car2pol ();
    void pol2car ();
    float setEngyHarm (float, float, float, float, float);
    float SCR (DTFS);
    float getSCR (const float *, int);
    void injectnoise (float);
    void adjustLag (int);
    bool quant_cw (int, const float *, int &, int *, float &, float &,
		   float *);
    void quant_cw_memless (const float *, int &, int *);
    void dequant_cw (int, int &, int *, float &, float &, float *);
    void dequant_cw_memless (int &, int *);

    void to_erb (float *);
    void erb_inv (float *, int *, float *);
    void sortamp (float *outfreq, int M);
    float alignment_band (DTFS X2, float Eshift, float lband, float hband,
			  float, float);
    void phaseShift_band (float ph, float lband, float hband);
    unsigned int align_band_FPPP (DTFS X2, float Eshift, float lband,
				  float hband, float samp_resol,
				  int num_steps);
    int alignment_CB (DTFS X2, float Eshift, const float *border,
		      double *CB, int DIM, int SIZE, float *SNR);
    void peaktoaverage (float *, float *);
};




class FGV_MEM
{
  public:

    int16 buf16[2 * SPEECH_BUFFER_LEN];
    int16 *buf16P;
    float buf[SPEECH_BUFFER_LEN + LOOKAHEAD_LEN];
    float *bufP;

    short hb_ana_delay;

//member vars

    float Scratch[SubFrameSize + 6];
    short SScratch[6];		/* scratch short memory */

    float lsp[ORDER];		/* Correlation coefficients                */
    float lspi[ORDER];		/* Interpolation of correlation coeff      */


    float *ExconvH;
    float pci[ORDER];		/* Interpolated prediction coefficients    */
    float pci_wb[ORDER];	/* lpc coefficients for wideband signal */

    float *gnvq;		/* Quantization table for fcb gain */

    short idxppg;		/* Pitch gain c.b. - selected gain index   */
    short idxcb;		/* Shape c.b. - selected shape index       */
    short idxcbg;		/* FCB gain c.b. - selected gain index     */

    int bit_rate;		/* Current coding scheme for encoder/decoder */
    int16 rate;
    int PACKET_RATE;		/* Current bitrate for encoder/decoder */
    int MODE_BIT;		/* Used to distinguish between CELP(0) and FPPP(1),
				   NELP(0) and QPPP(1) */

    int FCBGainSize;		/* Current Fixed Codebook Gain Size */
    float delay;		/* current frames delay */
    short LPCflag;		/* LPC spectral transistion flag */

    int fcbIndexVector[10];	/* ACELP fixed codebook index vector */
    short PackWdsPtr[2];	/* Pointer to current TX/RX receive word */
    short PackedWords[PACKWDSNUM];	/* TX/RX memory */

    short PktPtr[2];
    short TxPkt[PACKWDSNUM];
    short RxPkt[PACKWDSNUM];

    short *nsize, *nsub, *lognsize, knum;
    float *lsptab;

    double mod (double x)
    {
	if (x > pi)
	    x -= pi2;
	if (x <= -pi)
	    x += pi2;
	return x;
    }

    int Q_delta_lag;		/*Delta pitch for QPPP */
    int POWER_IDX;		/*Codebook index for the power quantization */
    int AMP_IDX[2];		/*Codebook index for the Amplitude quantization */
    unsigned short LSF_IDX[3];	/*Codebook index for the LSF quantization */

    unsigned int F_ROT_IDX;	/*Codebook index for the global alignment for FPPP */
    int A_POWER_IDX;		/*Codebook index for the absolute power quantization for FPPP */
    int A_AMP_IDX[3];		/*Codebook index for the absolute amplitude quantization for FPPP */
    unsigned int F_ALIGN_IDX[17];	/*Codebook index for the alignment shifts for FPPP */

    float SANITY[10];
    float delay1;
    float pdelay;

    short MDCT_NPULSES;
    short MDCT_NWORDS;
    short MDCT_REMAINDER;

    float mdctcoeff[160];
    short celp_mdct_dec;

    short prev_celp_mdct_dec;	//for memory updates in music_mode.cc

    long encode_fcnt;		/* Frame counter */


    /*specific to narrowband */

    short phase_offset;
    short run_length;

    float FfiltMem[ORDER];

    float last_delayBB;
    short prev_celp_erasure;

    float last_music_signal_sf[54];

    double preemphmem[1];

    float HPspeech[GUARD + FrameSize + LOOKAHEAD_LEN];	/* changed by J. Huang on 12/09/98, then changed on 01/19/00 */

    float ConstHPspeech[GUARD];	/* temporary buffer to store HPspech  */
    float OldlspE[ORDER];	/* Last frame quantized lsp                */
    float OldOldlspE[ORDER];	/* Last Last frame quantized lsp           */

    float ppvq_mid[ACBGainSize - 1];	/* Intermidiate values in ppvq */
    float lsp_nq[ORDER];	/* Correlation coefficients                */
    float Oldlsp_nq[ORDER];	/* Last frame quantized lsp                */
    float lspi_nq[ORDER];	/* Interpolation of correlation coeff      */
    float pci_nq[ORDER];	/* Interpolated prediction coefficients    */
    float wpci[ORDER];		/* Interpolated weighted prediction coefficients */

    float Excitation[ACBMemSize + SubFrameSize + EXTRA];
    int FrameRateMemory[3];

    float H[Hlength + 1];	/* Impulse response [Hlength]              */
    float HtH[Hlength + 1];	/* Impulse response ^2 [Hlength]           */
    float SynMemoryM[ORDER];	/* weighted speech synthesis filter memory */
    float SynMemoryM_bup[ORDER];	/* weighted speech synthesis filter memory */

    float TARGET[SubFrameSize + 1];	/* Residual - Zero input response       */
    float TARGETw[SubFrameSize + 1];	/* weighted target */

    float WFmemFIR[ORDER];	/* Weighting filter memory                 */
    float WFmemIIR[ORDER];	/* Weighting filter memory                 */

    float WFmemFIR_bup[ORDER];	/* Weighting filter memory                 */
    float WFmemIIR_bup[ORDER];	/* Weighting filter memory                 */


    float zir[SubFrameSize];	/* Zero Input Response (can share memory w/ HtH) */
    float zir_bup[SubFrameSize];	/* Zero Input Response (can share memory w/ HtH) */


    float residual[GUARD + FrameSize + LOOKAHEAD_LEN];
    float residualm[SubFrameSize + EXTRA];

    float mspeech[FSIZE];
    float wspeech[FSIZE];
    float residualm_frame[FrameSize + EXTRA];

    float prev_synth[80];
    float prev_synthSave[80];

    float origm[SubFrameSize];
    float *worigm;		/* shared weighted original memory */

    float accshift;
    float UBExcitationMusic[160];


    float beta, beta1;
    short dpm;

    float LPCgain;		/* used for frame erasures */
    short shiftSTATE;
    short lastrateE;		/* last bitrate used for encoder */

    int LAST_PACKET_RATE_E;
    int LAST_MODE_BIT_E;

    float fcbGain;		/* ACELP fixed codebook gain */
    float y2[55];		/* Filtered innovative vector (debug only) */

    float lastLgainE;		/*Previous gain value for the low band */
    float lastHgainE;		/*Previous gain value for the high band */
    float lasterbE[NUM_ERB];	/*Previous Amplitude spectrum (ERB) */

    float cbprev_E[ORDER];
    float cbprevprev_E[ORDER];

    int cod3_10_offset[3];	// added to sync with FGV_NB

    /*======================================================================*/
    /*  Lucent Technologies Network Wireless Systems                        */
    /*  EVRC Floating-point C Simulation.                                   */
    /*                                                                      */
    /*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
    /*  reserved.                                                           */
    /*----------------------------------------------------------------------*/
    /*  Module:     decoder globs                                           */
    /*----------------------------------------------------------------------*/
    /*  History:                                                            */
    /*     11/01/94  Written By Dror Nahumi, AT&T                           */
    /*----------------------------------------------------------------------*/
    
    /*======================================================================*/
    /*         ..Globals (decoder).                                         */
    /*----------------------------------------------------------------------*/
    long decode_fcnt;		/* Frame counter */
    int ER_counter;
    float ER_exct_nrg_avg;
    float attn_fac;

    float OldlspD[ORDER];	/* Last frame quantized lsp                */
    float OldOldlspD[ORDER];	/* Last last frame quantized lsp           */

    float PitchMemoryD[ACBMemSize + SubFrameSize * 2 + EXTRA];
    float PitchPreFiltMemoryD[ACBMemSize + SubFrameSize + EXTRA];
    float PitchMemoryD2[ACBMemSize], PitchMemoryD3[ACBMemSize];

    //Time-Warping: Size of DECspeech doubled below
    float DECspeech[SubFrameSize * 2];	/* Output decoder speech           */

    //Time-Warping: Size of DECspeechPF doubled below
    float DECspeechPF[SubFrameSize * 2];	/* Output decoder speech with post filter */

    //Time-Warping: Added for passing lower band pitch to high band decoder
    float LB_delay;

    float SynMemory[ORDER];	/* Synthesis filter's memory             */

    float SynMemory_bbg[ORDER];	/* Synthesis filter's memory             */
    float PitchMemoryD_bbg[ACBMemSize + SubFrameSize + EXTRA];
    float DECspeech_bbg[SubFrameSize];	/* Output decoder speech */
    float DECspeechPF_bbg[SubFrameSize];	/* Output decoder speech with post filter */

    short update_bbg_reenc_flag;
    short update_bbg_flag;
    short bbg_dtx_likely_flag;
    float bbg_dtx_likely_bgnenergy;

    /* REMOVE THIS COPY BY SHARING WITH ENCODER'S HAMMING WINDOW.
     * TO DO THAT MAKE SURE IT IS SET IN AN INIT FUNCTION INSTEAD
     * OF INSIDE pre_encode(). */
    float HammingWindow_bbg[LPC_ANALYSIS_WINDOW_LENGTH];	/* Window coefficients */

    short erasureFlag;
    short errorFlag;
    short lastrateD;

    short lastlastrateD;
    float pdelayD;
    float pdelayD2, pdelayD3;
    short pdeltaD;
    short fer_counter;

    int LAST_PACKET_RATE_D;
    int LAST_MODE_BIT_D;

    short kfer_flag;
    short prev_frame_error;
    float FadeScale;
    float ave_acb_gain;
    float ave_fcb_gain;
    float ave_acb_gain_back;

    short last_valid_rate;	/* last valid decoder rate of operation */

    float lastLgainD;		/*Previous gain value for the low band */
    float lastHgainD;		/*Previous gain value for the high band */
    float lasterbD[NUM_ERB];	/*Previous Amplitude spectrum (ERB) */

    float cbprev_D[ORDER];

/* Memory needed for the moving average LPC Quantization */
    float cbprevprev_D[ORDER];

//gautam:
    struct PACKET data_packet;


//stats and globals not inside globs.cc
    float prevpD[ACBMemSize];
    char PPP_MODE_D;
    int rcelp_half_rateD;
    int prev_rcelp_half;
    char LAST_PPP_MODE_D;
    int PPP_BUMPUP;
    int pppcountE;
    char PPP_MODE_E;
    char LASTLAST_PPP_MODE_E, LAST_PPP_MODE_E;
    float prev_cw_en;
    float cbprevprev_E2[LPCORDER], cbprev_E2[LPCORDER];
    DTFS currp_nq, prev_cw_E, prev_cw_D;
    float scr, ph_offset_E, ph_offset_D;
    float SANITYCHECK[FSIZE];

    float prev_qmdct[160];	//previous decoded MDCT coefficients for erasure processing
    float norm_fade_fac, prev_norm;
    float prev_noise_gain;

    int total_fer_counter;




    int rcelp_half_rateE, prev_dim_and_burstE, dim_and_burstE;

    float SynMemoryM2[ORDER];
    float WFmemFIR2[ORDER];
    float WFmemIIR2[ORDER];
    float Excitation2[ACBMemSize];
    float residualm2[EXTRA];
    short shiftSTATE2, dpm2;
    float accshift2;
    float bufferm2[ACBMemSize];
    float delay2;

    float prev_en[ACBMemSize];

    float NS_SNR;


    float mdct_hist_cnt, celp_hist_cnt;

    float bufferm[ACBMemSize + SubFrameSize + EXTRA];
    short acbevrcFirstTime;

    long celpErasureSeed;
    long celpSPL_HCELPSeed;

    float nelp_gain_mem;

    float dec_lookahead[80], dec_lookahead_wonoise[80];
    float dec_FormantFilterMemory[ORDER];
    double dec_preemphmem[1];

    long dec_MusicModeNoiseSeed;

    float last_delay, last_beta;

    /* primary copy of filter memory */
    float memA[ORDER], memA1[ORDER], memA2[ORDER];
    float memA_bup[ORDER], memA1_bup[ORDER], memA2_bup[ORDER];	//backup for celp mdct switch

    /* secondary copy of filter memory for half rate processing */
    float memB[ORDER], memB1[ORDER], memB2[ORDER];
    int zeroInputFirstTime;	/* init flag - sim only */

    short autocorrelationFirstTime;
    float w[ORDER + 1 + 6];	/*...add 6 for rate determination... */
    float HammingWindow[LPC_ANALYSIS_WINDOW_LENGTH];	/* Window coefficients */

    HPFMem hpfmem;
    HPFMem hpfmem8k;
    HPFMem z;			//for the struct inside new_mode.cc

    int get_nacf_at_pitch_FirstTime;
    float lpfr[LPFRMEM_LEN + (FSIZE + LOOKAHEAD_LEN) / SUBSAMP];
    float rlpf_filt_mem[RLPF_ORDER];
    double rhpf_filt_mem[RHPF_ORDER];


    short nelp_enc_seed;
    short nelp_dec_seed;
    short nelp_erasure_seed;

    //can avoid some redundancy by making a separate enc and dec struct
    double bp1_filt_mem[12];
    double shape1_filt_mem[10];
    double shape2_filt_mem[10];
    double shape3_filt_mem[10];
    double txlpf1_filt1_mem[10];
    double txlpf1_filt2_mem[10];
    double txhpf1_filt1_mem[10];
    double txhpf1_filt2_mem[10];

    double bp1_filt_mem_dec[12];
    double shape1_filt_mem_dec[10];
    double shape2_filt_mem_dec[10];
    double shape3_filt_mem_dec[10];

    double bp1_filt_mem_erasure_dec[12];
    double shape1_filt_mem_erasure_dec[10];
    double shape2_filt_mem_erasure_dec[10];
    double shape3_filt_mem_erasure_dec[10];

    float ave_rate;
    int numactive;
    float AV_TH;
    int NUMFRAMES[4];		//zero,quarter,half,full



//some diagnostic globals
    float global_lsp[ORDER];
    float copy[FrameSize];
    float sanity_check;

    float Eprev;
    float Eavg;
    float LOWVOICEDTH;		//0.6
    float VOICEDTH;		//0.75 
    float UNVOICEDTH;
    float SNRTH;
    int mode_decision_FirstTime;

    double lpf_filt_mem[12];
    double hpf_filt_mem[12];

    int FirstHVframe;

    int vcount;
    float vE[3], vEav;
    float vEprev;
    int prev_voiced, prev_mode;

    float prev_snr_diff;
    float prev_snr[2], prev_ns_snr[2];
    float curr_snr[2];

    int real_fft_first;

    int noise_suprs_first;

    float pre_emp_mem, de_emp_mem;

    float ch_enrg[NUM_CHAN];

    float ch_noise[NUM_CHAN];

    float overlap[FFT_LEN - FRM_LEN];	/* initialized to 0 automatically */

    float ch_gain[FFT_LEN / 2];

    int update_cnt;

    float window[DELAY + FRM_LEN];
    float window_overlap[DELAY];

    int hyster_cnt;		/* forced update statics... */
    int last_update_cnt;
    float ch_enrg_long_db[NUM_CHAN];
    float ns_snr_threshold;
    float ns_neg_snr_mean;
    float ns_neg_snr_var;
    float ns_neg_snr_bias;
    float ns_snr_previous;
    int ns_update_freeze_count;
    int ns_subframe_count;


    unsigned long frame_cnt;

    short evrc_rate;

    float DECbuf[FrameSize / 4];
    short lastgoodpitch;
    float lastbeta;
    float olp_memory[3];	//for olpitch
    int fndppf_FirstTime;


    float PF_mem_syn_pst[ORDER];
    float FIRmem[ORDER];
    float mem_pre;
    float past_gain;
    short post_filter_first_time;

    float Residual[ACBMemSize + SubFrameSize * 2];


    short modifyorig_FirstTime;
    float modifyorig_a1[RRESOLUTION];
    float modifyorig_a2[RRESOLUTION];
    float modifyorig_a3[RRESOLUTION];
    float Table[8 * (2 * 16 + 1)];	/* Largest practical size */
    float Table1[8 * (2 * 16 + 1)];	/* Largest practical size */
    float factor1;
    float factor2;
    short update_background_first;
    short select_rate_first;
    float (*THRESH_SNR)[TLEVELS][2];
    int *hangover;
    ENCODER_MEM rate_mem;
    ENCODER_MEM *e_mem;


    long silence_erasure_seed;
    int iset;
    float gset;
    long GetExc800bps_Seed;
    float GetExc800bps_Sum[NoOfSubFrames];
    float maxSFEnergyQLast;
    long GetExc800bps_dec_Seed;
    float GetExc800bps_dec_Sum[NoOfSubFrames];
    short PrevBest;
    float maxSFEnergyQLast_dec;

    float nacf_ap[5];
    float prev_nacf, nacf;

    float curr_ns_snr[2], next_ns_snr[2][2];
    short PREV_LSP_FLAG;
    short prev_prev_frame_error;

    float MusicResidual[320];

    float E_bgn[BBG_NUM_CHAN + 2];
    float E_bgn_smooth[BBG_NUM_CHAN + 2];
    float E_ch_mem[BBG_NUM_CHAN + 2];
    float bbg_window_overlap[BBG_DELAY];
    unsigned long BBG_frmSkipCnt;
    unsigned long BBG_frmSkipCnt2;
    float resmem[ORDER];
    unsigned long consec_eighth_rates;

    /*==================================================*/
    /*      BAD-RATE CHECK variables                    */
    /*==================================================*/
    short zrbit[2];

    short BAD_RATE;

    short patterncount;
    short int pattern_m;

    long ave_rate_kbps;

    short WB_COUNT;
    short NB_COUNT;
    short ones_dec_cnt;
    short rate_last;
    short last_wb_mode_bit;


    /* some UB related vars in NB code */

    float mem_BB[4];

    double fcb_target_lpfmem[4];
    double mdct_orig_lpfmem[4];

    double mdct_resid_lpfmem[4];

    double fcb_filt_mem[4];
    double fcb_filt_mem_dec[4];


    /* some UB vars from main_final.cc */

    float lookahead_UB[LOOKAHEAD_LEN * 7 / 8];
    float buf_HB_out[SPEECH_BUFFER_LEN * 7 / 8];

    float buf_WB14_out[SPEECH_BUFFER_LEN * 7 / 4];
    int initE;
    int initD;

    //Time-Warping: Increased Size of buf_float below to support warping
    float buf_float[4 * SPEECH_BUFFER_LEN + DELAY];
    float buf_tmp2[4 * SPEECH_BUFFER_LEN];
    float buf_HB[SPEECH_BUFFER_LEN * 7 / 8 + LOOKAHEAD_LEN * 7 / 8 + HB_ANA_DELAY_NS_LB];	/* sampled at 7 kHz */
    float buf_WB14[SPEECH_BUFFER_LEN * 7 / 4 + LOOKAHEAD_LEN * 7 / 4 + HB_ANA_DELAY_NS_LB * 2];	/* sampled at 14 kHz */

    /* structures for filter banks */
    STATE_ANA_LB S_ana_lb;
    STATE_ANA_HB S_ana_hb;
    STATE_SYN_LB S_syn_lb;
    STATE_SYN_HB S_syn_hb;

    QUANTIZED_HB Dec_Qp_HB;
    HB_INDICES dec_indices;
    UnQUANTIZED_HB_8thRate decP_HB_8R;

    UnQUANTIZED_HB P_HB;	//structure to hold unquantized upper band parameters

    //Time-Warping: Increased size of buf_out
    float buf_out[SPEECH_BUFFER_LEN * 2];
    int erasure_8R;
    float buf_backup[SPEECH_BUFFER_LEN + LOOKAHEAD_LEN];


    float worig_celp[160], wsynth_celp[160];
    float worig_mdct[160], wsynth_mdct[160];

// member functions below

    EvrcArgs args_storage;
    EvrcArgs *args;
    int16 ibuf_len;
    int16 obuf_len;
    int write_accshift;

    float q_resid[FrameSize];
    float prev_uvgain;
    int encode_HO;
    int dtx_posn_prev_NER;
    int dtx_ft;
    float dtx_prev_refl0;
    float dtx_flag;
    float dtx_ra_deltasum;
    float dtx_running_avg;
    short dtx_nocompute;

    float sum_sm;
    long silence_Seed;
    float lspi_sm[ORDER];
    int NASS;
    int rcelp_fail;

    float Er;
    float Erq;
    float Erq_ln;
    short idxe;
    float ckck;
    int sfcnt;
    int da_index[NoOfSubFrames];

    float wb_agc;
    float wb_agc_sm;
    double wb_agc_lpfmem[2];
    int quantize_wb_prev_mode;
    int wb_enc_previous_mode;
    short decode_DTX_STATE;
    int celp_mdct_firsttimehere;
    short celp_mdct_prev_dec;

    int total_bits_packed;
    int bit_rate_totals[25];
    int bit_rates;

    float celp_hnw_zir_state[ACBMemSize + SubFrameSize];
    float celp_hnw_state[ACBMemSize + SubFrameSize];
    long celp_hnw_encode_fcnt_last;
    int celp_hnw_last_subframesize;
    float celp_delay_x0;
    float celp_delay_x1;
    float celp_ppf_state[ACBMemSize + SubFrameSize];
    long celp_ppf_encode_fcnt_last;
    float celp_gppf_sm;
    float celp_env1;
    float celp_env2;

    void pre_encode (float *inFbuf, float *Rs);
    void encode (short &, short *);

    //Time-Warping: Added new call for decode
    void decode (short *, short, short, float *, float, short *);
    void celp_encoder (int);

    void music_encoder ();

    //void mdct(float*, float*, short);
    void get_rcelp ();

    void celpmdct_resid_calc (int bit_rate);

    void Update_CELPMEM ();
    void music_decoder (float *outFbuf);



    //Time-Warping: Added new call for celp_decoder
    short celp_decoder (float *, short, int, float, short *);
    void celp_erasure_decoder (float *, short);
    void music_erasure_decoder (float *);
    void silence_erasure_decoder (float *, short);
    void nelp_erasure_decoder (float *, short);
    void voiced_erasure_decoder (float *, short);
    void Weight2Res (float *, float *, float *, float *, float, float, short,
		     short);
    void weight (float *awght, float *a, float gammma, short order);
    void ImpulseRzp (float *output, float *coef_uq, float *coef, float gamma1,
		     float gamma2, short order, short length);

    void silence_encoder (void);
    void voiced_encoder (float *out, int &rate);
    void nelp_encoder (float *out);

    void silence_decoder (float *, short);

    //Time-Warping: Added new call for nelp_decoder
    void nelp_decoder (float *, short, float, short *);
    short voiced_decoder (float *outFbuf, short post_filter, short run_length,
			  short phase_offset, float time_warp_fraction,
			  short *obuf_len);
    short get_rcelpmres_for_voiced (float *mres, float *targ_speech,
				    float *acc3);
    void jumptofullcelp (int &rate);
    void fer_processing (float *out, short pf, short prev_rate);


    void celp_mdct_discriminator (short *decision);

    void InitEncoder ();
    void InitEncoder_1 ();
    void InitDecoder ();
    void InitDecoder_1 ();
    void ppp_full_encoder (DTFS *, DTFS, const float *);
    bool ppp_quarter_encoder (DTFS *, DTFS, DTFS, const float *, DTFS *);
    void ppp_full_decoder (DTFS *, const float *);
    void ppp_quarter_decoder (DTFS *, DTFS, const float *);

    void GetExc800bps (float *output, short *best, float scale, float *input,
		       short length, short flag, short n);
    void GetExc800bps_dec (float *output, short length, short best,
			   short flag, short n);

    void cod3_10jcelp (float xn[], float h[], int pit_lag, float pit_gain,
		       int l_subfr, float cod[], float *gain_code,
		       int *indices);
    void dec3_10jcelp (int pit_lag, float pit_gain, int l_subfr, int *indices,
		       float cod[]);

    //MODE new_mode_decision(int, int, int, float, float *,float*, float*,float*);
    MODE new_mode_decision (float *in);

    void getgain (float *exctation, float *lambda, float *H, short *idxcb,
		  float *gcb, float *gcb_mid, short gcb_size, short Quantize,
		  float *mresidual, short subframel, short hlength);
    void ACELP_Code (float xn[],	/* (i)     :Target signal for codebook.       */
		     float res2[],	/* (i)     :Residual after pitch contribution */
		     float h1[],	/* (i)     :Impulse response of filters.      */
		     int T0,	/* (i)     :Pitch lag.                        */
		     float pitch_gain,	/* (i)     :Pitch gain.                       */
		     int l_subfr,	/* (i)     :Subframe lenght.                  */
		     float code[],	/* (o)     :Innovative vector.                */
		     float *gain_code,	/* (o)     :Innovative vector gain.           */
		     float y2[],	/* (o)     :Filtered innovative vector.       */
		     int *index,	/* (o)     :Index of codebook + signs         */
		     int choice	/* (i)     :Choice of innovative codebook     */
		     /*          0 -> 34 bits                      */
		     /*          1 -> 35 bits                      */
		     /*          2 -> 36 bits                      */
		     /*          3 -> 37 bits                      */
		     /*          4 -> 10 bits                      */
		     /*          5 -> 11 bits                      */
	);

/* New F_P_7_35 search */
    void cod7_35 (float *xw,	/* (input)  Target signal. */
		  float *x,	/* (input)  Target signal (through inverse weighting filter). */
		  int l_subfr,	/* (input)  Length of current subframe. */
		  float *h,	/* (input)  Weighted filter impulse response. */
		  float *ck,	/* (output) Multipulse excitation. */
		  float y[],	/* (output) Filtered fixed codebook excitation. */
		  float *gain,	/* (output) Excitation gain parameter. */
		  int *indx);	/* (output) Factorial Packed codeword. */

    void ComputeACB (float *residualm, float *excitation, float *delay,
		     float *residual, short guard, short *dpm,
		     float *accshift, float beta, short length, short rshift);
    void mod (float *residualm, float *accshift, float beta, short shiftr,
	      short resolution, float *exctation, float *Dresidual,
	      float *residual, short guard, short *dpm, float delay,
	      short subframel, short extra);

    void ZeroInput (float *output, float *coef_uq, float *coef, float *in,
		    float gamma1, float gamma2, short order, short length,
		    short type);
    void lpcanalys (float *pc, float *rc, float *input, short order,
		    short len, float *, float *window);
    void autocorrelation (float *r, float *winput, short len, short order,
			  float *window);
    void durbin (float *r, float *rc, float *pc, short order);

    float get_nacf_at_pitch (float *resid, int pitch0, int pitch1,
			     float *nacf);
    void update_average_rate (int rate);	//valid rates are 1,2,3,4

    void real_fft (float *farray_ptr, int isign);
    void noise_suprs (float *, float *);
    void noise_suprs_8k (float *, float *);	// <<<<<<<

    void fndppf (float *delay, float *beta, float *buf, short dmin,
		 short dmax, short length);
    void Post_Filter (float *syn, float *Lsp, float *Az, float *synpf,
		      float delayi, float agc_fac, float mu, short l_subfr);


    void Init_Post_Filter (void);
    void preemphasis (float *signal, float g, int16 L);

    void agc (float *sig_in, float *sig_out, float *res_out, float agc_fac,
	      int16 l_trm);
    void ltf (float *res, float delayi, float ltgain, short len);
    void bl_intrp (float *output, float *input, float delay, float factor,
		   short fl);
    void modifyorig (float *residualm, float *accshift, float beta,
		     short *dpm, short shiftrange, short resolution,
		     float *TARGET, float *residual, short dp, short sfend);
    void update_background (short rate, ENCODER_MEM * e_mem, float beta);
    void select_rate (float *R_interp,
		      short max_rate, short min_rate, float beta, float HBnrg);

    void acb_excitation (float *Ex1, float gain, float *delay3,
			 float *PitchMemory, short length);
    void putacbc (float *exctation, float *input, short dpl, short subframel,
		  short extra, float *delay3, float freq, short prec);
    float ran_g (long *seed0);
    struct PARAM_FRAME_4GVLB P_LB;	//={Mode,Tilt,PitchGain,PitchLag,whtnd_la};
    void bass_boost (float *speech, float lag, int len);

    short bad_rate_check (short opt, short *);


    /* UB specific stuff inside globs.cc */
    float frame_shift[3];

    float LB_Excitation[160];
    float PitchGain;

    /* UB state for 2ms overlap add */
    float Synstate[14];
    float Synstate14[28 + 48];




//#ifndef FGV_NB

    /* UB static variables */
    /* de-click */
    int declick_frame_FirstTime;
    int start_dsLP, start_dsLP_BW, start_dsHP, start_dsHP_BW;
    struct POLEZERO_FILTER LB_filter_state;
    struct POLEZERO_FILTER HB_filter_state;
    float xLB_hpf_frame[LB_FRAMESIZE + LOOKAHEAD_8 - 1],
	xHB_hpf_frame[UB_FRAMESIZE + LOOKAHEAD_7 - 1];
    float LBenvLPdB_frame_save[4], LB2nd_der_frame_save,
	HBenvLPdB_frame_save2[4], HBenvLPdB_frame_save[4];
    float pulse_msr[DS_ATTN_SIZE], excess_lvl_frame[DS_ATTN_SIZE];
    int smooth_LB_ini;
    int smooth_HB_ini;
    int comp_shift;


    /* wideband encoder */
    float x_frame[BUFFER_LENGTH_7];	/* Vector to hold the input signal */
    float x6_frame[BUFFER_LENGTH_7];	/*vector to hold the 3KHz filtered signal */
    float prev_lsps[LPC_ORD_WB];
    float GainState;
    double filt_mem[LPF_NR_ORD];
    double saved_mem[LPF_NR_ORD];	/* saves the memory the of the filter for subsequent frames */

    /* the following vars are used to set up the GEN_SHP_NOISE struct
       in both wideband encode &  NoiseSynthesis  /*
       /* called from main_final using different classes */

    float mem1[6];

    double rhpf_filt_mem1[2];
    float state[LEN_DN_HB - 2];
    float memup[4];
    float memdown[7];
    double rapf_filt_mem[18];
    float memdown1[7];
    float stateExc[4];
    float noise_iir[1];
    float stateExcW[40 + 14];
    long WB_EncParams_Seed;
    long WB_Enc_Seed_Store;

    /* Quantize_WB_Params.cc  */
    float d_lsfs[6];

    /* wideband_encoder_8thRate.cc */
    float x14_frame[BUFFER_LENGTH_8R];	/* Vector to hold the filtered input signal */
    float prev_lsps_ER[LPC_ORD_WB_8R];

    double lpf_filt_mem1[LPF_8R_NR_ORD];
    double hpf_filt_mem1[LPF_8R_NR_ORD];
    long WB_DecParams_8thRate_Seed;
    float prev_lsfs[10], prev_gain;

    /* WB_Erasure_processing.cc */
    int MaxGoodCountNeeded, ErasureCount, GoodCount;
    int MaxErasureCount;
    float prev_LSFs[6], prev_Gain[5], prev_GainFrame;

    /* NoiseSynthesis.cc */
    long noise_synthesis_Seed;
    long noise_synthesis_Seed_Store;
    short prev_mode1;

    short autocorrelation_ExtWin_FirstTime;

    int WB_encoder_first_time;
    int WB_decoder_first_time;

    int SPL_HCELP_WB;
    short SPL_HCELP;
    short SPL_HPPP;
    short SPL_HNELP;
    short N_consec_ers;

    int go_back_input;


    // Smart blanking members
    WBSBEncoder SB_enc;
    WBSBDecoder SB_dec;

    short SILFrame;
    short prev_SILFrame;
    short last_last_last_rateD;
    short last_last_rateD;
    short INTERP;
    short N_inter;
    float final_lsp[ORDER];
    float init_lsp[ORDER];


    float init_gain;
    float final_gain;
    short NG_inter;
    short G_INTERP;
    short noSID;
    short lpcgflag;
    float prev_sum;

/* filterbank initialization functions */
    void filterbank_init_ana_lb (STATE_ANA_LB * State);
    void filterbank_init_ana_hb (STATE_ANA_HB * State);
    void filterbank_init_syn_lb (STATE_SYN_LB * State);
    void filterbank_init_syn_hb (STATE_SYN_HB * State);

/* filterbank processing functions */
    void filterbank_ana_lb (float *out, const float *in, int16 len,
			    STATE_ANA_LB * State);
    void filterbank_ana_hb (float *out_7k, float *out_14k, const float *in,
			    int16 len, STATE_ANA_HB * State);
    void filterbank_syn_lb (float *out, const float *in, int16 len,
			    STATE_SYN_LB * State);
    void filterbank_syn_hb (float *out, const float *in_7k,
			    const float *in_14k, int16 len,
			    STATE_SYN_HB * State);



//member functions
    void clearBitTotal ();
    void printBitTotal ();
    void Bitpack (short in, unsigned short *TrWords, short NoOfBits,
		  short *ptr);
    void BitUnpack (short *out, unsigned short *RecWords, short NoOfBits,
		    short *ptr);
    void dec_fpmp (int *index, float *cb_out, int wideband_mode);
    int check_fpmp_code (int *index, int sfnum);
    float GAIN_RE_F (float acbGain, int j);
    void gainvq (float *target, float *ACBExH, float *FCBExH, float *ACBEx,
		 float *fcbGain, float *acbGain, short *idxppg,
		 short *idxcbg, float *ppvq, float *gnvq, short ACBSize,
		 short FCBGainSize, int delay, short subframel);
    void gainfcbsearch (float *Targetw, float *y2, float *fcbGain,
			float acbgain, short *idxcbg, int subframesize);

    void WB_encoder ();
    void UB_enc ();

    //Time-Warping: Changed call below to WB_Decoder
    void WB_decoder (float time_warp_fraction);

    void declick_frame (float *xLB_frame, float *xHB_frame, float *dcl_frame,
			int mshf, short mode);
    void WB_DecParams_8thRate (struct UnQUANTIZED_HB_8thRate *pHB_8,
			       float *out, int FirstTime, int erasure_8R);
    void noise_synthesis (struct QUANTIZED_HB p, float *outbuf, short first_time);	// struct PARAM_FRAME_4GVLB plb

    void WB_EncParams (struct PARAM_FRAME_4GVLB *p_LB, float *in,
		       float *shift, struct UnQUANTIZED_HB *p_HB,
		       float *LSFdistance, int FirstTime);
    void Quantize_WB_Params (struct UnQUANTIZED_HB *p,
			     struct PARAM_FRAME_4GVLB *p_LB, float LSFDist,
			     struct QUANTIZED_HB *Qp,
			     struct HB_INDICES *indices, int FirstTime);

    void dequantize_WBparams (struct QUANTIZED_HB *Dec_Qp_HB,
			      struct HB_INDICES enc_indices, short mode);
    void WB_EncParams_8thRate (struct PARAM_FRAME_4GVLB *p_LB, float *in,
			       struct UnQUANTIZED_HB_8thRate *pHB_8,
			       int FirstTime);
    void dequantize_WBparams_8R (struct UnQUANTIZED_HB_8thRate *DecQp_HB,
				 struct HB_INDICES indices);
    void Quantize_WB_params_8R (struct UnQUANTIZED_HB_8thRate *P_HB_8R,
				struct HB_INDICES *indices);

    void WB_Erasure_processing (struct QUANTIZED_HB *p, unsigned short mode,
				short init);
    void gen_shaped_noise (float *in, short voiced, float noise_gain,
			   float *lpc, struct GEN_SHP_NOISE *f, float *noise);

    void autocorrelation_ExtWin (float *r, float *input, short len,
				 short order, float *HammingWindow);

    void lpcanalys_ExtWin (float *pc, float *rc, float *input, short order,
			   short len, float *R, float *window);


    void ltf_fir (float *Residual1, float delayi, float ltgain, short len,
		  float *out);
    void agc_new (float *sig_in, float *sig_out, int16 l_trm);

    void lsp_spread (float *lsp);
    void ER_Gain_Q (float Un_gain, short *index);


    void UB_DTX (struct QUANTIZED_HB *p);

    /* some debug vars */
    short flg;

    FGV_MEM ();			//constructor

    void FrameErrorHandler (short *);

    void init_bbg ();
    void update_bbg_estimate (float *buf16, int rate, float walpha);
    void update_bbg_estimate2 (float *lpc, float energy);

    void gen_encode_bbg (short *buf16);

//private:
    void update_bgestimate (float *buf16, int rate, float walpha);
    void gen_bg_fromestimate (float *bufF);
    void bbg_eighth_rate_encode (float inBufF[SPEECH_BUFFER_LEN],
				 short *outBuf16);

};
#endif
