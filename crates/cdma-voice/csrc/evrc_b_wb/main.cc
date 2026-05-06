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
static char const rcsid[] =
    "$Id: main.cc,v 1.87.4.6 2007/06/24 06:43:14 apadmana Exp $";



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
/*  C Source Code Simulatiget_nacf_at_pitchon                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*======================================================================*/
/*  IS-127 Enhanced Variable Rate Codec, Speech Service Option 3 for    */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1996 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/
/*  Module:     main.c                                                  */
/*----------------------------------------------------------------------*/
/*  History:                                                            */
/*     11/20/95  Born                                                   */
/*----------------------------------------------------------------------*/

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include  <unistd.h>

#include "struct.h"
#include "io.h"

#define PROGRAM_VERSION "2.0"
#define PROGRAM_DATE    "13 JUNE 2007"

/* Global variable */
EvrcArgs *Eargs;
int16 ibuf_len;
int16 obuf_len;

int write_accshift = 0;


int voicedFrame;


/*======================================================================*/
/*         ..Print banner.                                              */
/*----------------------------------------------------------------------*/
void
banner (FILE * fp)
{
    /*....execute.... */
    fprintf (fp,
	     "\n----------------------------------------------------------------------\n>>>>>>>>>    CDMA Enhanced Variable Rate Coder - Wideband    <<<<<<<<<\n>>>>>>>>>>>>>>>>>>>> - Version %s, %s - <<<<<<<<<<<<<<<<<<<\n----------------------------------------------------------------------\n",
	     PROGRAM_VERSION, PROGRAM_DATE);
}

/*======================================================================*/
/*         ..Usage_ed.                                                  */
/*----------------------------------------------------------------------*/
void
usage_ed (FILE * fp, char *prog_name)
{
    /*....execute.... */
    banner (fp);
    fprintf (fp, "usage:\n%s  <required_args>  [optional_args]\n\n",
	     prog_name);
    fprintf (fp, "required_args:\n");
    fprintf (fp, "\t-i <filename>   Input filename.\n");
    fprintf (fp, "\t-o <filename>   Output filename.\n");
    fprintf (fp,
	     "\t-e/-d           Encode only (infile=speech, outfile = bitstream)/ Decode only (infile = bitstream, outfile = speech).\n");
    fprintf (fp, "optional_args:\n");
    fprintf (fp, "\t-h              Print this message.\n");
    fprintf (fp,
	     "\t-M <rate>       Maximum rate, 1(1/8),2(1/4),3(1/2) or 4; (default=4).\n");
    fprintf (fp,
	     "\t-m <rate>       Minimum rate, 1(1/8),2(1/4),3(1/2) or 4; (default=1).\n");
    fprintf (fp,
	     "\t-p <1 or 0>     Enable (1)/disable (0) post filter, (default=1).\n");
    fprintf (fp,
	     "\t-n <1 or 0>     Enable (1)/disable (0) noise suppression, (default=1).\n");
    fprintf (fp,
	     "\t-H              Disable front end IIR high-pass filter\n");
    fprintf (fp,
	     "\t-c              Do open-loop rate calibration only, no coding\n");
    fprintf (fp, "\t-u              Keep LSP unquantized.\n");
    fprintf (fp, "\t-f <#>          Stop after processing # frames\n");
    fprintf (fp, "\t-S <#>          Starting frame number, default 0\n");
    fprintf (fp, "\t-r              Write formant residual to stdout\n");
    fprintf (fp,
	     "\t-a              Write quantized formant residual to stdout\n");
    fprintf (fp, "\t-R              Write modified residual to stdout\n");
    fprintf (fp,
	     "\t-E              Write target speech at the encoder to stdout\n");
    fprintf (fp, "\t-E              Write LPCs at the decoder to stdout\n");
    fprintf (fp,
	     "\t-L              Generate narrowband output at the decoder\n");
    fprintf (fp,
	     "\t-Y <0|1|2>      Select CELP (0), MDCT (1) or Switch between CELP and MDCT (2) for full rate coding. Default is CELP\n");
    fprintf (fp, "\t-G              Write rate to stdout\n");
    fprintf (fp, "\t-O              Write lag to stdout\n");
    fprintf (fp, "\t-N              Write current frame NACF to stdout\n");
    fprintf (fp, "\t-P              Unquantized prototype in half rate\n");
    fprintf (fp,
	     "\t-C <1|2|3|4>    Unquantized 1/8, 1/4, 1/2 or full rate  frames\n");
    fprintf (fp,
	     "\t-I <rate file>  Use external rate file to overwrite rate decision\n");
    fprintf (fp, "\t-k              Write packet to stdout\n");
    fprintf (fp, "\t-v              Write VAD to stdout \n");
    fprintf (fp, "\t-v              Write acb gain in decoder to stdout \n");
    fprintf (fp,
	     "\t-A <[F|f]#1<R|r>#1[<F|f>#2<R|r>#2 ...]> Force frame <#1> to rate <#1>,\n\t                up to 10 frames\n");
    fprintf (fp,
	     "\t-F              Control PPP threshold to keep avg rate fixed\n");
    fprintf (fp,
	     "\t-T <threshold>  The value of the above threshold (Default:-150.0dB)\n");
    fprintf (fp,
	     "\t-W <avg_rate>   Try to keep avg rate within a range of the\n\t                specified number in kbps (default 7.2)\n");
    fprintf (fp,
	     "\t-w <rate win>   Use this number of active frames over which to compute\n\t                average rate (default 100)\n");
    fprintf (fp,
	     "\t-X <options>    Extra Options with name=value format,\n\t                separated by commas if more than one\n");
    //Time-Warping: Added to support time-warping option
    fprintf (fp, "\t-D              Enable DTX(default: disabled)\n");
    fprintf (fp,
	     "\t-Q              Time Warping 0=None -1=Compress 1=Expand\n");
    fprintf (fp, "\t-s <filename>   Dim and Burst signaling\n");


    //Added for Support of Phase Matching/Warping in NB modes
    fprintf (fp, "\t-Y <1 or 0>     Enable (1)/disable (0) Phase Matching\n");
    fprintf (fp,
	     "\t-Z <1 or 0>     Enable (1)/disable (0) concealing double erasures using Phase Matching\n");
    fprintf (fp, "\t-V <1 or 0>     Enable (1)/disable (0) Time Warping\n");
    //End: Added for Support of Phase Matching/Warping
    fprintf (fp, "\n");
}

/*======================================================================*/
/*         ..Usage_e.                                                   */
/*----------------------------------------------------------------------*/
void
usage_e (FILE * fp, char *prog_name)
{
    /*....execute.... */
    banner (fp);
    fprintf (fp, "usage:\n%s  <required_args>  [optional_args]\n\n",
	     prog_name);
    fprintf (fp, "required_args:\n");
    fprintf (fp, "\t <filename>   Input speech filename.\n");
    fprintf (fp, "\t <filename>   Output bitstream filename.\n");
    fprintf (fp, "\t-m M  operating point 0, or 1, or 2: Default is OP2\n");
    fprintf (fp, "optional_args:\n");
    fprintf (fp,
	     "\t-f <mode file>  Use external file for external override switching modes\n");
    fprintf (fp, "\t-s <signalling file>  Use signalling file \n");
    fprintf (fp, "\t-v To run executable in verbose mode \n");
    fprintf (fp, "\n");
}

