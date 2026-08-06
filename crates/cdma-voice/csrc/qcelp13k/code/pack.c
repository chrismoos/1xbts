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
/* pack.c - pack various parameters into the packet */


#include "celp.h"
#include "pack.h"

#define ANSI_733_ALL_ZEROS 1

void putbit(
    struct PACKET *packet,
    int           type,
    int           number,
    int           loc,
    int           bit
)
{

    if (bit!=0) {
	switch(type) {
	case LSPs:
	    packet->lpc[number]=packet->lpc[number]|(1<<loc);
	    break;
	case PGAIN:
	    packet->b[number]=packet->b[number]|(1<<loc);
	    break;
	case PLAG:
	    packet->lag[number]=packet->lag[number]|(1<<loc);
	    break;
	case PFRAC:
	    packet->frac[number]=packet->frac[number]|(1<<loc);
	    break;
	case CBGAIN:
	    packet->G[number][0]=packet->G[number][0]|(1<<loc);
	    break;
	case LSPVs:
	    packet->lpc[number]=packet->lpc[number]|(1<<loc);
	    break;
	case CBSIGN:
	    packet->Gsign[number][0]=packet->Gsign[number][0]|(1<<loc);
	    break;
	case CBINDEX:
	    packet->i[number][0]=packet->i[number][0]|(1<<loc);
	    break;
	case CBSEED:
	    /*packet->sd=packet->sd|(1<<loc);*/ /* never used in coder */
	                                    /* only for debugging  */
	    break;
	case RESERVE:
	    packet->mode = ERASURE;
	    break;
	}
    }

}/* end of putbit() */

int getbit(
    struct PACKET *packet,
    int           type,
    int           number,
    int           loc
)
{

  switch(type) {
  case LSPs:
    return( truefalse(packet->lpc[number], loc) );
  case PGAIN:
    return( truefalse(packet->b[number], loc) );
  case PLAG:
    return( truefalse(packet->lag[number], loc) );
  case PFRAC:
    return( truefalse(packet->frac[number], loc) );
  case LSPVs:
    return( truefalse(packet->lpc[number], loc) );
  case CBGAIN:
    return( truefalse(packet->G[number][0], loc) );
  case CBSIGN:
    return( truefalse(packet->Gsign[number][0], loc) );
  case CBINDEX:
    return( truefalse(packet->i[number][0], loc) );
  case CBSEED:
    return( truefalse(packet->sd_enc, loc) );
  case RESERVE:
    return(0);
  default:
    fprintf(stderr,"getbit: Unrecognized Packing field\n");
    exit(-2);
  }
}/* end of getbit() */

void pack_frame(
     int           mode,
     struct PACKET *packet
)
{
  int i, j;
  int *data_ptr;
  int bit[(WORDS_PER_PACKET-1)*16];
  int cnt;

  switch(mode) {
  case 4:			/* full rate */
    data_ptr= &BIT_DATA2[0][0];
    break;
  case 3:			/* half rate */
    data_ptr= &BIT_DATA1[0][0];
    break;
  case 2:			/* quarter rate */
    data_ptr= &BIT_DATA3[0][0];
    break;
  case 1:
    data_ptr= &BIT_DATA0[0][0];
    break;
  }


#if 0
  /* The next few lines helps with debugging */
  if(mode == EIGHTH || mode == QUARTERRATE_UNVOICED){
    for(i = 0; i < MAXSF; i++)
      bzero((char*)packet->i[i], MAXNUMCB*sizeof(int));
  }
#else
  if(mode == EIGHTH || mode == QUARTERRATE_UNVOICED){
    for(i = 0; i < MAXSF; i++)
      for(j = 0; j < MAXNUMCB; j++)
	packet->i[i][j] = 0;
  }
#endif

  for (i=0; i<NUMBITS[mode]; i++) {
    bit[i]=getbit(packet, data_ptr[i*3], data_ptr[i*3+1],
		  data_ptr[i*3+2]);
  }
  for (i=NUMBITS[mode]; i<(WORDS_PER_PACKET-1)*16; i++) {
    bit[i]=0;
  }
  packet->data[0]=mode;
  cnt=0;
  for (i=1; i<WORDS_PER_PACKET; i++) {
    packet->data[i]=0;
    if(mode != BLANK){
      for (j=0; j<16; j++) {
	packet->data[i]=(packet->data[i]<<1)|(bit[cnt]);
	cnt+=1;
      }
      if(packet->data[i] & 0xffff0000){
	fprintf(stderr,"pack_frame: data[%d] = %d is more than 16 bits\n",
		i, packet->data[i]);
      }
    }

  }
  if(mode == EIGHTH )
    packet->sd_enc = packet->data[1];

}/* end of pack_frame() */

