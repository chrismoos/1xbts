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
/* filter.c - all initialization, filtering, and shutdown       */
/*    filtering routines.                                       */

#include <stdio.h>
#include <stdlib.h>
/* MODIFICATION: drop <malloc.h>, stdlib.h provides malloc()/free() on macOS. */
#include <math.h>
#include "celp.h"
#include "tty_dbg.h"

#define DEBUG(x)

void initialize_pole_filter(
    struct  POLE_FILTER     *filter,
    int                     order
)
{
    int     i;

  filter->order=order;
  if( (filter->memory    =(float *)(calloc((unsigned)order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: initialize_pole_filter(): Cannot allocate %d words for memory\n", order);
    exit(0);
  }

  if( (filter->pole_coeff=(float *)(calloc((unsigned)order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: initialize_pole_filter(): Cannot allocate %d words for coeff\n", order);
    exit(0);
  }

  for( i=0 ; i < order ; i++ )
  {
    filter->memory[i] = 0;
  }

}

void initialize_pole_1_tap_filter(
    struct POLE_FILTER_1_TAP  *filter,
    int                       max_order
)
{
    int     i;

  filter->max_order =max_order;
  if( (filter->memory    =(float *)(calloc((unsigned)max_order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: initialize_pole_1_tap_filter(): Cannot allocate %d words for memory\n", max_order);
    exit(0);
  }

  for( i=0 ; i < max_order ; i++ )
  {
    filter->memory[i] = 0;
  }

}

void initialize_zero_filter(
    struct ZERO_FILTER  *filter,
    int                 order
)
{
    int     i;

  filter->order=order;
  if( (filter->memory    =(float *)(calloc((unsigned)order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: initialize_zero_filter(): Cannot allocate %d words for memory\n", order);
    exit(0);
  }
  if( (filter->zero_coeff=(float *)(calloc((unsigned)order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: initialize_zero_filter(): Cannot allocate %d words for coeff\n", order);
    exit(0);
  }

  for( i=0 ; i < order ; i++ )
  {
    filter->memory[i] = 0;
  }

}

void free_pole_filter(
    struct  POLE_FILTER  *filter
)
{
  free((char*)filter->memory);
  free((char*)filter->pole_coeff);
}


void free_zero_filter(
    struct ZERO_FILTER  *filter
)
{
  free((char*)filter->memory);
  free((char*)filter->zero_coeff);
}

void do_pole_filter(
    float                *input,
    float                *output,
    int                  numsamples,
    struct POLE_FILTER   *filter,
    int                  update_flag
)
{

  int    i,j;
#if USE_CALLOC
  float  *tmpbuf;

  tmpbuf=(float *)(calloc((unsigned)numsamples+(filter->order), sizeof(float)));

  if( tmpbuf == NULL )
  {
    fprintf(stderr,"do_pole_filter(): CANNOT ALLOCATE MEMORY FOR tmpbuf[%d]\n\n",
        numsamples+(filter->order) );
    exit(0);
  }
#else
    float tmpbuf[160+10];

if( numsamples + filter->order > 160+10 )
{
    fprintf(stderr,"do_pole_filter(): numsamples = %d order = %d\n",numsamples, filter->order );
}
#endif


                                /* initialize first "order" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->order; i++) {
    tmpbuf[i]=filter->memory[filter->order-i-1];
  }
                                /* initialize next locations to the input */
  for (i=0; i<numsamples; i++) {
    tmpbuf[i+filter->order]=input[i];
  }

#if 1
                               /* filter the buffer                      */
  for (i=0; i<numsamples; i++)
  {
    for (j=i; j<i+(filter->order); j++)
    {
      tmpbuf[i+(filter->order)] +=
    tmpbuf[j]*(filter->pole_coeff[(filter->order)+i-j-1]);
    }
  }
#endif

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

#if USE_CALLOC
  free((char*)tmpbuf);
#endif

}

void debug_do_pole_filter(
    float                *input,
    float                *output,
    int                  numsamples,
    struct POLE_FILTER   *filter,
    int                  update_flag
)
{

  int    i,j;
#if USE_CALLOC
  float  *tmpbuf;

  tmpbuf=(float *)(calloc((unsigned)numsamples+(filter->order), sizeof(float)));

  if( tmpbuf == NULL )
  {
    fprintf(stderr,"do_pole_filter(): CANNOT ALLOCATE MEMORY FOR tmpbuf[%d]\n\n",
        numsamples+(filter->order) );
    exit(0);
  }
#else
    float tmpbuf[160+10];


if( numsamples + filter->order > 160+10 )
{
    fprintf(stderr,"do_pole_filter(): numsamples = %d order = %d\n",numsamples, filter->order );
}
#endif


                                /* initialize first "order" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->order; i++) {
    tmpbuf[i]=filter->memory[filter->order-i-1];
  }
                                /* initialize next locations to the input */
  for (i=0; i<numsamples; i++) {
    tmpbuf[i+filter->order]=input[i];
  }

#if 1
                               /* filter the buffer                      */
  for (i=0; i<numsamples; i++)
  {
    for (j=i; j<i+(filter->order); j++)
    {
      tmpbuf[i+(filter->order)] +=
    tmpbuf[j]*(filter->pole_coeff[(filter->order)+i-j-1]);
    }
  }
#endif

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

#if USE_CALLOC
  free((char*)tmpbuf);
#endif

}


void do_pole_filter_1_tap_interp(
    float                     *input,
    float                     *output,
    int                       numsamples,
    struct POLE_FILTER_1_TAP  *filter,
    int                       update_flag
)
{

  float  *tmpbuf;
  int    i,j;

  if( (tmpbuf=(float *)(calloc((unsigned)numsamples+filter->max_order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: do_pole_filter_1_tap_interp(): Cannot allocate %d words for tmpbuf[]\n",
        numsamples+filter->max_order);
    exit(0);
  }

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->max_order; i++) {
    tmpbuf[i]=filter->memory[filter->max_order-i-1];
  }
                                /* initialize next locations to the input */
  for (i=0; i<numsamples; i++) {
    tmpbuf[i+filter->max_order]=input[i];
  }

                /* apply pitch filter to the buffer                      */

  if (filter->frac==0) {
      for (i=0; i<numsamples; i++) {
    tmpbuf[i+filter->max_order]+=round_flt(filter->coeff*
	                      tmpbuf[i+filter->max_order-filter->delay]);
      }
  }
  else {/* Fractional Pitch Values */
    for (i=0; i<numsamples; i++) {
      for (j= -FR_INTERP_FILTER_LENGTH/2; j<FR_INTERP_FILTER_LENGTH/2; j++) {
	if (filter->frac>0) {
	  tmpbuf[i+filter->max_order]+= filter->coeff*
	    tmpbuf[i+filter->max_order-filter->delay+j]*
	      hammsinc((float)(filter->frac+j*MAX_FR_RESOLUTION));
	}
	else {
	  tmpbuf[i+filter->max_order]+=filter->coeff*
	    tmpbuf[i+filter->max_order-filter->delay-j]*
	      hammsinc((float)(filter->frac-j*MAX_FR_RESOLUTION));
	}
      }
    }
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
  free((char*)tmpbuf);
}/* end of void do_pole_filter_1_tap_interp() */

void do_zero_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
)
{

  float  *tmpbuf;
  int    i,j;


  if( (tmpbuf=(float *)(calloc((unsigned)numsamples+filter->order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: do_zero_filter(): Cannot allocate %d words for tmpbuf[]\n",
        numsamples+filter->order);
    exit(0);
  }

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->order; i++) {
    tmpbuf[i]=filter->memory[filter->order-i-1];
  }
                                /* initialize next locations to the input */
  for (i=0; i<numsamples; i++)
  {
    tmpbuf[i+filter->order]=input[i];
  }
                                /* filter the buffer                      */
  for (i=0; i<numsamples; i++)
  {
    output[i]=input[i];
    for (j=i; j<i+filter->order; j++)
    {
      output[i]+=tmpbuf[j]*filter->zero_coeff[filter->order+i-j-1];
    }
  }

                                /* if flag is set, update the memories    */
  if (update_flag==UPDATE) {
    for (i=0; i<filter->order; i++) {
      filter->memory[i]=tmpbuf[filter->order+numsamples-i-1];
    }
  }
  free((char*)tmpbuf);
}

#if 0
void debug_do_zero_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
)
{

  float  *tmpbuf;
  int    i,j;

  DEBUG(
  fflush(stdout);
  fprintf(stdout,"Inside debug_do_zero_filter():\n");
  )
#if 1
  tmpbuf=(float *)(calloc((unsigned)numsamples+filter->order, sizeof(float)));
#else
  tmpbuf=(float *)(malloc((unsigned)numsamples+filter->order*sizeof(float)));
#endif


  if( tmpbuf == NULL )
  {
        fprintf(stderr,"filter: mem alloc failure for tmpbuf\n");
  }

                                /* initialize first "order" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->order; i++) {
    tmpbuf[i]=filter->memory[filter->order-i-1];
  }
                                /* initialize next locations to the input */
  DEBUG(fprintf(stdout,"  order = %d\n",filter->order);)
  for (i=0; i<numsamples; i++)
  {
    tmpbuf[i+filter->order]=input[i];
  }
  DEBUG(fprintf(stdout,"  order = %d\n",filter->order);)
  DEBUG(dump_float_array(tmpbuf,numsamples,tempfp);)
                                /* filter the buffer                      */
  for (i=0; i<numsamples; i++)
  {
    output[i]=input[i];
    for (j=i; j<i+filter->order; j++)
    {
DEBUG(
fprintf(stdout,"  order = %d  ",filter->order);
fprintf(stdout,"(%d,%d,%d): %8.2f += %8.2f * %8.2f\n",
 i,j,(filter->order)+i-j-1,output[i],tmpbuf[j],filter->zero_coeff[filter->order+i-j-1]);
)
      output[i]+=tmpbuf[j]*filter->zero_coeff[filter->order+i-j-1];
    }
DEBUG(
  fprintf(stdout,"  order = %d",filter->order);
fprintf(stdout,"==================================\n");
)
  }
DEBUG(
  fprintf(stdout,"  order = %d\n",filter->order);
dump_float_array(output,numsamples,temp2fp);
fflush(stdout);
)

                                /* if flag is set, update the memories    */
  if (update_flag==UPDATE) {
    for (i=0; i<filter->order; i++) {
      filter->memory[i]=tmpbuf[filter->order+numsamples-i-1];
    }
  }
  free((char*)tmpbuf);
}  /* end debug_do_zero_filter() */
#endif


void do_fir_linear_filter(
    float               *input,
    float               *output,
    int                 numsamples,
    struct ZERO_FILTER  *filter,
    int                 update_flag
)
{

  float  *tmpbuf;
  int    i,j;

  if( (tmpbuf=(float *)(calloc((unsigned)numsamples+filter->order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: do_fir_linear_filter(): Cannot allocate %d words for tmpbuf[]\n",
        numsamples+filter->order);
    exit(0);
  }

                                /* initialize first "order-1" locations to  */
                                /* previous memories                      */
  for (i=0; i<filter->order-1; i++) {
    tmpbuf[i]=filter->memory[filter->order-i-2];
  }
                                /* initialize next locations to the input */
  for (i=0; i<numsamples; i++) {
    tmpbuf[i+filter->order-1]=input[i];
  }
                                /* filter the buffer                      */
  for (i=0; i<numsamples; i++) {
    output[i]=0.0;
    for (j=i; j<i+filter->order; j++) {
      output[i]+=tmpbuf[j]*filter->zero_coeff[filter->order+i-j-1];
    }
  }
                                /* if flag is set, update the memories    */
  if (update_flag==UPDATE) {
    for (i=0; i<filter->order-1; i++) {
      filter->memory[i]=tmpbuf[filter->order+numsamples-i-2];
    }
  }
  free((char*)tmpbuf);
}

void get_impulse_response_pole(
    float               *response,
    int                 length,
    struct POLE_FILTER  *filter
)
{
  int    i;
  float  *tmpmemory;
  float  *impulse;

  if( (tmpmemory=(float *)(calloc((unsigned)filter->order, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: get_impulse_response_pole(): Cannot allocate %d words for tmpmemory[]\n",
        filter->order);
    exit(0);
  }

  if( (impulse  =(float *)(calloc((unsigned)length, sizeof(float)))) == NULL )
  {
    fprintf(stderr,"ERROR: get_impulse_response_pole(): Cannot allocate %d words for impulse[]\n",
        length);
    exit(0);
  }

  for (i=0; i<filter->order; i++) {
    tmpmemory[i]=filter->memory[i];
    filter->memory[i]=0.0;
  }
  impulse[0]=1.0;

  do_pole_filter(impulse, response, length, filter, NO_UPDATE);

  for (i=0; i<filter->order; i++) {
    filter->memory[i]=tmpmemory[i];
  }
  free((char*)tmpmemory);
  free((char*)impulse);
}


void get_zero_input_response_pole(
    float               *response,
    int                 length,
    struct POLE_FILTER  *filter
)
{
  float  *input;

  input=(float *)(calloc((unsigned)length, sizeof(float)));

  do_pole_filter(input, response, length, filter, NO_UPDATE);

  free((char*)input);
}


void get_zero_input_response_pole_1_tap_interp(
    float                     *response,
    int                       length,
    struct POLE_FILTER_1_TAP  *filter
)
{
  float  *input;

  input=(float *)(calloc((unsigned)length, sizeof(float)));

  do_pole_filter_1_tap_interp(input, response, length, filter, NO_UPDATE);

  free((char*)input);
}
