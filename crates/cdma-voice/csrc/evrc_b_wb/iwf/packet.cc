
static char const rcsid[]="$Id: packet.cc,v 1.1 2006/08/12 01:35:35 vivekr Exp $";

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     Bitpack                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     01/01/95  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

/************************************************
* Routine name: Bitpack                         *
* Function: pack input data into bitstream.     *
* Inputs:                                       *
*    in - input data.                           *
*    TrWords - pointer to transmit words memory.*
*    NoOfBits - number of bits in input data.   *
*    ptr - bit and word pointers.               *
*                                               *
************************************************/
#if 0
#include "typedef_fx.h"
#include "basic_op40.h"
#include "basic_op.h"
#include "proto_fx.h"
#include "macro_fx.h"

#ifdef WMOPS_FX
#include "lib_wmp_fx.h"
#endif //WMOPS_FX
#endif

typedef short int Word16;
typedef unsigned short int UNS_Word16;
void  Bitpack(short in,unsigned short *TrWords,short NoOfBits,short *ptr)
{
  short   temp;
  unsigned short *WordPtr;
  
  WordPtr = TrWords + ptr[1];
  
  *ptr -= NoOfBits;
#ifdef WMOPS_FX
  test();
  logic16();
#endif
  if (*ptr >= 0) {
    *WordPtr = *WordPtr | (in << *ptr);
  }
  else {
    temp = (unsigned short)in >> (-*ptr);
    *WordPtr = *WordPtr | temp;
    WordPtr++;
    *ptr = 16 + *ptr;
    *WordPtr = (short) ((long) ((long) in << *ptr) & 0xffff);
  }
  ptr[1] = (short) (WordPtr - TrWords);
}

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:    bitupack.c                                               */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     1/01/95  Born                                                    */
/*----------------------------------------------------------------------*/

/***********************************************
* Routine name: BitUnpack                      *
* Function: pack input data into bitstream.    *
* Inputs:                                      *
*    RecWords - Location of receive words.     *
*    NoOfBits - number of bits for output data.*
*    ptr - bit and word pointers.              *
* Output:                                      *
*    out - output bits.                        *
*                                              *
* Written by: Dror Nahumi.                     *
***********************************************/

void BitUnpack(short *out,unsigned short *RecWords,short NoOfBits,short *ptr)
{
  unsigned short *WordPtr;
  long    temp;
  
  WordPtr = RecWords + ptr[1];
  
  *ptr -= NoOfBits;
#ifdef WMOPS_FX
  test();
#endif
  if (*ptr >= 0) {
    temp = (long) (*WordPtr) << NoOfBits;
  }
  else {
    temp = (long) (*WordPtr) << (NoOfBits + *ptr);
    WordPtr++;
    temp = (temp << (-*ptr)) | ((long) *WordPtr << (-*ptr));
    *ptr = 16 + *ptr;
  }
  
#ifdef WMOPS_FX
  logic16();
  logic16();
#endif
  *WordPtr = (short) (temp & 0xffff);
  *out = (short) ((long) (temp & 0xffff0000) >> 16);
  
  ptr[1] = (short) (WordPtr - RecWords);
}


/*===================================================================*/
/* FUNCTION      :  TTY_DTMF_pack ().                                */
/*-------------------------------------------------------------------*/
/* PURPOSE       :  This function converts the tty data              */
/*                   into the bit-stream representation.             */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*         _ (PARAMETER  *)  data_buf    : data_buf                  */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*         _ (Word16     [])  TxPkt: bit-stream.                     */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*         _ (Word16     [])  PktPtr:   pointer to the bit-stream. */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                            _ None.                                */
/*===================================================================*/
	

