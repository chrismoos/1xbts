#ifndef _EVRC_STATE_H_
#define _EVRC_STATE_H_

#include "typedef.h"
#include "macro.h"
#include "ansi.h"
#include "rda.h"

#define EVRC_BQ_N_DATA     160
#define EVRC_BQ_N_FILTERS  3
#define EVRC_BQ_N_ORDER    2
#define EVRC_BQ_N_SAVE     (EVRC_BQ_N_FILTERS * EVRC_BQ_N_ORDER)

#define EVRC_NS_FRM_LEN    80
#define EVRC_NS_DELAY      24
#define EVRC_NS_FFT_LEN    128
#define EVRC_NS_NUM_CHAN   16

typedef struct EvrcCommonState {
	short Scratch[SubFrameSize + 6];
	short SScratch[6];
	short lsp[ORDER];
	short lspi[ORDER];
	short pci[ORDER];
	short *gnvq;
	short idxppg;
	short idxcb;
	short idxcbg;
	short bit_rate;
	short FCBGainSize;
	short delay;
	short LPCflag;
	short fcbIndexVector[10];
	short PackWdsPtr[2];
	short PackedWords[PACKWDSNUM];
	short *nsize;
	short *nsub;
	short *lognsize;
	short knum;
	short *lsptab;
} EvrcCommonState;

typedef struct EvrcDspState {
	INT32 giFrmCnt;
	INT32 giSfrmCnt;
	INT32 giDTXon;
	INT32 giOverflow;
	INT32 giOldOverflow;
	Longword op_counter;
} EvrcDspState;

typedef struct EvrcTtyState {
	short tty_option;
	short tty_enc_flag;
	short tty_enc_char;
	short tty_enc_header;
	short tty_enc_baud_rate;
	short tty_dec_flag;
	short tty_dec_char;
	short tty_dec_header;
	short tty_dec_baud_rate;
	short tty_debug_print_flag;
	short tty_debug_flag;
} EvrcTtyState;

typedef struct EvrcEncoderState {
	INT16 *ExconvH;
	INT32 encode_fcnt;
	INT16 HPspeech[FrameSize + GUARD * 2];
	INT16 ConstHPspeech[GUARD * 2];
	INT16 OldlspE[ORDER];
	INT16 lsp_nq[ORDER];
	INT16 Oldlsp_nq[ORDER];
	INT16 lspi_nq[ORDER];
	INT16 pci_nq[ORDER];
	INT16 wpci[ORDER];
	INT16 Excitation[ACBMemSize + SubFrameSize + EXTRA];
	INT16 H[Hlength + 1];
	INT16 HtH[Hlength + 1];
	INT16 SynMemoryM[ORDER];
	INT16 TARGET[SubFrameSize + 1];
	INT16 TARGETw[SubFrameSize + 1];
	INT16 WFmemFIR[ORDER];
	INT16 WFmemIIR[ORDER];
	INT16 zir[SubFrameSize];
	INT16 residual[2 * GUARD + FrameSize + 10];
	INT16 residualm[SubFrameSize + EXTRA];
	INT16 origm[SubFrameSize];
	INT16 *worigm;
	INT16 accshift;
	INT16 delay1;
	INT16 pdelay;
	INT16 beta;
	INT16 beta1;
	INT16 dpm;
	INT16 LPCgain;
	INT16 shiftSTATE;
	INT16 lastrateE;
	INT16 fcbGain;
	INT16 y2[55];

	Shortword bqiir_bq_xsave[EVRC_BQ_N_FILTERS * EVRC_BQ_N_SAVE];
	Longword bqiir_bq_ysave[EVRC_BQ_N_FILTERS * EVRC_BQ_N_SAVE];

	INT16 comacb_buffer[ACBMemSize + SubFrameSize + EXTRA];
	INT16 comacb_FirstTime;

	short zeroinpt_memA[ORDER];
	short zeroinpt_memA1[ORDER];
	short zeroinpt_memA2[ORDER];
	int zeroinpt_FirstTime;

	INT16 fndppf_DECbuf[FrameSize / 4];
	INT16 fndppf_lastgoodpitch;
	INT16 fndppf_lastbeta;
	INT16 fndppf_memory[3];
	int fndppf_FirstTime;

	INT16 mdfyorig_FirstTime;
	INT16 mdfyorig_a1[RRESOLUTION];
	INT16 mdfyorig_a2[RRESOLUTION];
	INT16 mdfyorig_a3[RRESOLUTION];

	ENCODER_MEM rda_rate_mem;
	INT16 rda_update_background_first;
	INT16 rda_select_rate_first;

	Shortword ns_first;
	Shortword ns_pre_emp_mem;
	Shortword ns_de_emp_mem;
	Shortword ns_overlap[EVRC_NS_FFT_LEN - EVRC_NS_FRM_LEN];
	Shortword ns_ch_gain[EVRC_NS_FFT_LEN / 2];
	Shortword ns_update_cnt;
	Shortword ns_window_overlap[EVRC_NS_DELAY];
	Shortword ns_hyster_cnt;
	Shortword ns_last_update_cnt;
	Shortword ns_ch_enrg_long_db[EVRC_NS_NUM_CHAN];
	Longword ns_frame_cnt;
	Longword ns_ch_enrg[EVRC_NS_NUM_CHAN];
	Longword ns_ch_noise[EVRC_NS_NUM_CHAN];
	Shortword ns_last_normb_shift;

	int e_ran_g_iset;
	INT32 e_ran_g_gset;
	INT16 getexc800_seed;
	INT16 getexc800_sum[NoOfSubFrames];
} EvrcEncoderState;

typedef struct EvrcDecoderState {
	INT32 decode_fcnt;
	INT16 OldlspD[ORDER];
	INT16 PitchMemoryD[ACBMemSize + SubFrameSize + EXTRA];
	INT16 PitchMemoryD_back[ACBMemSize];
	INT16 DECspeech[SubFrameSize];
	INT16 DECspeechPF[SubFrameSize];
	INT16 SynMemory[ORDER];
	INT16 erasureFlag;
	INT16 errorFlag;
	INT16 lastrateD;
	INT16 pdelayD;
	INT16 pdelayD_back;
	INT16 fer_flag;
	INT16 fer_counter;
	INT16 FadeScale;
	INT16 ave_acb_gain;
	INT16 ave_fcb_gain;
	INT16 last_valid_rate;
	INT16 last_fer_flag;
#if ANSI_EVRC_ALL_ONES
	INT16 ones_dec_cnt;
#endif

	int apf_FirstTime;
	INT16 apf_FIRmem[ORDER];
	INT16 apf_IIRmem[ORDER];
	INT16 apf_last;
	INT16 apf_Residual[ACBMemSize + SubFrameSize];

	INT16 d_fer_Seed;
	int ran_g_iset;
	INT32 ran_g_gset;
	INT16 getexc800_dec_seed;
	INT16 getexc800_dec_sum[NoOfSubFrames];
	INT16 getexc800_dec_prev_best;
} EvrcDecoderState;

typedef struct EvrcNativeContext {
	EvrcCommonState common;
	EvrcDspState dsp;
	EvrcTtyState tty;
	EvrcEncoderState encoder;
	EvrcDecoderState decoder;
} EvrcNativeContext;

EvrcNativeContext *evrc_current_context(void);
EvrcNativeContext *evrc_set_current_context(EvrcNativeContext *context);
void evrc_state_init(EvrcNativeContext *context);

#endif
