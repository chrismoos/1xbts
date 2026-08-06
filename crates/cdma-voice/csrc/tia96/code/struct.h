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
/* struct.h - basic structures for celp coder   */

#include"filter.h"
#include"coder.h"

struct DECODER_MEM {
    INTTYPE    type;   /* To tell whether this is the Tx or Rx decoder */
    INTTYPE    err_cnt;
    INTTYPE    low_rate_cnt;
    float  lsp_pred[LPCORDER];
    INTTYPE    G_pred[GORDER];
    float  last_qlsp[LPCORDER];
    float  last_b;
    INTTYPE    last_lag;
    float  last_G;
    INTTYPE    seed;
    struct POLE_FILTER_1_TAP pitch_filt;
    struct POLE_FILTER       lpc_filt;

    /* wghting filter used only in encoder's decoder */
    struct POLE_ZERO_FILTER  wghting_filt;

    /* these params and filters used only in decoder's decoder */
    struct ZERO_FILTER       post_filt_z;
    struct POLE_FILTER       post_filt_p;
    struct POLE_ZERO_FILTER  bright_filt;
    float  agc_factor;

};

struct ENCODER_MEM {
    float  dc_block_mem;
    float  noise_est;
    INTTYPE    last_rate;
    INTTYPE    seed;
    struct POLE_FILTER       wght_syn_filt;
    struct ZERO_FILTER       form_res_filt;
    struct DECODER_MEM       dec;
};

struct LPCPARAMS {
    float  lsp[LPCORDER];
    INTTYPE    qcode_lsp[LPCORDER];
};

struct PITCHPARAMS {
    float  b;
    INTTYPE    lag;
    INTTYPE    qcode_b;
    INTTYPE    qcode_lag;
};

struct CBPARAMS {
    float  G;
    INTTYPE    i;
    INTTYPE    qcode_G;
    INTTYPE    qcode_i;
    INTTYPE    qcode_Gsign;
    INTTYPE    seed;
};

struct PACKET {
    INTTYPE    data[WORDS_PER_PACKET];
    INTTYPE    rate;
    INTTYPE    lsp[LPCORDER];
    INTTYPE    b[MAX_PITCH_SF], lag[MAX_PITCH_SF];
    INTTYPE    G[2*MAX_PITCH_SF], i[2*MAX_PITCH_SF];
    INTTYPE    sd;
    INTTYPE    pcb;
};

struct SIGNAL_DATA {
    INTTYPE    frame_num;
    INTTYPE    max_rate;
    INTTYPE    min_rate;
    struct SIGNAL_DATA *next_sig;
};

struct CONTROL {
    INTTYPE    num_frames;
    INTTYPE    encode_only;
    INTTYPE    decode_only;
    INTTYPE    max_rate;
    INTTYPE    min_rate;
    INTTYPE    pf_flag;
    INTTYPE    signal_file;
};