/*======================================================================*/
/*         ..Usage_d.                                                   */
/*----------------------------------------------------------------------*/
void
usage_d (FILE * fp, char *prog_name)
{
    /*....execute.... */
    banner (fp);
    fprintf (fp,
	     "usage:\n%s  <input_bitstream_file>  <output_speech_file> [-v]\n\n",
	     prog_name);
    fprintf (fp, "\t-v To run executable in verbose mode \n");
    fprintf (fp, "\n");
}

/*======================================================================*/
/*         ..Print command line arguments.                              */
/*----------------------------------------------------------------------*/
void
print_eargs_ed (FILE * fp, EvrcArgs * e)
{
    int16 i;

    /*....execute.... */
    banner (fp);
    if (e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = BITSTREAM_BUFFER_LEN - 1;

	fprintf (fp, "         input_speechfile == \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "         output_bitstream => \"%s\"\n",
		 e->output_filename);

    }
    else if (e->decode_only && !e->encode_only) {
	e->ibuf_len = BITSTREAM_BUFFER_LEN - 1;
	e->obuf_len = SPEECH_BUFFER_LEN;
	fprintf (fp, "          input_bitstream => \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "        output_speechfile == \"%s\"\n",
		 e->output_filename);
    }
    else if (!e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = SPEECH_BUFFER_LEN;
	fprintf (fp, "         input_speechfile == \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "        output_speechfile == \"%s\"\n",
		 e->output_filename);
    }

    if (e->max_rate_default) {
	fprintf (fp, "                  max_rate == %d\n", e->max_rate);
    }
    else {
	fprintf (fp, "                  max_rate => %d\n", e->max_rate);
    }

    if (e->min_rate_default) {
	fprintf (fp, "                  min_rate == %d\n", e->min_rate);
    }
    else {
	fprintf (fp, "                  min_rate => %d\n", e->min_rate);
    }

    if (e->post_filter) {
	if (e->post_filter_default) {
	    fprintf (fp, "               post_filter == ON\n");
	}
	else {
	    fprintf (fp, "               post_filter => ON\n");
	}
    }
    else {
	if (e->post_filter_default) {
	    fprintf (fp, "               post_filter == OFF\n");
	}
	else {
	    fprintf (fp, "               post_filter => OFF\n");
	}
    }

    if (e->noise_suppression) {
	if (e->noise_suppression_default) {
	    fprintf (fp, "         noise_suppression == ON\n");
	}
	else {
	    fprintf (fp, "         noise_suppression => ON\n");
	}
    }
    else {
	if (e->noise_suppression_default) {
	    fprintf (fp, "         noise_suppression == OFF\n");
	}
	else {
	    fprintf (fp, "         noise_suppression => OFF\n");
	}
    }

    if (e->avg_rate_control == 0)
	fprintf (fp, "\tKeeping the PPP-CELP Threshold fixed to %5.4fdB\n",
		 e->PPP_to_CELP_threshold);
    else
	fprintf (fp, "\tInitial PPP-CELP Threshold is %5.4fdB\n",
		 e->PPP_to_CELP_threshold);
    if (e->highpass_filter)
	fprintf (fp, "\tFront-end highpass filter == ON\n");
    else
	fprintf (fp, "\tFront-end highpass filter == OFF\n");

    if (e->rfileP)
	fprintf (fp, "\tUse external rate file \"%s\"\n", e->rate_filename);
    if (e->unquantized_lsp)
	fprintf (fp, "\tLSP not quantized\n");
    if (e->starting_frame_num > 0)
	fprintf (fp, "\tStart processing at frame %d\n",
		 e->starting_frame_num);
    if (e->partial_file_processing)
	fprintf (fp, "\tProcess only %d frames\n", e->num_frames);
    if (e->form_res_out)
	fprintf (fp, "\tWriting original formant residual to stdout\n");
    if (e->qform_res_out)
	fprintf (fp, "\tWriting quantized formant residual to stdout\n");
    if (e->mform_res_out)
	fprintf (fp, "\tWriting RCELP residual to stdout\n");
    if (e->target_speech_out)
	fprintf (fp, "\tWriting encoder target speech to stdout\n");
    if (e->rate_out)
	fprintf (fp, "\tWriting rate to stdout\n");
    if (e->olr_calibration)
	fprintf (fp, "\tDo open-loop rate calibration only, no coding\n");
    if (e->lag_out)
	fprintf (fp, "\tWriting pitch to stdout\n");
    if (e->nacf_out)
	fprintf (fp, "\tWriting current NACF to stdout\n");
    if (e->packet_out)
	fprintf (fp, "\tWriting packet to stdout\n");
    if (e->unquantized_prototype)
	fprintf (fp, "\tPPP prototype not quantized\n");
    if (e->unquantized_zero_rate)
	fprintf (fp, "\tEighth rate frames not quantized\n");
    if (e->unquantized_quarter_rate)
	fprintf (fp, "\tQuarter rate frames not quantized\n");
    if (e->unquantized_half_rate)
	fprintf (fp, "\tHalf rate frames not quantized\n");
    if (e->unquantized_full_rate)
	fprintf (fp, "\tFull rate frames not quantized\n");
    if (e->vad_out)
	fprintf (fp, "\tWriting VAD to stdout \n");
    if (!e->encode_only && e->lowband_decout)
	fprintf (fp, "\tDecoder output is Narrowband \n");

    if (e->fullrate_coding_method == 1)
	fprintf (fp, "\tAll full rate frames are coded in the MDCT domain\n");
    else if (e->fullrate_coding_method == 2)
	fprintf (fp,
		 "\tFull rate frames coded with MDCT or CELP based on detector\n");
    if (e->forced_count > 0)
	for (i = 0; i < e->forced_count; i++)
	    fprintf (fp, "\tFrame %d is forced to rate %d\n",
		     e->forced_frame[i], e->forced_rate[i]);
    /*  if(e-> accshift_out)
       fprintf(fp,"\tWrite accumulated shift to stdout\n");
     */
    fprintf (fp,
	     "---------------------------------------------------------------------\n");
    fprintf (fp, "\tRunning in OP-%d\n", e->operating_point);
    if (e->operating_point == 2)
	fprintf (fp, "\t'%s' Pattern used\n", e->pattern);
    if (e->signalling_file != NULL)
	fprintf (fp, "\tDimming being caused\n");
    if (e->erasure_file != NULL)
	fprintf (fp, "\tErasures being inserted\n");
    if (e->dtx == 1)
	fprintf (fp, "\tDTX enabled \n");

}

void
print_eargs_e (FILE * fp, EvrcArgs * e)
{
    int16 i;

    /*....execute.... */
    if (e->verbose)
	banner (fp);
    if (e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = BITSTREAM_BUFFER_LEN - 1;

	if (e->verbose) {
	    fprintf (fp, "         input_speechfile == \"%s\"\n",
		     e->input_filename);
	    fprintf (fp, "         output_bitstream => \"%s\"\n",
		     e->output_filename);
	}

    }
    else if (e->decode_only && !e->encode_only) {
	e->ibuf_len = BITSTREAM_BUFFER_LEN - 1;
	e->obuf_len = SPEECH_BUFFER_LEN;
	fprintf (fp, "          input_bitstream => \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "        output_speechfile == \"%s\"\n",
		 e->output_filename);
    }
    else if (!e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = SPEECH_BUFFER_LEN;
	fprintf (fp, "         input_speechfile == \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "        output_speechfile == \"%s\"\n",
		 e->output_filename);
    }

    if (e->verbose) {
	fprintf (fp, "\tRunning in OP-%d\n", e->operating_point);
	if (e->signalling_file != NULL)
	    fprintf (fp, "\tDimming being caused\n");
    }

}

