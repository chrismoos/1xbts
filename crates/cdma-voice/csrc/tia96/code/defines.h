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
/* defines.h - basic defines for QCELP coder   */

#define PI         3.1415927
#define PF_OFF     0
#define PF_ON      1
#define PITCH      0
#define CB         1
#define ENCODER    0
#define DECODER    1
#define NOT_WGHTED 0
#define WGHTED     1
#define NO         0
#define YES        1
#define NEGATIVE   0
#define POSITIVE   1
#define BOTH       0
#define ONLY_UNWGHTED 1
#define UNLIMITED  -1

#define FULL     4
#define HALF     3
#define QUARTER  2
#define EIGHTH   1
#define BLANK    0
#define ERASURE 14
#define FRL     15

#define LSP     0
#define PGAIN   1
#define PLAG    2
#define CBGAIN  3
#define CBINDEX 4
#define CBSEED  5
#define PCB     6
