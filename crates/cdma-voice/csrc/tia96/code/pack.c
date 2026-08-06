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
/* pack.c - pack various parameters into the packet */

#include"struct.h"

pack_lsp(rate, lpc_params, packet)
INTTYPE    rate;
struct LPCPARAMS *lpc_params;
struct PACKET    *packet;
{
    INTTYPE i;

    for (i=0; i<LPCORDER; i++) {
	packet->lsp[i]=lpc_params->qcode_lsp[i];
    }
}

pack_pitch(rate, pitch_params, packet, sf)
INTTYPE    rate;
struct PITCHPARAMS *pitch_params;
struct PACKET      *packet;
INTTYPE    sf;
{
    packet->b[sf]=pitch_params->qcode_b;
    packet->lag[sf]=pitch_params->qcode_lag;
}

pack_cb(rate, cb_params, packet, psf, cbsf)
INTTYPE    rate;
struct CBPARAMS *cb_params;
struct PACKET   *packet;
INTTYPE    psf, cbsf;
{
    if (rate==EIGHTH) {
	packet->G[0]=cb_params->qcode_G;
    }
    else {
	packet->G[psf*CBSF[rate]/PITCHSF[rate]+cbsf]=cb_params->qcode_G;
	if (cb_params->qcode_Gsign ==NEGATIVE) {
	    packet->i[psf*CBSF[rate]/PITCHSF[rate]+cbsf]=
	      (cb_params->qcode_i+89)%CBLENGTH;
	    packet->G[psf*CBSF[rate]/PITCHSF[rate]+cbsf]+=4;
	}
	else {
	    packet->i[psf*CBSF[rate]/PITCHSF[rate]+cbsf]=cb_params->qcode_i;
	}
    }
}


pack_frame(rate, packet)
INTTYPE    rate;
struct PACKET *packet;
{
    INTTYPE i, j, cnt;
    INTTYPE bit[(WORDS_PER_PACKET-1)*16];
    const INTTYPE *data_ptr;

    if (rate==FULL) {
	compute_pcb(packet);
    }

    switch (rate) {
    case FULL:
	data_ptr=&BIT_DATA4[0][0];
	break;
    case HALF:
	data_ptr=&BIT_DATA3[0][0];
	break;
    case QUARTER:
	data_ptr=&BIT_DATA2[0][0];
	break;
    case EIGHTH:
	data_ptr=&BIT_DATA1[0][0];
	break;
    }

    for (i=0; i<NUMBITS[rate]; i++) {
	bit[i]=getbit(packet, data_ptr[i*3], data_ptr[i*3+1], data_ptr[i*3+2]);
    }
    for (i=NUMBITS[rate]; i<(WORDS_PER_PACKET-1)*16; i++) {
	bit[i]=0;
    }

    packet->data[0]=packet->rate;
    cnt=0;
    for (i=1; i<WORDS_PER_PACKET; i++) {
	packet->data[i]=0;
	for (j=0; j<16; j++) {
	    packet->data[i]= (packet->data[i]<<1)|(bit[cnt]);
	    cnt+=1;
	}
    }
}

unpack_cb(rate, packet, cb_params, cbsf)
INTTYPE    rate;
struct CBPARAMS *cb_params;
struct PACKET   *packet;
INTTYPE    cbsf;
{

    if (rate==EIGHTH) {
	cb_params->qcode_G=packet->G[0];
	cb_params->qcode_Gsign=POSITIVE;
    }
    else {
	cb_params->qcode_G=packet->G[cbsf]&(0x3);
	if ((packet->G[cbsf]&(0x4))!=0) {
	    cb_params->qcode_Gsign=NEGATIVE;
	    cb_params->qcode_i=(packet->i[cbsf]+CBLENGTH-89)%CBLENGTH;
	}
	else {
	    cb_params->qcode_Gsign=POSITIVE;
	    cb_params->qcode_i=packet->i[cbsf];
	}
    }
}

