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
/* filter.c - all initialization, filtering, and impulse response */
/*    filtering routines.                                         */

#include<stdlib.h>
#include"filter.h"

initialize_pole_filter(filter, order)
struct  POLE_FILTER	*filter;
INTTYPE     order;
{

    filter->order=order;
    filter->memory    =(float *)(calloc(order, sizeof(float)));
    filter->pole_coeff=(float *)(calloc(order, sizeof(float)));

}

initialize_pole_1_tap_filter(filter, max_order)
struct  POLE_FILTER_1_TAP	*filter;
INTTYPE     max_order;
{

    filter->max_order =max_order;
    filter->memory    =(float *)(calloc(max_order, sizeof(float)));

}

initialize_zero_filter(filter, order)
struct  ZERO_FILTER	*filter;
INTTYPE     order;
{

    filter->order=order;
    filter->memory    =(float *)(calloc(order, sizeof(float)));
    filter->zero_coeff=(float *)(calloc(order, sizeof(float)));

}

initialize_pole_zero_filter(filter, order)
struct  POLE_ZERO_FILTER	*filter;
INTTYPE     order;
{

    filter->order=order;
    filter->memory    =(float *)(calloc(order, sizeof(float)));
    filter->pole_coeff=(float *)(calloc(order, sizeof(float)));
    filter->zero_coeff=(float *)(calloc(order, sizeof(float)));

}

do_pole_filter(input, output, numsamples, filter, update_flag)
float   *input, *output;
INTTYPE     numsamples;
struct  POLE_FILTER   *filter;
INTTYPE     update_flag;
{

    float  *tmpbuf;
    INTTYPE    i,j;

    tmpbuf=(float *)(calloc(numsamples+filter->order, sizeof(float)));

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
    for (i=0; i<filter->order; i++) {
	tmpbuf[i]=filter->memory[filter->order-i-1];
    }
                                /* initialize next locations to the input */
    for (i=0; i<numsamples; i++) {
	tmpbuf[i+filter->order]=input[i];
    }
                                /* filter the buffer                      */
    for (i=0; i<numsamples; i++) {
	for (j=i; j<i+filter->order; j++) {
	    tmpbuf[i+filter->order]+=
	      tmpbuf[j]*filter->pole_coeff[filter->order+i-j-1];
	}
    }
                                /* copy the filtered samples to the output*/
    for (i=0; i<numsamples; i++) {
	output[i]=tmpbuf[i+filter->order];
    }
                                /* if flag is set, update the memories    */
    if (update_flag==UPDATE) {
	for (i=0; i<filter->order; i++) {
	    filter->memory[i]=tmpbuf[filter->order+numsamples-i-1];
	}
    }
    free(tmpbuf);
}

do_pole_filter_1_tap(input, output, numsamples, filter, update_flag)
float   *input, *output;
INTTYPE     numsamples;
struct  POLE_FILTER_1_TAP   *filter;
INTTYPE     update_flag;
{

    float  *tmpbuf;
    INTTYPE    i,j;

    tmpbuf=(float *)(calloc(numsamples+filter->max_order, sizeof(float)));

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
    for (i=0; i<filter->max_order; i++) {
	tmpbuf[i]=filter->memory[filter->max_order-i-1];
    }
                                /* initialize next locations to the input */
    for (i=0; i<numsamples; i++) {
	tmpbuf[i+filter->max_order]=input[i];
    }
                                /* filter the buffer                      */
    for (i=0; i<numsamples; i++) {
	tmpbuf[i+filter->max_order]+=
	  tmpbuf[i-filter->delay+filter->max_order]*filter->coeff;
    }
                                /* copy the filtered samples to the output*/
    for (i=0; i<numsamples; i++) {
	output[i]=tmpbuf[i+filter->max_order];
    }
                                /* if flag is set, update the memories    */
    if (update_flag==UPDATE) {
	for (i=0; i<filter->max_order; i++) {
	    filter->memory[i]=tmpbuf[filter->max_order+numsamples-i-1];
	}
    }
    free(tmpbuf);
}

