/**********************************************************************
Each of the companies; Lucent, Motorola, Nokia, and Qualcomm (hereinafter 
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
all Normative portions of the Enhanced Variable Rate Codec (EVRC) as 
it exists on the date of submittal of this form, should the EVRC be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC or the field of use of practice of the 
EVRC Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
 
/*======================================================================*/
/*     Enhanced Variable Rate Codec - Bit-Exact C Specification         */
/*     Copyright (C) 1997-1998 Telecommunications Industry Association. */
/*     All rights reserved.                                             */
/*----------------------------------------------------------------------*/
/* Note:  Reproduction and use of this software for the design and      */
/*     development of North American Wideband CDMA Digital              */
/*     Cellular Telephony Standards is authorized by the TIA.           */
/*     The TIA does not authorize the use of this software for any      */
/*     other purpose.                                                   */
/*                                                                      */
/*     The availability of this software does not provide any license   */
/*     by implication, estoppel, or otherwise under any patent rights   */
/*     of TIA member companies or others covering any use of the        */
/*     contents herein.                                                 */
/*                                                                      */
/*     Any copies of this software or derivative works must include     */
/*     this and all other proprietary notices.                          */
/*======================================================================*/
/*  Module:     proto.h                                                 */
#ifndef  _PROTO_H_
#define  _PROTO_H_

/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/
#include "typedef.h"
#include  "macro.h"

/*======================================================================*/
/*         ..CELP Function definitions.                                 */
/*----------------------------------------------------------------------*/
extern void lpcanalys(
    INT16 *pc, 
    INT16 *rc, 
    INT16 *input, 
    INT16 order, 
    INT16 len,
    INT32  *R);

extern void weight(
    INT16 *awght, 
    INT16 *a, 
    INT16 gammma, 
    INT16 order);

extern void ZeroInput(
    INT16 *output, 
    INT16 *coef_uq, 
    INT16 *coef, 
    INT16 *in, 
    INT16 gamma1, 
    INT16 gamma2, 
    INT16 order, 
    INT16 length, 
    INT16 type);

extern void ImpulseRzp(
    INT16 *output, 
    INT16 *coef_uq, 
    INT16 *coef, 
    INT16 gamma1, 
    INT16 gamma2, 
    INT16 order, 
    INT16 length);

extern void SynthesisFilter(
    INT16 *output, 
    INT16 *input, 
    INT16 *coef, 
    INT16 *memory, 
    INT16 order, 
    INT16 length);

extern void autocorrelation(
    INT32  *r, 
    INT16 *r_scale,
    INT16 *winput, 
    INT16 len, 
    INT16 order);

extern void durbin(
    INT32  *r, 
    INT16 *rc, 
    INT16 *pc, 
    INT16 order);

extern void a2lsp(
    INT16 *freq, 
    INT16 *a);

extern void lsp2a(
    INT16 *a, 
    INT16 *freq);

extern void Interpol(
    INT16 *Lar, 
    INT16 *last, 
    INT16 *current, 
    INT16 SubNum, 
    INT16 order);

extern void Interpol_delay(
    INT32  *out, 
    INT16 *last, 
    INT16 *current, 
    INT16 SubNum);

extern void acb_excitation(
    INT16 *Ex1, 
    INT16 gain, 
    INT32  *delay3, 
    INT16 *PitchMemory, 
    INT16 length);

extern void Bitpack(
    INT16 in, 
    UINT16 *TrWords, 
    INT16 NoOfBits, 
    INT16 *ptr);

extern void BitUnpack(
    INT16 *out, 
    UINT16 *RecWords, 
    INT16 NoOfBits, 
    INT16 *ptr);

/*======================================================================*/
/*         ..RCELP routines.                                            */
/*----------------------------------------------------------------------*/
extern void GetResidual(
    INT16 *residual, 
    INT16 *input, 
    INT16 *coef, 
    INT16 *mem, 
    INT16 order, 
    INT16 length);

extern void fndppf(
    INT16 *delay, 
    INT16 *beta, 
    INT16 *buf, 
    INT16 dmin, 
    INT16 dmax, 
    INT16 length);

extern void ComputeACB(
    INT16 *residualm, 
    INT16 *excitation, 
    INT32  *delay, 
    INT16 *residual, 
    INT16 guard, 
    INT16 *dpm, 
    INT16 *accshift, 
    INT16 beta, 
    INT16 length, 
    INT16 rshift);