void
print_eargs_d (FILE * fp, EvrcArgs * e)
{
    int16 i;

    /*....execute.... */
    if (e->verbose)
	banner (fp);
    if (e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = BITSTREAM_BUFFER_LEN - 1;

	fprintf (fp, "         input_speechfile == \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "         output_bitstream => \"%s\"\n",
		 e->output_filename);

    }
    else if (e->decode_only && !e->encode_only) {
	e->ibuf_len = BITSTREAM_BUFFER_LEN - 1;
	e->obuf_len = SPEECH_BUFFER_LEN;
	if (e->verbose) {
	    fprintf (fp, "          input_bitstream => \"%s\"\n",
		     e->input_filename);
	    fprintf (fp, "        output_speechfile == \"%s\"\n",
		     e->output_filename);
	}
    }
    else if (!e->encode_only && !e->decode_only) {
	e->ibuf_len = SPEECH_BUFFER_LEN;
	e->obuf_len = SPEECH_BUFFER_LEN;
	fprintf (fp, "         input_speechfile == \"%s\"\n",
		 e->input_filename);
	fprintf (fp, "        output_speechfile == \"%s\"\n",
		 e->output_filename);
    }

    if (e->signalling_file != NULL)
	if (e->verbose)
	    fprintf (fp, "\tDimming being caused\n");
}

/*======================================================================*/
/*         ..Get command line arguments.                                */
/*----------------------------------------------------------------------*/
extern int optind;