do_zero_filter(input, output, numsamples, filter, update_flag)
float   *input, *output;
INTTYPE     numsamples;
struct  ZERO_FILTER   *filter;
INTTYPE     update_flag;
{

    float  *tmpbuf;
    INTTYPE    i,j;

    tmpbuf=(float *)(calloc(numsamples+filter->order, sizeof(float)));

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
    for (i=0; i<filter->order; i++) {
	tmpbuf[i]=filter->memory[filter->order-i-1];
    }
                                /* initialize next locations to the input */
    for (i=0; i<numsamples; i++) {
	tmpbuf[i+filter->order]=input[i];
    }
                                /* filter the buffer                      */
    for (i=0; i<numsamples; i++) {
	output[i]=input[i];
	for (j=i; j<i+filter->order; j++) {
	    output[i]+=tmpbuf[j]*filter->zero_coeff[filter->order+i-j-1];
	}
    }
                                /* if flag is set, update the memories    */
    if (update_flag==UPDATE) {
	for (i=0; i<filter->order; i++) {
	    filter->memory[i]=tmpbuf[filter->order+numsamples-i-1];
	}
    }
    free(tmpbuf);
}

do_pole_zero_filter(input, output, numsamples, filter, update_flag)
float   *input, *output;
INTTYPE     numsamples;
struct  POLE_ZERO_FILTER   *filter;
INTTYPE     update_flag;
{

    float  *tmpbuf;
    INTTYPE    i,j;

    tmpbuf=(float *)(calloc(numsamples+filter->order, sizeof(float)));

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
    for (i=0; i<filter->order; i++) {
	tmpbuf[i]=filter->memory[filter->order-i-1];
    }
                                /* initialize next locations to the input */
    for (i=0; i<numsamples; i++) {
	tmpbuf[i+filter->order]=input[i];
    }
                                /* POLE filter the buffer                 */
    for (i=0; i<numsamples; i++) {
	for (j=i; j<i+filter->order; j++) {
	    tmpbuf[i+filter->order]+=
	      tmpbuf[j]*filter->pole_coeff[filter->order+i-j-1];
	}
    }
                                /* if flag is set, update the memories    */
    if (update_flag==UPDATE) {
	for (i=0; i<filter->order; i++) {
	    filter->memory[i]=tmpbuf[filter->order+numsamples-i-1];
	}
    }
                                /* ZERO filter the buffer                 */
    for (i=0; i<numsamples; i++) {
	output[i]=tmpbuf[i+filter->order];
	for (j=i; j<i+filter->order; j++) {
	    output[i]+=tmpbuf[j]*filter->zero_coeff[filter->order+i-j-1];
	}
    }
    free(tmpbuf);
}

get_impulse_response_pole(response, length, filter)
float  *response;
INTTYPE    length;
struct POLE_FILTER  *filter;
{
    INTTYPE    i,j;
    float  *tmpmemory;
    float  *impulse;

    tmpmemory=(float *)(calloc(filter->order, sizeof(float)));
    impulse  =(float *)(calloc(length, sizeof(float)));

    for (i=0; i<filter->order; i++) {
	tmpmemory[i]=filter->memory[i];
	filter->memory[i]=0.0;
    }
    impulse[0]=1.0;

    do_pole_filter(impulse, response, length, filter, NO_UPDATE);

    for (i=0; i<filter->order; i++) {
	filter->memory[i]=tmpmemory[i];
    }
    free(tmpmemory);
    free(impulse);
}

get_zero_input_response_pole(response, length, filter)
float  *response;
INTTYPE    length;
struct POLE_FILTER  *filter;
{
    INTTYPE    i,j;
    float  *tmpmemory;
    float  *input;

    input=(float *)(calloc(length, sizeof(float)));

    do_pole_filter(input, response, length, filter, NO_UPDATE);

    free(input);
}

get_zero_input_response_pole_1_tap(response, length, filter)
float  *response;
INTTYPE    length;
struct POLE_FILTER_1_TAP  *filter;
{
    INTTYPE    i,j;
    float  *tmpmemory;
    float  *input;

    input=(float *)(calloc(length, sizeof(float)));

    do_pole_filter_1_tap(input, response, length, filter, NO_UPDATE);

    free(input);
}