void unpack_frame(
     struct PACKET *packet
)
{
  int i, j;
  int *data_ptr;
  int bit[(WORDS_PER_PACKET-1)*16];
  int cnt;
#if ANSI_733_ALL_ZEROS
  unsigned long ULtmp;
#endif /* ANSI_733_ALL_ZEROS */

  packet->mode=packet->data[0];
#if ANSI_733_ALL_ZEROS
  /* Check for all zeros full, half, or quarter-rate packet */
  /* and force that to Erasure				    */
  switch(packet->mode) {
  case 4:			/* rate 4 */
    ULtmp = 0;
    for (i=0; i < 17-1; i++) {
      ULtmp += (unsigned long) packet->data[1+i];
    }
    /* i=16, points to last word, Need to mask out 10 MSBs, bits 256:265 */
    ULtmp += (unsigned long) (packet->data[1+i]&&(0xFFC0));
    if (ULtmp ==0) packet->mode = ERASURE;
    break;
  case 3:			/* rate 3 */
    ULtmp = 0;
    for (i=0; i < 8-1; i++) {
      ULtmp += (unsigned long) packet->data[1+i];
    }
    /* i=7, points to last word, Need to mask out 12 MSBs, bits 112:123 */
    ULtmp += (unsigned long) (packet->data[1+i]&&(0xFFF0));
    if (ULtmp ==0) packet->mode = ERASURE;
    break;
  case 2:			/* rate 2 */
    ULtmp = 0;
    for (i=0; i < 4-1; i++) {
      ULtmp += (unsigned long) packet->data[1+i];
    }
    /* i=3, points to last word, Need to mask out 6 MSBs, bits 48:53 */
    ULtmp += (unsigned long) (packet->data[1+i]&&(0xFC00));
    if (ULtmp ==0) packet->mode = ERASURE;
    break;
  default:
    break;
  }
#endif /* ANSI_733_ALL_ZEROS */

  switch(packet->mode) {
  case 4:			/* rate 1 */
    data_ptr= &BIT_DATA2[0][0];
    break;
  case 3:			/* rate 1/2 */
    data_ptr= &BIT_DATA1[0][0];
    break;
  case 2:			/*  rate 1/4 */
    data_ptr= &BIT_DATA3[0][0];
    break;
  case 1:			/* rate 1/8 */
    data_ptr= &BIT_DATA0[0][0];
    break;
  case 14:/* ERASURE */
    break;
  case  0:  /* BLANK */
    break;
  default:
    fprintf(stderr, "unpack_frame: Irregular mode=%d\n", packet->mode);
  }


  if(packet->mode == EIGHTH)
    packet->sd_dec = packet->data[1];

  if (packet->mode!=ERASURE) {
    cnt=0;
    for (i=1; i<WORDS_PER_PACKET; i++) {
      for (j=15; j>=0; j--) {
	bit[cnt]=truefalse(packet->data[i], j);
	cnt+=1;
      }
    }
    clear_packet_params(packet);
    if(packet->mode != BLANK){
      for (i=0; i<NUMBITS[packet->mode]; i++) {
	putbit(packet, data_ptr[i*3], data_ptr[i*3+1], data_ptr[i*3+2],
	       bit[i]);
      }
    }
  }
}/* end of unpack_frame() */

void clear_packet_params(
     struct PACKET *packet
)
{
  int i, j;

  for (i=0; i<LPCORDER; i++) {
    packet->lsp[i]=0;
    packet->lpc[i]=0;
  }

  for (i=0; i<MAXSF; i++) {
    packet->b[i]=0;
    packet->lag[i]=0;
    packet->frac[i]=0;
    for (j=0; j<MAXNUMCB; j++) {
      packet->G[i][j]=0;
      packet->i[i][j]=0;
      packet->Gsign[i][j]=0;
    }
  }


}/* end of clear_packet_params() */

int truefalse(
    int word,
    int bitloc
)
{
    if ((word&(1<<bitloc)) ==0) {
	return(0);
    }
    else {
	return(1);
    }
}

