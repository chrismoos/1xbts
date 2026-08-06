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
/* filter.h - structure definitions for filter.c */

#define NO_UPDATE  0
#define UPDATE     1

struct POLE_FILTER {
  INTTYPE   order;
  float *memory;
  float *pole_coeff;
};

struct  POLE_FILTER_1_TAP {
  INTTYPE   delay;
  float *memory;
  float coeff;
  INTTYPE   max_order;
};

struct  ZERO_FILTER {
  INTTYPE   order;
  float *memory;
  float *zero_coeff;
};

struct  POLE_ZERO_FILTER {
  INTTYPE   order;
  float *memory;
  float *pole_coeff;
  float *zero_coeff;
};
