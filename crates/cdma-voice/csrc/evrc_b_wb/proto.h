/**********************************************************************
Each of the companies; Qualcomm, Motorola, Lucent, and Nokia (hereinafter 
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
all Normative portions of the EVRC-WB Variable Rate Speech Codec as 
it exists on the date of submittal of this form, should the EVRC-WB be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC-WB or the field of use of practice of the 
EVRC-WB Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

/*======================================================================*/
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     proto.h                                                 */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     11/01/94  Written By Dror Nahumi, AT&T                           */
/*----------------------------------------------------------------------*/

#ifndef  _PROTO_H_
#define  _PROTO_H_

/*======================================================================*/
/*         ..Includes.                                                  */
/*----------------------------------------------------------------------*/
#include  "macro.h"
#include  "struct.h"

//void    InitDecoder_1();
/*======================================================================*/
/*         ..CELP Function definitions.                                 */
/*----------------------------------------------------------------------*/
extern void ImpulseR (float *output, float *coefw, float *IIRmemory,
		      short order, short length);


extern void ImpulseRzp1 (float *output, float *coef_uq, float *coef,
			 float gamma1, float gamma2, short order,
			 short length);
extern void ComputeHtH (float *HtH, float *H, short length);
extern void FCBsearch (float *Ex2, float *inTARGET, float *HtH, short length,
		       short hlength, short spacing, float gopt);
extern void FCBsearch_best (float *Ex2, short *idxcb, short *idxcbg,
			    float *inTARGET, float *HtH, float *gcb,
			    short gcb_size, short length, short hlength,
			    short gainq, short delay);
extern void SynthesisFilter (float *output, float *input, float *coef,
			     float *memory, short order, short length);
extern void a2lsp (float *freq, float *a, short ord);
extern void lsp2a (float *pc, float *freq, short ord);
extern void a2rc (float *a, float *refl, short lpcorder);


extern void Interpol (float *Lar, float *last, float *current, short SubNum,
		      short order);
extern void Interpol_delay (float *out, float *last, float *current,
			    short SubNum);
extern void acb_excitation (float *Ex1, float gain, float *delay3,
			    float *PitchMemory, short length);

/*======================================================================*/
/*         ..RCELP routines.                                            */
/*----------------------------------------------------------------------*/
extern void GetResidual (float *residual, float *input, float *coef,
			 float *mem, short order, short length);
extern void putacbc (float *exctation, float *input, short dpl,
		     short subframel, short extra, float *delay3, float freq,
		     short prec);
extern void ConvolveImpulseR (float *out, float *in, float *H, short hlength,
			      short length);
extern void lspmaq_dec (short ndim, short kdim, short many, short *nsub,
			short *nsiz, float *y, short *index, short br,
			float *);
extern void lspmaq (float *x, short ndim, short kdim, short many, short *nsub,
		    short *nsiz, float alp, float *y, short *index, short br,
		    float *);
extern void iir (float *output, float *input, float *coef, float *IIRmemory,
		 short order, short length);
extern void fir (float *output, float *input, float *coef, float *FIRmemory,
		 short order, short length);
extern void SetRate (int);
extern void V_copy (float *, float *, long);

void compute_sens (float *lsc, float *Rs, double *P_s, double *Q_s,
		   double *sens);
void quantize_LSI2 (float *x, double *sens, float *xq,
		    unsigned short *qindex);
void unquantize_LSI2 (unsigned short *, float *);
void LPCPowSpect (const float *, int, const float *, int, float *);
void ppp_extract_pitch_period (const float *, float *, int);
float getSCR (const float *, int);
void timeadjust (const float *, int, float *, int);
void numHarmPerBin (int *out, int, const int *, int);
void WIsyn (DTFS, DTFS *, const float *, const float *, float *, float *,
	    int);
void stabilize (float *qlsi);

void space_lsfs (float *lsfs, int order);

void LPC_pred2refl (float a[], float refl[], int LPC_ORDER);

float ran0 (long *seed0);

#include "io.h"
#endif