void pack_lpc(
    int              mode,
    struct LPCPARAMS *lpc_params,
    struct PACKET    *packet
)
{
    int i;


    for (i=0; i<LPCORDER; i++) {
	packet->lsp[i]=lpc_params->lsp[i];
    }
    if(mode == EIGHTH){
      for (i=0; i<LPCORDER; i++) {
	packet->lpc[i]=lpc_params->qcode_lsp[i];
      }
    }
    else{
      for (i=0; i<5; i++) {
	packet->lpc[i]=lpc_params->qcode_lsp[i];
      }
      for (i=5; i<LPCORDER; i++) {
	packet->lpc[i]=0;
      }
    }
}


void unpack_lpc(
    int              mode,
    struct LPCPARAMS *lpc_params,
    struct PACKET    *packet
)
{
    int i;


    for (i=0; i<LPCORDER; i++) {
	lpc_params->lsp[i]=packet->lsp[i];
    }
    if(mode == EIGHTH){
      for (i=0; i<LPCORDER; i++) {
	lpc_params->qcode_lsp[i]=packet->lpc[i];
      }
    }
    else{
      for (i=0; i<5; i++) {
	lpc_params->qcode_lsp[i]=packet->lpc[i];
      }
      for (i=5; i<LPCORDER; i++) {
	lpc_params->qcode_lsp[i]=0;
      }
    }
}

void pack_pitch(
    struct PITCHPARAMS *pitch_params,
    struct PACKET      *packet,
    int                sf
)
{

  packet->b  [sf]=pitch_params->qcode_b;
  packet->lag[sf]=pitch_params->qcode_lag;
  packet->frac[sf]=pitch_params->qcode_frac;
}

void unpack_pitch(
     struct PITCHPARAMS *pitch_params,
     struct PACKET      *packet,
     int                sf
)
{
    pitch_params->qcode_b=packet->b[sf];
    pitch_params->qcode_lag=packet->lag[sf];
    pitch_params->qcode_frac=packet->frac[sf];
}/* end of unpack_pitch() */


void pack_cb(
    int             mode,
    struct CBPARAMS cb_params[MAXCBPERPIT][MAXNUMCB],
    struct PACKET   *packet,
    int             psf, /* pitch sub-frame */
    int             cbsf  /* codebook sub-frame */
)
{
    int j;

    if(mode != QUARTERRATE_UNVOICED && mode != EIGHTH){
      for (j=0; j<NUMCB[mode]; j++) {
	packet->G[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j]=
	  cb_params[cbsf][j].qcode_G;
	packet->Gsign[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j]=
	  cb_params[cbsf][j].qcode_Gsign;

	if(cb_params[cbsf][j].qcode_Gsign == POSITIVE)
	  packet->i[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j]=
	    cb_params[cbsf][j].qcode_i;
	else
	  packet->i[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j]=
	    (cb_params[cbsf][j].qcode_i+89)%CBLENGTH;
      }
    }
    else{
      for (j=0; j<NUMCB[mode]; j++) {
	packet->G[psf+cbsf][j]=
	  cb_params[cbsf][j].qcode_G;
      }
    }
}

void unpack_cb(
    int             mode,
    struct PACKET   *packet,
    struct CBPARAMS cb_params[MAXCBPERPIT][MAXNUMCB],
    int             psf,
    int             cbsf
)
{

    int j;

    if(mode != QUARTERRATE_UNVOICED && mode != EIGHTH){
      for (j=0; j<NUMCB[mode]; j++) {
	cb_params[cbsf][j].qcode_G=
	  packet->G[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j];
	cb_params[cbsf][j].qcode_Gsign=
	  packet->Gsign[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j];

	if(cb_params[cbsf][j].qcode_Gsign== POSITIVE)
	  cb_params[cbsf][j].qcode_i=
	    packet->i[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j];
	else
	  cb_params[cbsf][j].qcode_i=
	    (packet->i[psf*CBSF[mode]/PITCHSF[mode]+cbsf][j] + CBLENGTH -89)%
	      CBLENGTH;

      }
    }
    else{/* QUARTERRATE_UNVOICED */
      for (j=0; j<NUMCB[mode]; j++) {
	cb_params[cbsf][j].qcode_G=
	  packet->G[psf+cbsf][j];
	cb_params[cbsf][j].qcode_Gsign=POSITIVE;
      }
    }
}