unpack_frame(packet, err_cnt, last_b, last_lag)
struct PACKET *packet;
float  last_b;
INTTYPE    *err_cnt, last_lag;
{
    INTTYPE i, j, cnt;
    INTTYPE bit[(WORDS_PER_PACKET-1)*16];
    INTTYPE rate;
    INTTYPE frl_flag;
    const INTTYPE *data_ptr;
    float q_b;
    INTTYPE   q_lag;

    packet->rate=packet->data[0];

    /* check for null Traffic Channel data */
    if ((packet->rate==EIGHTH)&&(packet->data[1]==0xffff)) {
	packet->rate=ERASURE;
    }

    frl_flag=NO;

    switch (packet->rate) {
    case FULL:
	data_ptr=&BIT_DATA4[0][0];
	*err_cnt=0;
	break;
    case HALF:
	data_ptr=&BIT_DATA3[0][0];
	*err_cnt=0;
	break;
    case QUARTER:
	data_ptr=&BIT_DATA2[0][0];
	*err_cnt=0;
	break;
    case EIGHTH:
	data_ptr=&BIT_DATA1[0][0];
	*err_cnt=0;
	break;
    case FRL:
	data_ptr=&BIT_DATA4[0][0];
	frl_flag=YES;
	packet->rate=FULL;
    case ERASURE:
	*err_cnt+=1; /* do this for FRLs and erasures */
	if (*err_cnt>4) {
	    *err_cnt=4;
	}
	break;
    }

    if (packet->rate!=ERASURE) {
	cnt=0;
	for (i=1; i<WORDS_PER_PACKET; i++) {
	    for (j=15; j>=0; j--) {
		bit[cnt]=truefalse(packet->data[i], j);
		cnt+=1;
	    }
	}

	clear_packet_params(packet);
	for (i=0; i<NUMBITS[packet->rate]; i++) {
	    putbit(packet, data_ptr[i*3], data_ptr[i*3+1], data_ptr[i*3+2],
		   bit[i]);
	}

	if (packet->rate==FULL) {
	    check_pcb(&(packet->rate), packet);
	}
	if ((packet->rate==FULL)&&(frl_flag==YES)) {
	    for (i=0; i<PITCHSF[packet->rate]; i++) {
		if (last_b>FRL_B_SAT[*err_cnt]) {
		    q_b=FRL_B_SAT[*err_cnt];
		}
		else {
		    q_b=last_b;
		}
		q_lag=last_lag;
		quantize_b(packet->rate, q_b, &q_b, &(packet->b[i]));
		quantize_b_and_lag(packet->rate, &q_lag, &(packet->lag[i]),
				   &q_b, &(packet->b[i]));
	    }
	}
    }

}

clear_packet_params(packet)
struct PACKET *packet;
{
    INTTYPE i;

    for (i=0; i<LPCORDER; i++) {
	packet->lsp[i]=0;
    }
    for (i=0; i<MAX_PITCH_SF; i++) {
	packet->b[i]=0;
	packet->lag[i]=0;
	packet->i[2*i]=0;
	packet->G[2*i]=0;
	packet->i[2*i+1]=0;
	packet->G[2*i+1]=0;
    }
    packet->sd=0;
    packet->pcb=0;
}


compute_pcb(packet)
struct PACKET *packet;
{
    INTTYPE  i, parity;
    INTTYPE reg;

    reg=0;
    parity=0;
    for (i=0; i<LPCORDER; i++) {
	if ((packet->lsp[i]&0x8) !=0) {
	    cycle_pcb(&reg, 1);
	    parity++;
	}
	else {
	    cycle_pcb(&reg, 0);
	}
    }
    for (i=0; i<8; i++) {
	if ((packet->G[i]&0x2) !=0) {
	    cycle_pcb(&reg, 1);
	    parity++;
	}
	else {
	    cycle_pcb(&reg, 0);
	}
    }
    reg=reg^((1<<10)-1);
    for (i=0; i<10; i++) {
	if ((reg&(1<<i))!=0) {
	    parity++;
	}
    }
    reg=reg<<1;
    if ((parity&1) !=0) {
	reg=reg+1;
    }
    packet->pcb= (INTTYPE)(reg);

}

check_pcb(rate, packet)
INTTYPE    *rate;
struct PACKET *packet;
{
    INTTYPE  i, parity, rx_parity;
    INTTYPE reg;
    INTTYPE errs;