extern void putacbc(
    INT16 *exctation, 
    INT16 *input, 
    INT16 dpl, 
    INT16 subframel, 
    INT16 extra, 
    INT32  *delay3, 
    INT16 freq, 
    INT16 prec);

extern void mod(
    INT16 *residualm, 
    INT16 *accshift, 
    INT16 beta, 
    INT16 shiftr, 
    INT16 resolution, 
    INT16 *exctation, 
    INT16 *Dresidual, 
    INT16 *residual, 
    INT16 guard, 
    INT16 *dpm, 
    INT32  delay, 
    INT16 subframel, 
    INT16 extra);

extern void cshiftframe(
    INT16 *sfstart, 
    INT16 *sfend, 
    INT16 *maxshift2, 
    INT16 dpm, 
    INT16 *residual, 
    INT16 guard, 
    INT16 accshift, 
    INT16 maxshift, 
    INT32  delay, 
    INT16 subframel, 
    INT16 extra);

extern void maxeloc(
    INT16 *maxloc, 
    INT32  *maxener, 
    INT16 *signal, 
    INT16 dp, 
    INT16 length, 
    INT16 ewl);

extern void modifyorig(
    INT16 *residualm, 
    INT16 *accshift, 
    INT16 beta, 
    INT16 *dpm, 
    INT16 shiftrange, 
    INT16 resolution, 
    INT16 *TARGET, 
    INT16 *residual, 
    INT16 dp, 
    INT16 sfend);

extern void bl_intrp(
    INT16 *output, 
    INT16 *input, 
    INT32  delay, 
    INT16 factor, 
    INT16 fl);

extern void getgain(
    INT16 *exctation, 
    INT16 *lambda, 
    INT16 *H, 
    INT16 *idxcb, 
    INT16 *gcb, 
    INT16 *gcb_mid, 
    INT16 gcb_size, 
    INT16 Quantize, 
    INT16 *mresidual, 
    INT16 subframel, 
    INT16 hlength);

extern void ConvolveImpulseR(
    INT16 *out, 
    INT16 *in, 
    INT16 *H, 
    INT16 hlength, 
    INT16 length);

extern void PickToAverage(
    INT16 *res, 
    INT16 *signal, 
    INT16 length);

extern void lspmaq_dec(
    INT16 ndim, 
    INT16 kdim, 
    INT16 many, 
    INT16 *nsub, 
    INT16 *nsiz, 
    INT16 *y, 
    INT16 *index, 
    INT16 br, 
    INT16 *);

extern void lspmaq(
    INT16 *x, 
    INT16 ndim, 
    INT16 kdim, 
    INT16 many, 
    INT16 *nsub, 
    INT16 *nsiz, 
    INT16 alp, 
    INT16 *y, 
    INT16 *index, 
    INT16 br, 
    INT16 *);

extern void Weight2Res(
    INT16 *, 
    INT16 *, 
    INT16 *, 
    INT16 *, 
    INT16, 
    INT16, 
    INT16, 
    INT16);

extern void iir(
    INT16 *output, 
    INT16 *input, 
    INT16 *coef, 
    INT16 *IIRmemory, 
    INT16 order, 
    INT16 length);

extern void fir(
    INT16 *output, 
    INT16 *input, 
    INT16 *coef, 
    INT16 *FIRmemory, 
    INT16 order, 
    INT16 length);

extern void apf(
    INT16 *, 
    INT16 *, 
    INT16 *, 
    INT32, 
    INT16, 
    INT16, 
    INT16, 
    INT16, 
    INT16, 
    INT16, 
    INT16, 
    INT16);

extern void GetExc800bps(
    INT16 *output, 
    INT16 *best, 
    INT16 scale, 
    INT16 *input, 
    INT16 length, 
    INT16 flag, 
    INT16 n);

extern void GetExc800bps_dec(
    INT16 *output, 
    INT16 length, 
    INT16 best, 
    INT16 flag, 
    INT16 n, 
    INT16 fer_flag);

extern INT16  ran_g(
    INT16 *seed0);

extern void fcb_gainq(
    INT16 *index,
    INT16 *gain_q,
    INT32 l_gain_nq,
    INT16 *gain_tab,
    INT16 gain_size);


#endif