EvrcArgs *
get_eargs (int argc, char *argv[])
{
    /*....(local) variables.... */
    EvrcArgs *eargs;
    int16 option;

    char frame_rate[80];

    char *num;

    char temp[10];

    if (strstr (argv[0], "_enc") != NULL) {
	if (argc < 5 || argc > 10)
	    eargs = NULL;
	else {
	    if ((eargs = (EvrcArgs *) malloc (sizeof (EvrcArgs))) == NULL) {
		fprintf (stderr,
			 "%s:  ERROR - Unable to malloc arg memory.\n",
			 argv[0]);
		exit (-1);
	    }
	    eargs->encode_only = 1;
	    eargs->decode_only = 0;
	}
    }
    else if (strstr (argv[0], "_dec") != NULL) {
	if (argc < 3 || argc > 4)
	    eargs = NULL;
	else {
	    if ((eargs = (EvrcArgs *) malloc (sizeof (EvrcArgs))) == NULL) {
		fprintf (stderr,
			 "%s:  ERROR - Unable to malloc arg memory.\n",
			 argv[0]);
		exit (-1);
	    }
	    eargs->encode_only = 0;
	    eargs->decode_only = 1;
	}
    }
    else {
	if (argc < 5)
	    eargs = NULL;
	else {
	    if ((eargs = (EvrcArgs *) malloc (sizeof (EvrcArgs))) == NULL) {
		fprintf (stderr,
			 "%s:  ERROR - Unable to malloc arg memory.\n",
			 argv[0]);
		exit (-1);
	    }
	    eargs->encode_only = 0;
	    eargs->decode_only = 0;
	}
    }

    if (eargs != NULL) {
	eargs->input_filename = NULL;
	eargs->output_filename = NULL;
	eargs->max_rate = 4;
	eargs->max_rate_default = 1;
	eargs->min_rate = 1;
	eargs->min_rate_default = 1;
	eargs->post_filter = 1;
	eargs->post_filter_default = 1;
	eargs->noise_suppression = 1;
	eargs->noise_suppression_default = 1;
	eargs->ibuf_len = SPEECH_BUFFER_LEN;
	eargs->obuf_len = SPEECH_BUFFER_LEN;

	eargs->highpass_filter = 1;
	eargs->olr_calibration = 0;

	eargs->unquantized_lsp = 0;
	eargs->partial_file_processing = 0;
	eargs->starting_frame_num = 0;
	eargs->num_frames = 0;
	eargs->form_res_out = 0;
	eargs->qform_res_out = 0;
	eargs->mform_res_out = 0;
	eargs->target_speech_out = 0;
	eargs->rate_out = 0;
	eargs->lag_out = 0;
	eargs->nacf_out = 0;
	eargs->unquantized_prototype = 0;
	eargs->unquantized_zero_rate = 0;
	eargs->unquantized_quarter_rate = 0;
	eargs->unquantized_half_rate = 0;
	eargs->unquantized_full_rate = 0;
	eargs->rfileP = NULL;
	eargs->packet_out = 0;
	eargs->vad_out = 0;
	eargs->forced_count = 0;
	eargs->accshift_out = 0;
	eargs->avg_rate_target = 5.3;
	eargs->avg_rate_control = 0;
	eargs->PPP_to_CELP_threshold = -150.0;
	eargs->ratewin = 100;
	eargs->Fsop = 16000;
	eargs->Fsinp = 16000;

	eargs->fullrate_coding_method = 0;

	eargs->operating_point = 3;
	strcpy (eargs->pattern, "QQF");
	eargs->erasure_file = NULL;
	eargs->signalling_file = NULL;
	eargs->verbose = NO;
	eargs->dtx = 0;
	eargs->lowband_decout = 0;

	//Added for Support of Time-Warping        
	eargs->time_warping = 0;
	//End:Added for Support of Time-Warping

	//Added for Support of Phase Matching/Warping in NB Modes
	eargs->phase_matching = 0;
	eargs->double_erasures_pm = 0;
	eargs->time_warping = 0;
	//End:Added for Support of Phase Matching/Warping


	if (strstr (argv[0], "_enc") != NULL) {
	    //int fileflag=0;
	    while ((option = getopt (argc, argv, "m:f:s:v:")) != EOF) {
		//while (optind<argc) {}
		// option=getopt(argc,argv,"m:f:s:v");
		switch (option) {
		case 'm':
		    eargs->operating_point = (int16) atoi (argv[optind - 1]);
		    break;
		case 'f':
		    break;
		case 's':
		    if ((eargs->signalling_file =
			 fopen (argv[optind - 1], "rb")) == NULL) {
			fprintf (stderr, "Cannot Open Signalling File %s - ",
				 argv[optind - 1]);
			fprintf (stderr, "Running without dim and burst\n");
		    }
		    break;
		case 'v':
		    eargs->verbose = YES;
		    break;
/*          case 'Y':
        eargs->fullrate_coding_method = (int16)atoi(argv[optind-1]);
		        break;
*/ default:
		    //if (fileflag==0) {eargs->input_filename=argv[optind];fileflag++;}
		    //else if(fileflag==1){eargs->output_filename=argv[optind];fileflag++;}
		    //else {printf("Illegal option, optind = %d\n", optind);}
		    //optind++;
		    printf ("Illegal option, optind = %d, option = %s\n",
			    optind, argv[optind - 1]);
		    usage_e (stderr, argv[0]);
		    exit (-1);
		}
	    }
	    eargs->input_filename = argv[argc - 2];
	    eargs->output_filename = argv[argc - 1];
	    return eargs;
	}
	else if (strstr (argv[0], "_dec") != NULL) {
	    //int fileflag=0;
	    //while (optind<argc) {}
	    //option=getopt(argc,argv,"v");
	    while ((option = getopt (argc, argv, "v:L")) != EOF) {
		switch (option) {
		case 'v':
		    eargs->verbose = YES;
		    break;
		case 'L':
		    eargs->lowband_decout = YES;
		    break;
		default:
		    //if (fileflag==0) {eargs->input_filename=argv[optind];fileflag++;}
		    //else if(fileflag==1){eargs->output_filename=argv[optind];fileflag++;}
		    //else {printf("Illegal option\n");}
		    //optind++;
		    printf ("Illegal option, optind = %d, option = %s\n",
			    optind, argv[optind - 1]);
		    usage_e (stderr, argv[0]);
		    exit (-1);
		    //break;
		}
	    }
	    eargs->input_filename = argv[argc - 2];
	    eargs->output_filename = argv[argc - 1];
	    return eargs;
	}
	else {
//fprintf(stderr,"atleast came here\n");fflush(stderr);
	    //eargs->verbose=YES;
	    //Time-Warping: Added :Q below in while for Support of Time-Warping
	    while ((option =
		    getopt (argc, argv,
			    "X:aA:cC:dEef:FGhHi:I:kLM:m:Oo:Pp:Nn:RrS:s:T:uvxVQ:DY:Z:V:D"))
		   != EOF) {
		switch (option) {

		    //Time-Warping: Added for Support of Time-Warping
		case 'Q':
		    eargs->time_warping = atoi (argv[optind - 1]);
		    printf ("\n Time Warping %d %s", eargs->time_warping,
			    argv[optind - 1]);
		    break;
		    //End: Time-Warping: Added for Support of Time-Warping
		    //Added for Support of Phase Matching/Warping
		case 'Y':
		    eargs->phase_matching = atoi (argv[optind - 1]);
		    break;
		case 'Z':
		    eargs->double_erasures_pm = atoi (argv[optind - 1]);
		    break;
		case 'V':
		    eargs->time_warping = atoi (argv[optind - 1]);
		    break;
		    //End:Added for Support of Phase Matching/Warping

		case 'X':
		    {
			char arg[200], *sx = arg, name[20], value[200], *p;

			strcpy (arg, argv[optind - 1]);
			int i = 0, nc = 0, nt = 0;

			do {
			    char *p1 = p = strchr (sx, ',');

			    if (p == NULL)
				nc = strlen (sx);
			    else
				nc = p - sx;
			    p = strchr (sx, '=');
			    if (p == NULL) {
				fprintf (stderr,
					 "Bad Format for option 'X'\n");
				exit (-1);
			    }
			    else
				nt = p - sx;
			    strncpy (name, sx, nt);
			    name[nt] = '\0';
			    strncpy (value, sx + nt + 1, nc - nt - 1);
			    value[nc - nt - 1] = '\0';
			    if (!strcmp (name, "OP")) {
				sscanf (value, "%d", &eargs->operating_point);
				strcpy (temp, value);

				if (strcmp (temp, "Music") == 0) {
				    eargs->fullrate_coding_method = 2;
				    eargs->operating_point = 3;
				}	//switch
				if (strcmp (temp, "Music_MDCT") == 0) {
				    eargs->fullrate_coding_method = 1;
				    eargs->operating_point = 3;
				}	//all mdct


				if (eargs->operating_point < 0
				    || eargs->operating_point > 3) {
				    fprintf (stderr,
					     "Bad Operating Point %s\n",
					     value);
				    exit (-1);
				}
			    }
			    else if (!strcmp (name, "Fsop")) {
				sscanf (value, "%d", &eargs->Fsop);
				if ((eargs->Fsop != 8000)
				    && (eargs->Fsop != 16000)) {
				    fprintf (stderr,
					     "Bad o/p sampling frequency %s\n",
					     value);
				    exit (-1);
				}
			    }
			    else if (!strcmp (name, "Fsinp")) {
				sscanf (value, "%d", &eargs->Fsinp);
				if ((eargs->Fsinp != 8000)
				    && (eargs->Fsinp != 16000)) {
				    fprintf (stderr,
					     "Bad input sampling frequency %s\n",
					     value);
				    exit (-1);
				}
			    }
			    else if (!strcmp (name, "pattern")) {
				strcpy (eargs->pattern, value);
				if (strcmp (eargs->pattern, "QQF")
				    && strcmp (eargs->pattern, "QHQF")
				    && strcmp (eargs->pattern, "FQFQ")
				    && strcmp (eargs->pattern, "FFQQ")) {
				    fprintf (stderr, "Bad Pattern %s\n",
					     value);
				    exit (-1);
				}
			    }
			    else if (!strcmp (name, "erasure_file")) {
				if ((eargs->erasure_file =
				     fopen (value, "rb")) == NULL) {
				    fprintf (stderr,
					     "Cannot Open Erasure File %s - ",
					     value);
				    fprintf (stderr,
					     "Running without erasures\n");
				}
			    }
			    else if (!strcmp (name, "signalling_file")) {
				if ((eargs->signalling_file =
				     fopen (value, "rb")) == NULL) {
				    fprintf (stderr,
					     "Cannot Open Signalling File %s - ",
					     value);
				    fprintf (stderr,
					     "Running without dim and burst\n");
				}
			    }

			    if (p1 == NULL)
				break;
			    sx += nc + 1;
			    i++;
			} while (strlen (sx) != 0);
		    }
		    break;
		case 'h':
		    usage_ed (stderr, argv[0]);
		    exit (-1);
		    break;
		case 'i':
		    eargs->input_filename = argv[optind - 1];
		    break;
		case 'o':
		    eargs->output_filename = argv[optind - 1];
		    break;
		case 'e':
		    eargs->encode_only = 1;
		    if (eargs->decode_only) {
			fprintf (stderr, "%s:  ERROR - Encode with decode.\n",
				 argv[0]);
			exit (-1);
		    }
		    break;
		case 'd':
		    eargs->decode_only = 1;
		    if (eargs->encode_only) {
			fprintf (stderr, "%s:  ERROR - Decode with encode.\n",
				 argv[0]);
			exit (-1);
		    }
		    break;
		case 'M':
		    eargs->max_rate = (int16) atoi (argv[optind - 1]);
		    if (eargs->max_rate > 8)
			eargs->max_rate = 8;
		    if (eargs->max_rate < 1)
			eargs->max_rate = 1;
		    eargs->max_rate_default = 0;
		    break;
		case 'm':
		    eargs->min_rate = (int16) atoi (argv[optind - 1]);
		    if (eargs->min_rate > 8)
			eargs->min_rate = 8;
		    if (eargs->min_rate < 1)
			eargs->min_rate = 1;
		    eargs->min_rate_default = 0;
		    break;
		    /*   case 'Y':
		       eargs->fullrate_coding_method = (int16)atoi(argv[optind-1]);
		       break;
		 */ case 'p':
		    eargs->post_filter = (int16) atoi (argv[optind - 1]);
		    if (eargs->post_filter != 0)
			eargs->post_filter = 1;
		    eargs->post_filter_default = 0;
		    break;
		case 'n':
		    eargs->noise_suppression =
			(int16) atoi (argv[optind - 1]);
		    if (eargs->noise_suppression != 0)
			eargs->noise_suppression = 1;
		    eargs->noise_suppression_default = 1;
		    break;
		case 'u':
		    eargs->unquantized_lsp = 1;
		    break;
		case 'f':
		    eargs->partial_file_processing = 1;
		    eargs->num_frames = (int16) atoi (argv[optind - 1]);
		    fprintf (stderr, "%d\n", eargs->num_frames);
		    break;
		case 'S':
		    eargs->starting_frame_num =
			(int16) atoi (argv[optind - 1]);
		    break;
		case 'r':
		    eargs->form_res_out = 1;
		    break;
		case 'a':
		    eargs->qform_res_out = 1;
		    break;
		case 'R':
		    eargs->mform_res_out = 1;
		    break;
		case 'E':
		    eargs->target_speech_out = 1;
		    break;
		case 'G':
		    eargs->rate_out = 1;
		    break;
		case 'O':
		    eargs->lag_out = 1;
		    break;
		case 'P':
		    eargs->unquantized_prototype = 1;
		    break;
		case 'C':
		    switch (atoi (argv[optind - 1])) {
		    case 1:
			eargs->unquantized_zero_rate = 1;
			break;
		    case 2:
			eargs->unquantized_quarter_rate = 1;
			break;
		    case 3:
			eargs->unquantized_half_rate = 1;
			break;
		    case 4:
			eargs->unquantized_full_rate = 1;
			break;
		    default:
			break;
		    }
		    break;
		case 'I':
		    eargs->rate_filename = argv[optind - 1];
		    if ((eargs->rfileP =
			 fopen (eargs->rate_filename, "rb")) == NULL) {
			fprintf (stderr,
				 "%s:  ERROR - Unable to open rate file \"%s\".\n",
				 argv[0], eargs->rate_filename);
			exit (-1);
		    }
		    break;
		case 'k':
		    eargs->packet_out = 1;
		    break;
		case 'N':
		    eargs->nacf_out = 1;
		    break;
		case 'v':
		    eargs->vad_out = 1;
		    break;
		case 'A':
		    sprintf (frame_rate, "%s", argv[optind - 1]);
		    num = strtok (frame_rate, " FfRr");
		    while (num != NULL && eargs->forced_count < 10) {
			eargs->forced_frame[eargs->forced_count] = atoi (num);
			num = strtok (NULL, "Ff");
			if (num != NULL)
			    eargs->forced_rate[eargs->forced_count++] =
				atoi (num);
			num = strtok (NULL, "Rr");
		    }
		    break;
		case 'x':
		    write_accshift = 1;
		    break;
		case 'H':
		    eargs->highpass_filter = 0;
		    break;
		case 'F':
		    eargs->avg_rate_control = 1;
		    break;
		case 'T':
		    eargs->PPP_to_CELP_threshold =
			(float) atof (argv[optind - 1]);
		    break;
		case 'W':
		    eargs->avg_rate_target = (int16) atoi (argv[optind - 1]);
		    eargs->avg_rate_control = 1;
		    break;
		case 'w':
		    eargs->ratewin = (int16) atoi (argv[optind - 1]);
		    break;
		case 'c':
		    eargs->olr_calibration = 1;
		    break;
		case 'D':
		    eargs->dtx = 1;
		    break;
/*        case 'V':
          eargs->verbose=YES;
		          break;
*/ case 'L':
		    eargs->lowband_decout = YES;
		    break;
		case 's':
		    if ((eargs->signalling_file =
			 fopen (argv[optind - 1], "rb")) == NULL) {
			fprintf (stderr, "Cannot Open Signalling File %s - ",
				 argv[optind - 1]);
			fprintf (stderr, "Running without dim and burst\n");
		    }
		    break;
		}
	    }
	    if (eargs->min_rate > eargs->max_rate) {
		eargs->min_rate = eargs->max_rate;
	    }
	    return (eargs);
	}
    }
    else
	return (eargs);
}

