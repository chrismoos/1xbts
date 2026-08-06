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
/*===================================================================*/
/* LIBRARY: lib_wmp_fx.c                                             */
/*-------------------------------------------------------------------*/
/* PURPOSE: Calculate the WMOPS for the fixed point code.            */
/*===================================================================*/

#ifdef WMOPS_FX

/*----------------------------------------------------------------------------*/
/*--------------------------------- INCLUDE ----------------------------------*/
/*----------------------------------------------------------------------------*/

#include "typedef_fx.h"

#include "const_fx.h"
#include "main_fx.h"

#include "ext_var_fx.h"

#include "gputil_fx.h"

#include "lib_wmp_fx.h"

/*----------------------------------------------------------------------------*/
/*-------------------------------- FUNCTIONS ---------------------------------*/
/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_init_lib_fx ().                                   */
/*-------------------------------------------------------------------*/
/* PURPOSE   : Performs the initialisation of the WMOPS fixed point  */
/*             counting library.                                     */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                      _ None.                                      */
/*===================================================================*/

void WMP_init_lib_fx (void)
	{
	 /*-------------------------------------------------------------------*/
	 /*            Reset function weight operation counter variable       */
	 /*-------------------------------------------------------------------*/

	 ini_ivector_fx(wc_fx, 0, NB_FUNC_MAX-1, (Word32)0);

	 glob_wc_fx = 0;
	 nbframe_fx = 0;
	 total_wmops_fx = 0.0;

	 /*-------------------------------------------------------------------*/

	 return;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : 'op' ().                                              */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions increment the corresponding 'op'      */
/*             counters.                                             */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*          _ None     (increment the counter by 1).                 */
/*       or                                                          */
/*          _ (Word16)  nb:  the number of test operations.          */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                      _ None.                                      */
/*===================================================================*/

void move16 (void)
{
    counter_fx.DataMove16++;
}

void move32 (void)
{
    counter_fx.DataMove32++;
}

void test (void)
{
    counter_fx.Test++;
}

void logic16 (void)
{
    counter_fx.Logic16++;
}

void logic32 (void)
{
    counter_fx.Logic32++;
}

void Nmove16 (Word16 n)
{
#ifdef WRNG_WMOPS_FX
    if( n> 160 || n < 0)
      printf("DataMove16 %hd\n", n);
#endif
    counter_fx.DataMove16+=n;
}

void Nmove32 (Word16 n)
{
#ifdef WRNG_WMOPS_FX
    if( n> 160 || n < 0)
      printf("DataMove32  %hd\n",n);
#endif
    counter_fx.DataMove32+=n;
}

void Ntest (Word16 n)
{
#ifdef WRNG_WMOPS_FX
    if( n> 160 || n < 0)
      printf("Test  %hd\n",n);
#endif
    counter_fx.Test+=n;
}

void Nlogic16 (Word16 n)
{
#ifdef WRNG_WMOPS_FX
    if( n> 160 || n < 0)
      printf("Logic  %hd\n",n);
#endif
    counter_fx.Logic16+=n;
}

void Nlogic32 (Word16 n)
{
#ifdef WRNG_WMOPS_FX
    if( n> 160 || n < 0)
      printf("Logic32 %hd \n",n);
#endif
    counter_fx.Logic32+=n;
}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_calcul_total_WMOPS_fx ().                         */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions calculate the total Number of         */
/*             Weighted Operations for fixed point code.             */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*         _ (Word32)   wmops:  the number of weighted operations.    */
/*===================================================================*/

Word32 WMP_calcul_total_WMOPS_fx (void)
	{
	 /*-------------------------------------------------------------------*/

	 Word16 i=0, k=0;
	 Word32 wmops=0.0, *ptr1, *ptr2;

	 /*-------------------------------------------------------------------*/

	 ptr1 = (Word32 *) &counter_fx;
	 ptr2 = (Word32 *) &op_weight_fx;

	 k  = sizeof (counter_fx) / sizeof (Word32);
	 wmops = 0;
	 for (i = 0; i < k; i++)
	   wmops += ((*ptr1++) * (*ptr2++));

	 /*-------------------------------------------------------------------*/

	 return wmops;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_reset_counter_WMOPS_fx ().                        */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions reset the code counter of WMOPS       */
/*             and it's for fixed point called at the beginning of   */
/*             the processing of each frame.                         */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*         _ (Word32)   wmops:  the number of weighted operations.    */
/*===================================================================*/

void WMP_reset_counter_WMOPS_fx (void)
	{
	 /*-------------------------------------------------------------------*/

	 Word16 i, k;
	 Word32 *ptr1;

	 /*-------------------------------------------------------------------*/
	 /*                         Reset all the counters                    */
	 /*-------------------------------------------------------------------*/

	 ptr1 = (Word32 *) &counter_fx;

	 k = sizeof (counter_fx) / sizeof (Word32);

	 for (i = 0; i < k; i++)
	   *ptr1++ = 0;

	 /*-------------------------------------------------------------------*/

	 LastWOper_fx = 0;

	 /*-------------------------------------------------------------------*/

	 return;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_calcul_delta_WMOPS_fx ().                         */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions calculate the partial Number of       */
/*             Weighted Operations between two WMP_fwc().            */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*         _ (Word32)   delta:  the number of weighted operations.    */
/*===================================================================*/

Word32 WMP_calcul_delta_WMOPS_fx (void)
	{
	 /*-------------------------------------------------------------------*/

	 Word32 NewWOper, delta;

	 /*-------------------------------------------------------------------*/

	 NewWOper = WMP_calcul_total_WMOPS_fx ();
	 delta = NewWOper - LastWOper_fx;

	 /*-------------------------------------------------------------------*/

	 LastWOper_fx = NewWOper;

	 /*-------------------------------------------------------------------*/

	 return delta;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_hash_fx ().                                       */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions extract the pointer to a specific     */
/*             worst case function.                                  */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*     _ (char []) func_name :  the name of the fucntion under test. */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*         _ (Word16)  funcid:  the function identifier.             */
/*===================================================================*/

Word16 WMP_hash_fx (char func_name [])
	{
	 /*-------------------------------------------------------------------*/

	 unsigned	funcid;
	 Word16		k;

	 /*-------------------------------------------------------------------*/

	 funcid = 0;
	 k = 0;
	 while (func_name [k] != '\0')
	   {
	     funcid = func_name [k] + 31 * funcid;
	     k++;
	   }

	 /*-------------------------------------------------------------------*/

	 funcid = funcid  % NB_FUNC_MAX;

	 /*-------------------------------------------------------------------*/

	 return funcid;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_fwc_fx ().                                        */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions extract the counter status for the    */
/*             fixed point WMOPS it must be called AFTER the call    */
/*             to the function to be evaluated.                      */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*     _ (char []) func_name :  the name of the fucntion under test. */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*         _ (Word32)   delta:  the number of weighted operations.    */
/*===================================================================*/

Word32 WMP_fwc_fx (char func_name [])
	{
	 /*-------------------------------------------------------------------*/

	 Word32	wmops;
	 Word16  funcid;

	 /*-------------------------------------------------------------------*/

	 wmops = WMP_calcul_delta_WMOPS_fx ();

	 funcid = WMP_hash_fx(func_name);

	 if (wmops > wc_fx[funcid])
	   {
	     wc_fx[funcid] = wmops;
	     strcpy(wc_fn_fx [funcid], func_name);
	   }

	 /*-------------------------------------------------------------------*/

	 return wmops;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_calcul_stat_fx ().                                */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions extract the counter status for the    */
/*             fixed point WMOPS at the end of each frame processing */
/*             and calculate the stats.                              */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*      _ (Word32)   frame:  the number of frames.                    */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*    _ (Word40)  frame_wmops:   the number of weighted operations  */
/*                                for the current frame.             */
/*===================================================================*/

Word40 WMP_calcul_stat_fx (Word32 frame)
	{
	 /*-------------------------------------------------------------------*/

	 Word32	 wmops;
	 Word40 frame_wmops;

	 /*-------------------------------------------------------------------*/
	 /*                Extract the WMOPS for the current frame            */
	 /*-------------------------------------------------------------------*/

	 wmops = WMP_calcul_total_WMOPS_fx ();

	 /*-------------------------------------------------------------------*/
	 /*                    Worst Case per Frame Detection                 */
	 /*-------------------------------------------------------------------*/

	 if (wmops > glob_wc_fx)
	   {
	     glob_wc_fx = wmops;
	     wc_frame_fx = frame;
	   }

	 nbframe_fx++;

	 frame_wmops = (Word40)wmops * 0.00005;

	 total_wmops_fx += frame_wmops;

	 /*-------------------------------------------------------------------*/

	 return frame_wmops;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_swap_fx ().                                       */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions swap two elements in the array.       */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*     _ (Word16) i    :  i firt element index.                      */
/*     _ (Word16) j    :  j second element index.                    */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*     _ (Word32  []) wc    :  array of the mesured worst case.       */
/*     _ (char   []) wc_fn :  array of the the name of the function. */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                      _ None.                                      */
/*===================================================================*/

void WMP_swap_fx (Word32 wc[], char wc_fn [NB_FUNC_MAX][255], Word16 i, Word16 j)
	{
	 /*-------------------------------------------------------------------*/

	 Word32	temp_wc;
	 char 	tmp_str [255];

	 /*-------------------------------------------------------------------*/

	 temp_wc = wc[i];
	 wc[i]   = wc[j];
	 wc[j]   = temp_wc;

	 /*-------------------------------------------------------------------*/

	 strcpy (tmp_str,  wc_fn[i]);
	 strcpy (wc_fn[i], wc_fn[j]);
	 strcpy (wc_fn[j], tmp_str);

	 /*-------------------------------------------------------------------*/

	 return;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/


/*===================================================================*/
/* FUNCTION  : WMP_qsort_fx ().                                      */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions reorder the Worst Case function  and  */
/*              name.                                                */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*     _ (Word16 ) left    :  lower index of the array.              */
/*     _ (Word16 ) right   :  higher index of the array.             */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*     _ (Word32  []) wc    :  array of the mesured wosrt case.       */
/*     _ (char   []) wc_fn :  array of the the name of the function. */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                      _ None.                                      */
/*===================================================================*/

void WMP_qsort_fx (Word32 wc[], char wc_fn [NB_FUNC_MAX][255], Word16 left,
		   Word16 right)
	{
	 /*-------------------------------------------------------------------*/

	 Word16 i, last;

	 /*-------------------------------------------------------------------*/
	 /*           If the array is smaller than 2 just return              */
	 /*-------------------------------------------------------------------*/

	 if (left >= right)
	   return;

	 /*-------------------------------------------------------------------*/
	 /*                      Move the element in wc[0]                    */
	 /*-------------------------------------------------------------------*/

	 WMP_swap_fx (wc, wc_fn, left, (left+right)/2);

	 /*-------------------------------------------------------------------*/
	 /*                             Splitting                             */
	 /*-------------------------------------------------------------------*/

	 last = left;

	 for (i = left+1; i <= right; i++)
	   if (wc[i] > wc[left])
	     WMP_swap_fx (wc, wc_fn, ++last, i);

	 /*-------------------------------------------------------------------*/
	 /*                         Restore the element                       */
	 /*-------------------------------------------------------------------*/

	 WMP_swap_fx  (wc, wc_fn, left, last);

	 WMP_qsort_fx (wc, wc_fn, left, last-1);

	 WMP_qsort_fx (wc, wc_fn, last+1, right);

	 /*-------------------------------------------------------------------*/

	 return;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

/*===================================================================*/
/* FUNCTION  : WMP_print_stat_fx ().                                 */
/*-------------------------------------------------------------------*/
/* PURPOSE   : These functions print the stats for the fixed point   */
/*             WMOPS.                                                */
/*-------------------------------------------------------------------*/
/* INPUT ARGUMENTS  :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* OUTPUT ARGUMENTS :                                                */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* INPUT/OUTPUT ARGUMENTS :                                          */
/*                      _ None.                                      */
/*-------------------------------------------------------------------*/
/* RETURN ARGUMENTS :                                                */
/*                      _ None.                                      */
/*===================================================================*/

void WMP_print_stat_fx (void)
	{
	 /*-------------------------------------------------------------------*/

	 Word16	 i;
	 Word32	 tot_wc;

	 /*-------------------------------------------------------------------*/
	 /*                 Print Average WMOPS and the WORST case            */
	 /*-------------------------------------------------------------------*/

	 fprintf (stdout, "\n\n  WMIPS FIXED POINT COMPLEXITY ESTIMATION\n");
	 fprintf (stdout, "\n  Average=%.2f",
		  total_wmops_fx/(Word40) nbframe_fx);

	 fprintf (stdout, "  WorstCase=%.2f at frame %d",
		  ((Word40) glob_wc_fx) * 0.00005,  wc_frame_fx);

	 /*-------------------------------------------------------------------*/
	 /*                  Calculate the WORST cummulate case               */
	 /*-------------------------------------------------------------------*/

	 tot_wc = 0L;
	 for (i = 0; i < NB_FUNC_MAX; i++)
	   if (strcmp(wc_fn_fx [i], ""))
	     tot_wc += wc_fx[i];

	 /*-------------------------------------------------------------------*/

	 fprintf (stdout, " Sum of WorstWC=%.2f", ((Word40) tot_wc) * 0.00005);
	 fprintf (stdout, "\n");

	 /*-------------------------------------------------------------------*/
	 /*                         Sorting of worst case                     */
	 /*-------------------------------------------------------------------*/

	 fprintf (stdout, "\n");

	 WMP_qsort_fx (wc_fx, wc_fn_fx, 0, NB_FUNC_MAX-1);

	 for (i = 0; i < NB_FUNC_MAX; i++)
	   if ((wc_fx[i] != 0) && (strcmp(wc_fn_fx [i], "")))
	     fprintf (stdout, "\n%s\t\t%.2f ", wc_fn_fx[i], wc_fx[i]*0.00005);

	 fprintf (stdout, "\n");

	 /*-------------------------------------------------------------------*/

	 return;

	 /*-------------------------------------------------------------------*/
	}

/*----------------------------------------------------------------------------*/

#endif

/*============================================================================*/
/*------------------------------------- END ----------------------------------*/
/*============================================================================*/
