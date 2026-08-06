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
/* ratedec - selects rate to use */

#include"struct.h"

select_rate(rate, frame_num, R, noise_est, control, sig_ptr,
	    last_rate, packed_rate)
INTTYPE    *rate;
INTTYPE    frame_num;
float  R;
float  *noise_est;
struct CONTROL     *control;
struct SIGNAL_DATA **sig_ptr;
INTTYPE    *last_rate;
INTTYPE    *packed_rate;
{
    INTTYPE i, j;
    INTTYPE thresh_set;
    float thresh[3];

    if (*noise_est>LOW_THRESH_LIM) {
	thresh_set=1;
    }
    else {
	thresh_set=0;
    }

    *rate=1;
    for (i=0; i<3; i++) {
	thresh[i]=0;
	for (j=0; j<3; j++) {
	    thresh[i]=thresh[i]* *noise_est + RATE_THRESH[thresh_set][i][j];
	}
	if (R>thresh[i]) {
	    *rate+=1;
	}
    }
    if (*last_rate - *rate>1) {
	*rate= *last_rate-1;
    }

    if (*rate>control->max_rate) {
	*rate=control->max_rate;
    }
    if (*rate<control->min_rate) {
	*rate=control->min_rate;
    }
    if (control->signal_file==YES) {
	if ((*sig_ptr)->frame_num==frame_num) {
	    if (*rate>(*sig_ptr)->max_rate) {
		*rate=(*sig_ptr)->max_rate;
	    }
	    if (*rate<(*sig_ptr)->min_rate) {
		*rate=(*sig_ptr)->min_rate;
	    }
	    *sig_ptr=(*sig_ptr)->next_sig;
	}
    }

    *last_rate= *rate;
    *packed_rate= *rate;

    /* update background noise estimate for the next frame */
    /* Identical to just updating the estimate at the beginning */
    /*     of the next frame */
    if (THRESH_UPDATE_FACTOR* *noise_est> *noise_est+1) {
	*noise_est*=THRESH_UPDATE_FACTOR;
    }
    else {
	*noise_est+=1;
    }
    if (*noise_est>HIGH_THRESH_LIM) {
	*noise_est=HIGH_THRESH_LIM;
    }
    if (*noise_est>R) {
	*noise_est=R;
    }
}