/*======================================================================*/
/*         ..Returns number of frames in a binary file.                 */
/*----------------------------------------------------------------------*/
int32
GetNumFrames (FILE * fp, int16 blocksize)
{
    /*....(local) variables.... */
    int32 position;
    int32 numFrames;

    /*....execute.... */
    if (Eargs->starting_frame_num > 0) {
	fseek (fp, blocksize * Eargs->starting_frame_num, SEEK_SET);
	if (Eargs->rfileP)
	    fseek (Eargs->rfileP, sizeof (int16) * Eargs->starting_frame_num,
		   SEEK_SET);
    }
    position = ftell (fp);
    fseek (fp, 0L, 2);
    numFrames = (ftell (fp) - position) / blocksize;
    fseek (fp, position, 0);
    return (numFrames);
}

//float NS_SNR;

/*======================================================================*/
/*         ..Main.                                                      */
/*----------------------------------------------------------------------*/
int
main (int argc, char *argv[])
{
    /*....(local) variables.... */

    FILE *ifileP;
    FILE *ofileP;







    int32 buf_count;
    int32 j;
    int16 k;
    float rate_sum;
    float avg_rate;
    int32 rate_cnt[4 + 12 + 1];
    int32 rate_cnt_dec[5 + 11 + 1];
    float chan_rate, voc_rate;

    int16 evrc_rate;

    FGV_MEM FGV_A, FGV_B;

    //Time-Warping Variable below
    float time_warp_fraction = 1.5;
    int num_non_warped = 0;



    /*....execute.... */
    /*...get arguments and check usage... */
    if ((Eargs = get_eargs (argc, argv)) == NULL) {
	if (strstr (argv[0], "_enc") != NULL)
	    usage_e (stderr, argv[0]);
	else if (strstr (argv[0], "_dec") != NULL)
	    usage_d (stderr, argv[0]);
	else
	    usage_ed (stderr, argv[0]);
	exit (-1);
    }
    if (Eargs->encode_only == 0 && Eargs->decode_only == 0) {
	fprintf (stderr,
		 "\n %s ERROR - Encoder and Decoder cannot be jointly run. This feature is currently disabled. \n");
	usage_ed (stderr, argv[0]);
	exit (-1);
    }
    if (strstr (argv[0], "_enc") != NULL)
	print_eargs_e (stderr, Eargs);
    else if (strstr (argv[0], "_dec") != NULL)
	print_eargs_d (stderr, Eargs);
    else
	print_eargs_ed (stderr, Eargs);

    /*...open files... */
    if ((ifileP = fopen (Eargs->input_filename, "rb")) == NULL) {
	fprintf (stderr, "%s:  ERROR - Unable to open input file \"%s\".\n",
		 argv[0], Eargs->input_filename);
	exit (-1);
    }

    if ((ofileP = fopen (Eargs->output_filename, "wb")) == NULL) {
	fprintf (stderr, "%s:  ERROR - Unable to open output file \"%s\".\n",
		 argv[0], Eargs->output_filename);
	exit (-1);
    }

    /*...loop counter max... */
    if (Eargs->decode_only) {
	buf_count =
	    GetNumFrames (ifileP, sizeof (int16) * BITSTREAM_BUFFER_LEN);
    }
    else {
	if (Eargs->Fsinp == 16000)	//WB input
	{
	    buf_count =
		GetNumFrames (ifileP, sizeof (int16) * SPEECH_BUFFER_LEN * 2);
	}
	else if (Eargs->Fsinp == 8000) {	//NB input
	    buf_count =
		GetNumFrames (ifileP, sizeof (int16) * SPEECH_BUFFER_LEN);
	}
    }
    if (!Eargs->partial_file_processing)
	Eargs->num_frames = buf_count;






    /*...processing loop... */
    FGV_A.InitEncoder ();
    FGV_B.InitDecoder ();
    rate_sum = 0.0;
    avg_rate = 0.0;

    ibuf_len = Eargs->ibuf_len;
    obuf_len = Eargs->obuf_len;

    for (j = 0; j < 4 + 12 + 1; j++)
	rate_cnt[j] = 0;
    for (j = 0; j < 5 + 11 + 1; j++)
	rate_cnt_dec[j] = 0;
    voc_rate = 0;
    chan_rate = 0;



    j = 1;

    int NS;

    if (Eargs->Fsinp == 16000)	//WB encoding
	NS = 320;
    else if (Eargs->Fsinp == 8000)
	NS = 160;

    if (!Eargs->decode_only) {
	if (Eargs->Fsinp == 16000)
	    fread (FGV_A.buf16, sizeof (int16), 172, ifileP);	/*discard 10.75 ms to align with Input */
	else
	    fread (FGV_A.buf16, sizeof (int16), 40, ifileP);	/*discard 5 ms to align with EVRC */

	while (((fread (FGV_A.buf16, sizeof (int16), NS, ifileP)) ==
		(unsigned short) (NS))
	       && ((Eargs->partial_file_processing == 0)
		   || (Eargs->partial_file_processing
		       && (Eargs->num_frames >= j)))) {
	    if (Eargs->Fsinp == 8000)
		for (FGV_A.buf16P = FGV_A.buf16, FGV_A.bufP =
		     FGV_A.buf + LOOKAHEAD_LEN, k = 0; k < ibuf_len;
		     k++, FGV_A.buf16P++, FGV_A.bufP++) {
		    *FGV_A.bufP = (float) *FGV_A.buf16P;
		}




	    FGV_A.WB_encoder ();	//does WB encoding



	    if (Eargs->verbose)
		fprintf (stderr, "Encoding %ld of %ld\r", j, buf_count);

	    if (Eargs->rate_out) {
		int temp_rate;

		if (FGV_A.rate == 3) {
		    if (FGV_A.rcelp_half_rateE || FGV_A.dim_and_burstE)
			temp_rate = 6;
		    else if (FGV_A.PPP_MODE_E == 'Q')
			temp_rate = 9;
		    else if (FGV_A.PPP_MODE_E == 'H')
			temp_rate = 12;
		    else {
			if (FGV_A.PPP_BUMPUP)
			    temp_rate = 16;
			else
			    temp_rate = 15;
		    }
		}
		else if (FGV_A.rate == 4) {
		    if (FGV_A.PPP_BUMPUP)
			temp_rate = 8;
		    else
			temp_rate = FGV_A.rate;
		}
		else
		    temp_rate = FGV_A.rate;
		write_ints (1, &temp_rate, 1);
	    }
	    switch (FGV_A.rate) {
	    case 1:
		rate_sum += 1;
		rate_cnt[0]++;
		voc_rate += 0.8;
		chan_rate += 1.2;
		break;
	    case 2:
		rate_sum += 2;
		rate_cnt[1]++;
		voc_rate += 2.0;
		chan_rate += 2.4;
		break;
	    case 3:
		if (FGV_A.rcelp_half_rateE || FGV_A.dim_and_burstE) {
		    rate_sum += 4;
		    rate_cnt[6]++;
		    rate_cnt[2]++;
		    voc_rate += 4.0;
		    chan_rate += 4.8;
		}
		else
		    switch (FGV_A.PPP_MODE_E) {
		    case 'Q':
			rate_sum += 2;
			rate_cnt[9]++;
			voc_rate += 2.0;
			chan_rate += 2.4;
			break;
		    case 'H':
			rate_sum += 4;
			rate_cnt[12]++;
			rate_cnt[2]++;
			voc_rate += 4.0;
			chan_rate += 4.8;
			break;
		    case 'F':
			rate_sum += 8;
			if (FGV_A.PPP_BUMPUP)
			    rate_cnt[16]++;
			rate_cnt[15]++;
			voc_rate += 8.55;
			chan_rate += 9.6;
			break;
		    }
		break;
	    default:
		if (FGV_A.PPP_BUMPUP == 1)
		    rate_cnt[8]++;
		rate_sum += 8;
		rate_cnt[3]++;
		voc_rate += 8.55;
		chan_rate += 9.6;
		break;
	    }
	    avg_rate = (rate_sum / (float) j) * 1.2;

	    if (Eargs->olr_calibration) {
		j++;
		for (k = 0; k < FSIZE; k++)
		    FGV_A.buf[k] = FGV_A.buf[k + FSIZE];
		for (k = 0; k < LPCORDER; k++)
		    FGV_A.Oldlsp_nq[k] = FGV_A.lsp_nq[k];
		FGV_A.pdelay = FGV_A.delay;
		continue;
	    }

	    if (Eargs->encode_only) {
		fwrite (&FGV_A.data_packet.PACKET_RATE, sizeof (int16), 1,
			ofileP);
		fwrite (FGV_A.buf16, sizeof (int16), obuf_len, ofileP);
	    }
	    else {




		// copy over from FGV_A enc class to FGV_B dec class 
		FGV_B.data_packet.PACKET_RATE = FGV_A.data_packet.PACKET_RATE;
		FGV_B.rate = FGV_B.data_packet.PACKET_RATE;
		//copying some diagnostic params from encoder to dec
		for (k = 0; k < BITSTREAM_BUFFER_LEN - 1; k++)
		    FGV_B.buf16[k] = FGV_A.buf16[k];
		FGV_B.sanity_check = FGV_A.sanity_check;
		for (k = 0; k < 160; k++) {
		    FGV_B.SANITYCHECK[k] = FGV_A.SANITYCHECK[k];	//diag
		    FGV_B.copy[k] = FGV_A.copy[k];
		}

		// Erasure Insertion
		if (Eargs->erasure_file != NULL) {
		    char Err_Mask;
		    fread (&Err_Mask, sizeof (char), 1, Eargs->erasure_file);
		    if (Err_Mask == 1)
			FGV_B.rate = FGV_B.data_packet.PACKET_RATE = 0xE;
		}

		/* copy unquantized LSPs */
		if (Eargs->unquantized_lsp) {
		    for (k = 0; k < ORDER; k++)
			FGV_B.global_lsp[k] = FGV_A.global_lsp[k];
		}


		FGV_B.BAD_RATE = 0;

		//WB HR detector
		FGV_B.data_packet.WB_MODE_BIT = 0;	//default is NB decoding

		short rate_bck;

		rate_bck = FGV_B.rate;


		if (((FGV_B.buf16[10] & 0x0020) == 0x20) && (FGV_B.rate == 4))
		    //implies the 171st bit is set => EVRC-WB mode
		{
		    if ((FGV_B.buf16[10] & 0x0040) == 0x40) {
			//implies the 170th bit is set => MDCT mode
			FGV_B.data_packet.Celp_Mdct_Flag = 1;	//MDCT
			if ((FGV_B.buf16[10] & 0x0080) == 0x80) {
			    //implies 169 th bit is set => WB-MDCT mode
			    FGV_B.data_packet.WB_MODE_BIT = 1;

			    FGV_B.MDCT_NPULSES = 23;	/* Used for WB Coding */
			    FGV_B.MDCT_NWORDS = 8;	/* 23 pulses on 144 position = 114 bits => 7*15 bits + 9 bits */
			    FGV_B.MDCT_REMAINDER = 9;
			}
			else {
			    //implies 169 th bit is not set => NB-MDCT mode
			    FGV_B.data_packet.WB_MODE_BIT = 0;

			    // use this flag to distinguish between 23/28 pulse MDCT
			    FGV_B.MDCT_NPULSES = 28;	/* Used for NB Coding */
			    FGV_B.MDCT_NWORDS = 9;	/* 28 pulses on 144 position = 131 bits => 8*15 bits + 11 bits */
			    FGV_B.MDCT_REMAINDER = 11;

			}
		    }
		    else {
			//implies the 170th bit is not set => CELP mode
			FGV_B.data_packet.Celp_Mdct_Flag = 0;	//CELP
			FGV_B.data_packet.WB_MODE_BIT = 1;

		    }
		}
		else {
		    //implies the 171st bit is not set => EVRC-B mode
		    FGV_B.data_packet.WB_MODE_BIT = 0;
		    FGV_B.data_packet.Celp_Mdct_Flag = 0;	//EVRC-B does not support MDCT mode
		}




		//printf("%d %d\n",FGV_B.data_packet.Celp_Mdct_Flag,FGV_B.decode_fcnt);







		if ((FGV_B.rate == 2) && (FGV_B.WB_COUNT > 10)
		    && (FGV_B.rate_last != 2)) {

		    FGV_B.rate = 0xE;
		}

		//the idea here is that if we are in the WB decoding mode then rate=2 is not possible and hence bad-rate
		//however since rate=2 is possible when a switch to NB mode happens , we erase only the 1st rate =2 frame
       /********************* FIGURE OUT IF INCOMING PACKET IS WB/NB AND SET THE WB_MODE_BIT ************/
		if ((((FGV_B.buf16[0] & 0xFC00) >> 10) == 0x3F)
		    && (FGV_B.rate == 3)) {
		    FGV_B.rate = 2;
		    FGV_B.data_packet.WB_MODE_BIT = 1;
		}		//WB_HR

		if (((FGV_B.buf16[0] & 0x1) == 1) && (FGV_B.rate == 1))
		    FGV_B.data_packet.WB_MODE_BIT = 1;

		/* WB_MODE_BIT HANDLING FOR ERASURES */
		//if ((FGV_B.WB_COUNT>=5) && (FGV_B.rate==0xE)) FGV_B.data_packet.WB_MODE_BIT=1;
		// changed this to the line below

		if ((FGV_B.last_wb_mode_bit == 1) && (FGV_B.rate == 0xE))
		    FGV_B.data_packet.WB_MODE_BIT = 1;	// this would perform no worse if not better than the line above         
	/***********************************************************************************************/


		if (FGV_B.rate == 0)
		    FGV_B.data_packet.WB_MODE_BIT = FGV_B.last_wb_mode_bit;	//Added by Jagadeesh 08/21/06

		//if D&B signalling created a HCELP frame then set WB_MODE_BIT to 1 and DO NOT erase

		if ((FGV_B.rate == 3) && (FGV_B.last_wb_mode_bit != 0))
		    FGV_B.data_packet.WB_MODE_BIT = 1;


		if (FGV_B.data_packet.WB_MODE_BIT == 1)
		    Eargs->operating_point = 3;
		else
		    Eargs->operating_point = 0;


		if ((FGV_B.data_packet.WB_MODE_BIT == 0)
		    && (FGV_B.WB_COUNT > 10) && (FGV_B.last_wb_mode_bit != 0)
		    && (FGV_B.rate != 3)) {

		    FGV_B.rate = 0xE;
		}


		if ((FGV_B.data_packet.WB_MODE_BIT == 1)
		    && (FGV_B.NB_COUNT > 10)
		    && (FGV_B.last_wb_mode_bit == 0)) {
		    FGV_B.rate = 0xE;
		}



		if ((FGV_B.last_wb_mode_bit == 0) && (FGV_B.rate == 3))
		    FGV_B.initD = 0;	// ensure that for the current HCELP the upper band erasure proc. starts off with
		//initialized parameters IF the previous frame was NB


		FGV_B.rate_last = rate_bck;
		FGV_B.last_wb_mode_bit = FGV_B.data_packet.WB_MODE_BIT;


		//printf("%d\n",FGV_B.rate);



		//Time-Warping: Added to support time-warping option
		if (Eargs->time_warping == 0)
		    time_warp_fraction = 1;
		else if (Eargs->time_warping == -1)
		    time_warp_fraction = 0.5;
		else if (Eargs->time_warping == 1)
		    time_warp_fraction = 1.5;
		//End: Time-Warping: Added to support time-warping option

		if (FGV_B.data_packet.WB_MODE_BIT == 0) {	//narrowband
		    if (Eargs->phase_matching == 0)
			FGV_B.phase_offset = 10;
		    if (Eargs->double_erasures_pm == 0) {
			if (FGV_B.phase_offset == -1) {
			    FGV_B.phase_offset = 10;
			    time_warp_fraction = 1;
			}
		    }
		    if (Eargs->time_warping == 0)
			time_warp_fraction = 1;
		}



		FGV_B.data_packet.PACKET_RATE = FGV_B.rate;
		//Time-Warping: Slight Change to WB_decoder call below
		FGV_B.WB_decoder (time_warp_fraction);	//does WB decoding or NB decoding depending on the flag setting 
		//printf ("\n main obuf_len = %d %d", obuf_len, j);

		if ((!FGV_B.BAD_RATE)
		    && (FGV_B.data_packet.PACKET_RATE != 14)) {

		    if (FGV_B.data_packet.WB_MODE_BIT == 1) {
			FGV_B.WB_COUNT++;
			FGV_B.NB_COUNT = 0;
		    }
		    else {
			FGV_B.NB_COUNT++;
			FGV_B.WB_COUNT = 0;
		    }
		}



		if (!Eargs->lowband_decout)
		    fwrite (FGV_B.buf16, sizeof (int16), 2 * obuf_len, ofileP);	//writing to o/p file
		else
		    fwrite (FGV_B.buf16, sizeof (int16), obuf_len, ofileP);	//writing to o/p file


		switch (FGV_B.rate) {
		case 1:
		    rate_cnt_dec[0]++;
		    break;
		case 2:
		    rate_cnt_dec[1]++;
		    break;
		case 3:
		    if (FGV_B.rcelp_half_rateD) {
			rate_cnt_dec[6]++;
			rate_cnt_dec[2]++;
		    }
		    else
			switch (FGV_A.PPP_MODE_E) {
			case 'Q':
			    rate_cnt_dec[9]++;
			    break;
			case 'H':
			    rate_cnt_dec[12]++;
			    rate_cnt_dec[2]++;
			    break;
			case 'F':
			    rate_cnt_dec[15]++;
			    break;
			}
		    break;
		case 4:
		    rate_cnt_dec[3]++;
		    break;
		default:	// Erasure
		    rate_cnt_dec[4]++;
		    break;
		}

	    }
	    j++;
	    fflush (stdout);
	    fflush (stderr);
	    fflush (ofileP);


	    /* update buf, buf_HB, buf */
	    for (k = 0; k < LOOKAHEAD_LEN; k++)
		FGV_A.buf[k] = FGV_A.buf[k + FSIZE];
	    for (k = 0; k < LOOKAHEAD_LEN; k++)
		FGV_A.buf_backup[k] = FGV_A.buf_backup[k + FSIZE];


	    /* copy back the de-clicked input for buf_HB update */
	    for (k = 0; k < FGV_A.hb_ana_delay; k++)
		FGV_A.buf_HB[k] = FGV_A.buf_HB[7 * ibuf_len / 8 + k];
	    /* copy back the un-de-clicked input for buf_HB update */
	    for (k = 0; k < LOOKAHEAD_LEN * 7 / 8; k++)
		FGV_A.buf_HB[k + FGV_A.hb_ana_delay] = FGV_A.lookahead_UB[k];

	    /* shifting the 14K buf , not modified by de-click */
	    for (k = 0; k < LOOKAHEAD_LEN * 7 / 4 + FGV_A.hb_ana_delay * 2;
		 k++)
		FGV_A.buf_WB14[k] = FGV_A.buf_WB14[7 * ibuf_len / 4 + k];
	    int dummycount;

	    dummycount = 0;

	}
	j--;
	if (strstr (argv[0], "_enc") != NULL) {
	    if (Eargs->verbose)
		fprintf (stderr, "\nDone encoding\n");
	}
	else {
	    if (Eargs->encode_only)
		fprintf (stderr, "\nDone encoding\n");
	    else
		fprintf (stderr, "\nDone encoding and decoding\n");

	    print_wsnr ();

	    fprintf (stderr, "\n");
	    fprintf (stderr, "Encoder:\n");
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 2, 2.0,
		     2.4, rate_cnt[1] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 9, 2.0,
		     2.4, rate_cnt[9] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 3, 4.0,
		     4.8, rate_cnt[2] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 6, 4.0,
		     4.8, rate_cnt[6] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 12, 4.0,
		     4.8, rate_cnt[12] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 15, 8.5,
		     9.6, rate_cnt[15] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 4, 8.5,
		     9.6, rate_cnt[3] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2s:\t %.3f\t\t%6.3f\t\t%5.3f\n", "PC",
		     8.5, 9.6, rate_cnt[8] / (float) (j - rate_cnt[0]));
	    fprintf (stderr, "Mode %2s:\t %.3f\t\t%6.3f\t\t%5.3f\n", "PB",
		     8.5, 9.6, rate_cnt[16] / (float) (j - rate_cnt[0]));
	    if (!Eargs->encode_only) {
		fprintf (stderr, "\n");
		fprintf (stderr, "Decoder:\n");
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 2,
			 2.0, 2.4,
			 rate_cnt_dec[1] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 9,
			 2.0, 2.4,
			 rate_cnt_dec[9] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 3,
			 4.0, 4.8,
			 rate_cnt_dec[2] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 6,
			 4.0, 4.8,
			 rate_cnt_dec[6] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 12,
			 4.0, 4.8,
			 rate_cnt_dec[12] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 15,
			 8.5, 9.6,
			 rate_cnt_dec[15] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 4,
			 8.5, 9.6,
			 rate_cnt_dec[3] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "Erasure:\t\t\t\t\t%5.3f\n",
			 rate_cnt_dec[4] / (float) (j - rate_cnt_dec[0]));
		fprintf (stderr, "\n");
	    }
	    fprintf (stderr, "A Total of %ld frames, Voice Activity=%.3f%%\n",
		     j, 100 - rate_cnt[0] * 100.0 / j);

	    fprintf (stderr, "Overall Average Channel Rate=%.3f\n",
		     chan_rate / j);
	    fprintf (stderr, "Average Vocoder Rate for Active Speech=%.3f\n",
		     (voc_rate - rate_cnt[0] * 0.8) / (j - rate_cnt[0]));
	    fprintf (stderr, "Average Channel Rate for Active Speech=%.3f\n",
		     (chan_rate - rate_cnt[0] * 1.2) / (j - rate_cnt[0]));


	}
    }
    else {
	//Time-Warping: Added to support time-warping option
	if (Eargs->time_warping == 0)
	    time_warp_fraction = 1;
	else if (Eargs->time_warping == -1)
	    time_warp_fraction = 0.5;
	else if (Eargs->time_warping == 1)
	    time_warp_fraction = 1.5;

	if (Eargs->phase_matching == 0)
	    FGV_B.phase_offset = 10;
	if (Eargs->double_erasures_pm == 0) {
	    if (FGV_B.phase_offset == -1) {
		FGV_B.phase_offset = 10;
		time_warp_fraction = 1;
	    }
	}


	while ((fread (&FGV_B.rate, sizeof (int16), 1, ifileP)) == 1
	       && (Eargs->num_frames >= j)) {


	    if ((fread (FGV_B.buf16, sizeof (int16), ibuf_len, ifileP)) ==
		ibuf_len) {

		//Time-Warping: Slight Change to WB_decoder call below
		FGV_B.WB_decoder (time_warp_fraction);	// does WB decoding or NB decoding depending on the flag setting 


		if (Eargs->Fsop == 16000
		    || FGV_B.data_packet.WB_MODE_BIT == 1) {

		    if (!Eargs->lowband_decout)
			fwrite (FGV_B.buf16, sizeof (int16), 2 * obuf_len, ofileP);	//writing to o/p file
		    else
			fwrite (FGV_B.buf16, sizeof (int16), obuf_len, ofileP);	//writing to o/p file
		}
		else {
		    fwrite (FGV_B.buf16, sizeof (int16), obuf_len, ofileP);	//writing to o/p file

		}

		if (Eargs->verbose)
		    fprintf (stderr, "Decoding %ld of %ld\r", j, buf_count);

		j++;
		switch (FGV_B.rate) {
		case 1:
		    rate_cnt_dec[0]++;
		    break;
		case 2:
		    rate_cnt_dec[1]++;
		    break;
		case 3:
		    rate_cnt_dec[2]++;
		    break;
		case 4:
		    rate_cnt_dec[3]++;
		    break;
		default:	// Erasure
		    rate_cnt_dec[4]++;
		    break;
		}
		fflush (stdout);
		fflush (stderr);
		fflush (ofileP);
	    }
	}
	if (Eargs->verbose) {
	    fprintf (stderr, "Done decoding\n");
	}
	if (strstr (argv[0], "_dec") == NULL) {
	    fprintf (stderr, "\n");
	    fprintf (stderr, "Decoder:\n");
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 2, 1.8,
		     2.4, rate_cnt_dec[1] / (float) (j - rate_cnt_dec[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 3, 3.9,
		     4.8, rate_cnt_dec[2] / (float) (j - rate_cnt_dec[0]));
	    fprintf (stderr, "Mode %2d:\t %.3f\t\t%6.3f\t\t%5.3f\n", 4, 8.5,
		     9.6, rate_cnt_dec[3] / (float) (j - rate_cnt_dec[0]));
	    fprintf (stderr, "Erasure:\t\t\t\t\t%5.3f\n",
		     rate_cnt_dec[4] / (float) (j - rate_cnt_dec[0]));
	    fprintf (stderr, "\n");
	}
    }
    fprintf (stderr, "\n");
}
