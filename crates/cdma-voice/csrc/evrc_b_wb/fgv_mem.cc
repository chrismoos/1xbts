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
#include "struct.h"
#include "defines.h"

//The constructor for FGV_MEM class
FGV_MEM::FGV_MEM ()
{
    int i;

    args = &args_storage;
    ibuf_len = 0;
    obuf_len = 0;
    write_accshift = 0;

    rate = 4;
    WB_encoder_first_time = 1;
    WB_decoder_first_time = 1;
    Q_delta_lag = 0;
    fer_counter = 0;
    lastrateE = 1;
    pdelay = DMIN;
    last_delay = delay1 = DMIN;
    lastrateD = 1;
    pdelayD = DMIN;

    LAST_PACKET_RATE_D = 1;

    ExconvH = Scratch;
    worigm = origm;		/* shared weighted original memory */
    for (i = 0; i < SubFrameSize; i++)
	worigm[i] = origm[i] = 0.0;
    for (i = 0; i < SubFrameSize + 6; i++)
	ExconvH[i] = Scratch[i] = 0.0;
    prev_rcelp_half = 0;
    patterncount = 0;
    pattern_m = 0;

    ave_rate_kbps = 0;
    LAST_PPP_MODE_D = 'Q';
    PPP_MODE_E = 'Q';
    acbevrcFirstTime = 1;
    celpErasureSeed = 0;

    dec_MusicModeNoiseSeed = 0;
    GetExc800bps_Seed = 0;
    GetExc800bps_dec_Seed = 0;
    cod3_10_offset[0] = 0;
    cod3_10_offset[1] = 2;
    cod3_10_offset[2] = 4;
    zeroInputFirstTime = 1;	/* init flag - sim only */
    autocorrelationFirstTime = 1;

    for (i = 0; i < ACBMemSize + SubFrameSize * 2; i++)
	Residual[i] = 0;

    for (i = 0; i < 5; i++)
	hpfmem.a[i] = hpfmem.b[i] = 0.0;
    for (i = 0; i < 5; i++)
	hpfmem8k.a[i] = hpfmem8k.b[i] = 0.0;
    rcelp_half_rateE = prev_dim_and_burstE = dim_and_burstE = 0;

    NUMFRAMES[0] = 0;		//zero,quarter,half,full
    NUMFRAMES[1] = 0;		//zero,quarter,half,full
    NUMFRAMES[2] = 0;		//zero,quarter,half,full
    NUMFRAMES[3] = 0;		//zero,quarter,half,full
    get_nacf_at_pitch_FirstTime = 1;

    for (i = 0; i < ORDER; i++) {
	FfiltMem[i] = 0.0;
	dec_FormantFilterMemory[i] = 0.0;
    }
    preemphmem[0] = 0.0;
    dec_preemphmem[0] = 0.0;

    last_delayBB = DMIN;
    prev_celp_erasure = 0;

    nelp_enc_seed = 0;
    nelp_dec_seed = 0;
    nelp_erasure_seed = 0;
    phase_offset = 10;
    run_length = 0;

    N_consec_ers = 0;

    go_back_input = 0;

    for (i = 0; i < NUM_CHAN; i++)
	ch_noise[i] = 0;
    LB_delay = DMIN;
    update_bbg_flag = 0;

    for (i = 0; i < 160; i++)
	prev_qmdct[i] = 0.0;	//previous decoded MDCT coefficients for erasure processing
    norm_fade_fac = 1;
    prev_norm = 1;
    prev_noise_gain = 1;

    for (i = 0; i < 80; i++)
	prev_synth[i] = 0.0;


    for (i = 0; i < 80; i++) {
	dec_lookahead[i] = 0.0;
	dec_lookahead_wonoise[i] = 0.0;
    }
    for (i = 0; i < ORDER; i++)
	dec_FormantFilterMemory[i] = 0.0;
    dec_preemphmem[0] = 0;


    MDCT_NPULSES = 23;		/* Default: Used for WB */
    MDCT_NWORDS = 8;		/* 23 pulses on 144 position = 114 bits => 7*15 bits + 9 bits */
    MDCT_REMAINDER = 9;

    mdct_hist_cnt = 0, celp_hist_cnt = 0;

    for (i = 0; i < 160; i++)
	mdctcoeff[i] = 0.0;
    for (i = 0; i < 54; i++)
	last_music_signal_sf[i] = 0.0;

    celpSPL_HCELPSeed = 0;
    prev_celp_mdct_dec = 0;

    ave_rate = 0;
    numactive = 0;
    AV_TH = 6.3;

    Eprev = 0;
    Eavg = 1.6E6;
    LOWVOICEDTH = 0.55;
    VOICEDTH = 0.75;
    UNVOICEDTH = 0.35;
    SNRTH = 9.9657;

    mode_decision_FirstTime = 1;
    for (i = 0; i < 12; i++) {
	lpf_filt_mem[i] = hpf_filt_mem[i] = 0;
    }

    FirstHVframe = 1;


    for (i = 0; i < 5; i++)
	z.a[i] = z.b[i] = 0.0;


    vcount = 0;
    vE[0] = 0;
    vE[1] = 0;
    vE[2] = 0;
    vEav = 0;
    vEprev = 1E8;
    prev_voiced = 0;
    prev_mode = 1;
    prev_snr_diff = 0;


	/*==================================================*/
    /*      BAD-RATE CHECK variables                    */
	/*==================================================*/
    zrbit[0] = 0;
    zrbit[1] = 0;

    BAD_RATE = 0;
    WB_COUNT = 0;
    NB_COUNT = 0;
    ones_dec_cnt = 0;

    frame_cnt = 0;
    noise_suprs_first = TRUE;
    pre_emp_mem = de_emp_mem = 0.0;
    update_cnt = 0;
    ns_snr_threshold = 6.0;
    ns_neg_snr_mean = 0.0;
    ns_neg_snr_var = 0.0;
    ns_neg_snr_bias = 0.0;
    ns_snr_previous = 0.0;
    ns_update_freeze_count = 0;
    ns_subframe_count = 0;
    for (i = 0; i < DELAY; i++)
	window_overlap[i] = 0.0;
    for (i = 0; i < NUM_CHAN; i++)
	ch_enrg[i] = 0.0;
    for (i = 0; i < FFT_LEN - FRM_LEN; i++)
	overlap[i] = 0.0;
    last_update_cnt = 0;
    hyster_cnt = 0;
    lastgoodpitch = 0;
    lastbeta = 0.0;
    fndppf_FirstTime = 1;

    mem_pre = 0;
    past_gain = 1.0;
    post_filter_first_time = 1;

    modifyorig_FirstTime = 1;
    factor1 = 0;
    factor2 = 0;

    update_background_first = 0;
    select_rate_first = 0;
    e_mem = &rate_mem;


    silence_erasure_seed = 0;
    iset = 0;
    PrevBest = 0;

    LASTLAST_PPP_MODE_E = LAST_PPP_MODE_E = 'Q';
    scr = ph_offset_E = ph_offset_D = 0.0;

    prev_nacf = 0;
    for (i = 0; i < 5; i++)
	nacf_ap[i] = 0;
    lpcgflag = FALSE;
    noSID = 0;
    WB_EncParams_Seed = 0;
    declick_frame_FirstTime = 0;
    LB2nd_der_frame_save = 0;
    comp_shift = 0;
    GainState = 1e-3;
    WB_DecParams_8thRate_Seed = 0;
    MaxErasureCount = 5;
    MaxGoodCountNeeded = 2;
    ErasureCount = 0;
    GoodCount = 0;
    noise_synthesis_Seed = 0;
    prev_mode1 = 0;
    for (i = 0;
	 i <
	 SPEECH_BUFFER_LEN * 7 / 8 + LOOKAHEAD_LEN * 7 / 8 +
	 HB_ANA_DELAY_NS_LB; i++)
	buf_HB[i] = 0.0;

    for (i = 0;
	 i <
	 SPEECH_BUFFER_LEN * 7 / 4 + LOOKAHEAD_LEN * 7 / 4 +
	 HB_ANA_DELAY_NS_LB * 2; i++)
	buf_WB14[i] = 0.0;

    for (i = 0; i < SPEECH_BUFFER_LEN + LOOKAHEAD_LEN; i++)
	buf_backup[i] = 0.0;

    for (i = 0; i < SPEECH_BUFFER_LEN * 7 / 4; i++)
	buf_WB14_out[i] = 0.0;
    for (i = 0; i < 28 + 48; i++)
	Synstate14[i] = 0.0;

    for (i = 0; i < 4; i++) {
	LBenvLPdB_frame_save[i] = 0.0;
	HBenvLPdB_frame_save2[i] = 0.0;
	HBenvLPdB_frame_save[i] = 0.0;
    }
    LB2nd_der_frame_save = 0;
    for (i = 0; i < LB_FRAMESIZE + LOOKAHEAD_8 - 1; i++)
	xLB_hpf_frame[i] = 0.0;

    for (i = 0; i < UB_FRAMESIZE + LOOKAHEAD_7 - 1; i++)
	xHB_hpf_frame[i] = 0.0;
    for (i = 0; i < DS_ATTN_SIZE; i++) {
	pulse_msr[i] = 0.0;
	excess_lvl_frame[i] = 0.0;
    }
    smooth_LB_ini = smooth_HB_ini = 0;
    comp_shift = 0;
    noise_iir[0] = 0;

    for (i = 0; i < LPF_8R_NR_ORD; i++)
	hpf_filt_mem1[i] = 0.0;



    for (i = 0; i < 4; i++) {
	fcb_target_lpfmem[i] = 0.0;
	mdct_orig_lpfmem[i] = 0.0;
	mdct_resid_lpfmem[i] = 0.0;
    }

    for (i = 0; i < 4; i++)
	fcb_filt_mem[i] = 0.0;

    for (i = 0; i < 4; i++)
	fcb_filt_mem_dec[i] = 0.0;
    autocorrelation_ExtWin_FirstTime = 1;
    initE = 0;
    initD = 0;
    erasure_8R = 0;


    // to fix UMR problems
    for (i = 0; i < SPEECH_BUFFER_LEN; i++)
	buf_out[i] = 0.0;
    beta = beta1 = 0.0;
    for (i = 0; i < 2; i++)
	curr_ns_snr[i] = 0;
    prev_snr[0] = prev_snr[1] = curr_snr[0] = curr_snr[1] = 0;
    for (i = 0; i < SPEECH_BUFFER_LEN + LOOKAHEAD_LEN; i++)
	buf[i] = 0.0;


    for (i = 0; i < 2 * SPEECH_BUFFER_LEN; i++)
	buf16[i] = 0;

    SPL_HCELP = SPL_HPPP = SPL_HNELP = 0;

    ER_counter = 0;

    ER_exct_nrg_avg = 0;

    attn_fac = 1.0;

    float ftmp;

    ftmp = 0.48 / (float) LPC_ORD_WB;
    prev_LSFs[0] = ftmp;

    for (i = 1; i < LPC_ORD_WB; i++) {
	prev_LSFs[i] = prev_LSFs[i - 1] + ftmp;
    }

    rate_last = 0;
    last_wb_mode_bit = 0;

    for (i = 0; i < FrameSize; i++)
	q_resid[i] = 0.0;
    prev_uvgain = 0.0;
    encode_HO = 0;
    dtx_posn_prev_NER = 0;
    dtx_ft = 1;
    dtx_prev_refl0 = 0.0;
    dtx_flag = 0.0;
    dtx_ra_deltasum = 0.0;
    dtx_running_avg = 0.0;
    dtx_nocompute = 0;

    sum_sm = 0.0;
    silence_Seed = 123456;
    for (i = 0; i < ORDER; i++)
	lspi_sm[i] = 0.0;
    NASS = 0;
    rcelp_fail = 0;

    Er = Erq = Erq_ln = 0.0;
    idxe = 0;
    ckck = 0.0;
    sfcnt = 0;
    for (i = 0; i < NoOfSubFrames; i++)
	da_index[i] = 0;

    wb_agc = 1.0;
    wb_agc_sm = 1.0;
    wb_agc_lpfmem[0] = wb_agc_lpfmem[1] = 0.0;
    quantize_wb_prev_mode = 1;
    wb_enc_previous_mode = 1;
    decode_DTX_STATE = 0;
    celp_mdct_firsttimehere = 1;
    celp_mdct_prev_dec = 13;

    total_bits_packed = 0;
    bit_rates = 0;
    for (i = 0; i < 25; i++)
	bit_rate_totals[i] = 0;

    for (i = 0; i < ACBMemSize + SubFrameSize; i++) {
	celp_hnw_zir_state[i] = 0.0;
	celp_hnw_state[i] = 0.0;
	celp_ppf_state[i] = 0.0;
    }
    celp_hnw_encode_fcnt_last = 0;
    celp_hnw_last_subframesize = 54;
    celp_delay_x0 = 0.0;
    celp_delay_x1 = 0.0;
    celp_ppf_encode_fcnt_last = 0;
    celp_gppf_sm = 0.0;
    celp_env1 = 1.0;
    celp_env2 = 1.0;

}