    reg=0;
    parity=0;
    errs=0;
    for (i=0; i<LPCORDER; i++) {
	if ((packet->lsp[i]&0x8) !=0) {
	    reg=(reg<<1)|1;
	    parity++;
	}
	else {
	    reg=reg<<1;
	}
    }
    for (i=0; i<8; i++) {
	if ((packet->G[i]&0x2) !=0) {
	    reg=(reg<<1)|1;
	    parity++;
	}
	else {
	    reg=reg<<1;
	}
    }
    rx_parity=(packet->pcb)&1;

    packet->pcb=(packet->pcb>>1)^((1<<10)-1);
    for (i=9; i>=0; i--) {
	if ((packet->pcb&(1<<i))!=0) {
	    reg=(reg<<1)|1;
	    parity++;
	}
	else {
	    reg=reg<<1;
	}
    }
    parity=parity&1;
    for (i=0; i<18; i++) {
	if ((reg&(1<<27))!=0) {
	    reg=reg^(0xED20000);
	}
	reg=reg<<1;
    }
    if (reg==0) {                 /* no errors, or just a parity error */
	errs=0;
    }
    else if (parity==rx_parity) { /* more than one error */
	errs=2;
    }
    else {                        /* one or odd # > 1? */
	for (i=0; i<LPCORDER; i++) {
	    if (reg==0xF080000) {
		packet->lsp[i]=(packet->lsp[i])^0x8;
		errs++;
	    }
	    if ((reg&(1<<27))!=0) {
		reg=reg^(0xED20000);
	    }
	    reg=reg<<1;
	}
	for (i=0; i<8; i++) {
	    if (reg==0xF080000) {
		packet->G[i]=(packet->G[i])^0x2;
		errs++;
	    }
	    if ((reg&(1<<27))!=0) {
		reg=reg^(0xED20000);
	    }
	    reg=reg<<1;
	}
	for (i=0; i<10; i++) {
	    if (reg==0xF080000) {
		errs++;
	    }
	    if ((reg&(1<<27))!=0) {
		reg=reg^(0xED20000);
	    }
	    reg=reg<<1;
	}
	if (errs!=1) {             /* more than 1 error */
	    errs=2;
	}
    }

    if (errs==2) {
	/* erase full rate likelies here */
	*rate=ERASURE;
    }
}

cycle_pcb(reg, in)
INTTYPE *reg;
INTTYPE  in;
{
    INTTYPE G;

    G=0x769;

    *reg= (*reg<<1) ^ (in<<10);
    if ((*reg&(1<<10))!=0) {
	*reg= *reg^G;
    }
}

putbit(packet, type, number, loc, bit)
struct PACKET *packet;
INTTYPE    type, number, loc, bit;
{

    if (bit!=0) {
	switch(type) {
	case LSP:
	    packet->lsp[number]=packet->lsp[number]|(1<<loc);
	    break;
	case PGAIN:
	    packet->b[number]=packet->b[number]|(1<<loc);
	    break;
	case PLAG:
	    packet->lag[number]=packet->lag[number]|(1<<loc);
	    break;
	case CBGAIN:
	    packet->G[number]=packet->G[number]|(1<<loc);
	    break;
	case CBINDEX:
	    packet->i[number]=packet->i[number]|(1<<loc);
	    break;
	case CBSEED:
	    packet->sd=packet->sd|(1<<loc); /* never used in coder */
	                                    /* only for debugging  */
	    break;
	case PCB:
	    packet->pcb=packet->pcb|(1<<loc);
	    break;
	}
    }

}

getbit(packet, type, number, loc)
struct PACKET *packet;
INTTYPE    type, number, loc;
{

    switch(type) {
    case LSP:
	return( truefalse(packet->lsp[number], loc) );
	break;
    case PGAIN:
	return( truefalse(packet->b[number], loc) );
	break;
    case PLAG:
	return( truefalse(packet->lag[number], loc) );
	break;
    case CBGAIN:
	return( truefalse(packet->G[number], loc) );
	break;
    case CBINDEX:
	return( truefalse(packet->i[number], loc) );
	break;
    case CBSEED:
	return( truefalse(packet->sd, loc) );
	break;
    case PCB:
	return( truefalse(packet->pcb, loc) );
	break;
    }

}

truefalse(word, bitloc)
INTTYPE word, bitloc;
{
    if ((word&(1<<bitloc)) ==0) {
	return(0);
    }
    else {
	return(1);
    }
}
