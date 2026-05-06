/**********************************************************************
Each of the companies; Motorola, and Lucent, and Qualcomm, and Nokia (hereinafter 
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
all Normative portions of the EVRC-B Variable Rate Speech Codec as 
it exists on the date of submittal of this form, should the EVRC-B be 
approved as a Specification or Report by 3GPP2, or as a transposed 
Standard by any of the 3GPP2's Organizational Partners, the Source(s) 
state(s) that a worldwide license to reproduce, use and distribute the 
Software, the license rights to which are held by the Source(s), will 
be made available to applicants under terms and conditions that are 
reasonable and non-discriminatory, which may include monetary compensation, 
and only to the extent necessary for the practice of any or all of the 
Normative portions of the EVRC-B or the field of use of practice of the 
EVRC-B Specification, Report, or Standard.  The statement contained above 
is irrevocable and shall be binding upon the Source(s).  In the event 
the rights of the Source(s) in and to copyright or copyright license 
rights subject to such commitment are assigned or transferred, the 
Source(s) shall notify the assignee or transferee of the existence of 
such commitments.
*******************************************************************/
/******************************************************************************
 ** voip_test
 ** Copyright (c) 2005 by QUALCOMM, Inc.  All Rights Reserved
 **
 **     This program is an aid for evaluating the quality of a speech codec in
 **     a simulated VoIP (Voice over Internet Protocol) call.  It runs a
 **     sequence of RTP packets through a speech decoder, collecting the output
 **     PCM from the decoder.
 **
 **     The input to the program is a sequence of RTP packets, with an
 **     additional header field marking each packet with its "actual received
 **     time" or when the decoder would have actually received the packet.
 **     The actual received time is removed before sending the RTP packet to
 **     the decoder.  The packets are sent in the order of actual received
 **     time.
 **
 **     The output of the program is a sequence of PCM samples, retrieved from
 **     the decoder.  The decoder is queried for PCM samples between every
 **     input RTP packet.  The number of PCM samples queried in each call is
 **     equal to the difference between the actual received times of the two
 **     adjacent RTP packets in time.
 **
 **     It is permissible for the decoder module to include tools such as an
 **     adaptive dejitter buffer, to cope with the extra burdens associated
 **     with typical VoIP environments.
 **
 ** MODULE
 **     main
 **
 ** DESCRIPTIONS
 **     This module contains the main function, the entry point of the
 **     program.  It opens the input and output files, reads input RTP packets
 **     with actual received times, and alternates between calls to input RTP
 **     packets into the speech codec and querying PCM output samples from the
 **     speech codec.  The program continues until the input file is exhausted.
 *****************************************************************************/

/* This header defines the interface between the tester and the codec. */
#include "codec.h"

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>

#include "voip.h"

#define CRD_STDOUT 0

/* Global variable for VoIP */
VOIP_Options *voip_options;

/* These control how much memory is initially allocated for the program's
** input and output buffers.
*/
static const size_t PREALLOCATE_INPUT_SIZE = 1000;
static const size_t PREALLOCATE_OUTPUT_SIZE = 1000;

/* This structure contains the payload, payload size, and received timestamp
** of each input RTP packet.
*/
struct Packet
{
    unsigned long size;
    unsigned long timestamp;
    void *payload;
};

/* Initialize the environment variables */
void init_voip_options () 
{
    voip_options->do_iba = 1; //Implicit Buffer Adaptation
    voip_options->do_iplc = 0; //Interpolation-based PLC
    voip_options->do_timewarping = 1; //Time Warping
    voip_options->target_pkt_underflow = -1.0; //target packet underflow rate; Give it an invalid number
    voip_options->wb=1; //wideband decoding
    voip_options->ncim_input_pkt_file = NULL;
    voip_options->output_pcm_file = NULL;
}

static void usage (const char *programName)
/******************************************************************************
 ** This function prints the program's usage information to the standard error
 ** device.
 **
 ** IN const char *programName
 **     This is the name of the program, to be printed as part of the program
 **     usage information.
 *****************************************************************************/
{
    fprintf (stderr, "Usage:  %s [options] <ncim_packet_file> <pcm_file>\n", programName);
    fprintf (stderr, "\t-T <0 or 1> \tENABLE (1)/disable (0) timewarping, (default=1).\n");
    fprintf (stderr, "\t-A <0 or 1> \tENABLE (1)/disable (0) implicit buffer adaptation, (default=1).\n");
    fprintf (stderr, "\t-I <0 or 1> \tenable (1)/DISABLE (0) interpolation based packet loss concealment, (default=0).\n");
    fprintf (stderr, "\t-r <UnderflowRate> \t Target underflow rate due to delay, (default=0.005).\n");
    fprintf (stderr, "\t-W <1 or 0> \tWideband (1) or Narrowband(0) decoding (default:wideband)\n");
}

/* Get the arguments from command line */
char  valid_options[]="A:I:r:T:W:";
void get_args ( int argc, char* argv[])
{
    const char *programName = argv[0];
    register int i, j;
    int c;
 
    if (argc < 3) {
	usage(programName);
	exit(-1);
    }
    
    while ((c = getopt (argc, argv, valid_options)) != -1) {
	switch (c)
	{
	case 'A': //implicit buffer Adaptation
	    if (atoi(optarg)) voip_options->do_iba = 1;
	    else voip_options->do_iba = 0;
	    break;
	case 'I': //Interpolation-based packet loss concealment
	    if (atoi(optarg)) voip_options->do_iplc = 1;
	    else voip_options->do_iplc = 0;
            break;
	case 'r': //target underflow rate
	    sscanf(optarg, "%lf", &voip_options->target_pkt_underflow);
	    break;
	case 'T': //time-warping
	    if (atoi(optarg)) voip_options->do_timewarping = 1;
	    else voip_options->do_timewarping = 0;
	    break;
	case 'W':
	    voip_options->wb = atoi(optarg);
	    break;
	case '?':
	    fprintf (stderr, "Unknown option character `\\x%x'.\n", optopt);
	    usage(programName);
	    exit(-1);
	default:
	    exit(-1);
	}
    }
     
    if ( argc - optind != 2) {
	fprintf(stderr, "Please provide both input filename and output filename\n");
	usage(programName);
	exit(-1);
    }
    else {
	voip_options->ncim_input_pkt_file = argv[optind];
	voip_options->output_pcm_file = argv[optind+1];
    }

    return;
}

int main (int argc, char *argv[])
/******************************************************************************
 ** This is the main function of the program.  It parses the command line and
 ** then performs the codec test.
 **
 ** IN int argc
 **     This is the number of command line arguments, including the name of
 **     the program itself.
 **
 ** IN char *argv[]
 **     This is the array of command line arguments.
 **
 ** RETURNS int
 **     The value 0 is returned if the program exits normally.  Otherwise, the
 **     value -1 is returned, indicating a problem occurred.  A descriptive
 **     string for the problem is printed to the standard error device in this
 **     case.
 *****************************************************************************/
{
    FILE *pkt, *pcm;
    struct Packet *input = 0;
    size_t inputSize = 0;
    size_t inputUsed = 0;
    size_t i;
    int error = 0;
    short *output = 0;
    size_t outputSize = 0;
    unsigned long pktSize;

    /* allocate memeory for voip_options */
    if ((voip_options=(VOIP_Options*)malloc(sizeof(VOIP_Options))) == NULL) {
	fprintf(stderr, "ERROR-unable to malloc memory to voip_options");
	exit(-1);
    }    

    /* Initialize the voip options */
    init_voip_options();
    /* Get the arguments for command line */
    get_args(argc, argv);

    /* Open input RTP packet file and output PCM file. */
    pkt = fopen (voip_options->ncim_input_pkt_file, "rb");
    if (!pkt)
    {
	fprintf (stderr, "ERROR: unable to open packet file\n");
	return -1;
    }
    pcm = fopen (voip_options->output_pcm_file, "wb");
    if (!pkt)
    {
	fprintf (stderr, "ERROR: unable to open PCM file\n");
	fclose (pkt);
	return -1;
    }

    /* Initialize the parameters of codec */    
    codecCreateInstance ();

    /* Preallocate a reasonable number of input packets, to reduce the
    ** number of memory reallocations that would be done for the packet array
    ** otherwise, if it was not preallocated.
    */
    inputSize = PREALLOCATE_INPUT_SIZE;
    input = (struct Packet *) malloc (sizeof (struct Packet) * inputSize);
    if (!input)
    {
	fprintf (stderr, "ERROR: unable to allocate memory for "
		 "input packet list!\n");
	error = 1;
    }

    /* Preallocate a reasonable sized buffer for PCM output, to reduce the
    ** number of memory reallocations that would be done for the buffer
    ** otherwise, if it was not preallocated.
    */
    if (!error)
    {
	outputSize = PREALLOCATE_OUTPUT_SIZE;
	output = (short *) malloc (sizeof (short) * PREALLOCATE_OUTPUT_SIZE);
	if (!output)
        {
	    fprintf (stderr, "ERROR: unable to allocate memory for "
		     "output PCM buffer!\n");
	    error = 1;
        }
    }

    /* Read each packet, one at a time, into the packet array, expanding it as
    ** necessary.
    */
    while (!error && fread (&pktSize, 4, 1, pkt))
    {
	/* Make sure packet buffer is large enough. */
	while (inputUsed >= inputSize)
        {
	    struct Packet *newInput = (struct Packet *) realloc
		(input, sizeof (struct Packet) * inputSize * 2);
	    if (!newInput)
            {
		fprintf (stderr, "ERROR: unable to reallocate memory "
			 "for input packet list!\n");
		error = 1;
		break;
            }
	    input = newInput;
	    inputSize *= 2;
        }

#if CRD_STDOUT
	printf ("\n pktSize %d", pktSize);
#endif

	/* Save packet payload size. */
	input[inputUsed].size = pktSize;

	/* Read packet "received" time. */
	if (!fread (&input[inputUsed].timestamp, 4, 1, pkt))
        {
	    fprintf (stderr, "ERROR: last packet truncated!\n");
	    error = 1;
	    break;
        }

	/* Allocate memory for packet payload. */
	input[inputUsed].payload = malloc (input[inputUsed].size);
	if (!input[inputUsed].payload)
        {
	    fprintf (stderr, "ERROR: unable to allocate memory for "
		     "packet %d payload!\n", (inputUsed + 1));
	    error = 1;
	    break;
        }
	++inputUsed;
#if CRD_STDOUT
	printf ("\n Num Packets Read %d", inputUsed);
#endif
	/* Read in packet payload. */
	if (!fread (input[inputUsed - 1].payload, input[inputUsed - 1].size,
		    1, pkt))
        {
	    fprintf (stderr, "ERROR: last packet truncated!\n");
	    error = 1;
	    break;
        }
    }

    /* Now that all input has been read into memory, the input file is no
    ** longer needed, so close it.
    */
    fclose (pkt);

    /* Sort packets by "received" timestamp. */
    if (!error)
    {
	//qsort (input, inputUsed, sizeof (struct Packet), packetCompare);
    }

    /* Perform simulation by feeding the codec each RTP packet, in the order
    ** in which they are marked "received", and querying for PCM output
    ** between each RTP packet to cover the time differences.
    */
    i = 0;
    double	currentTime = 0;
    double	nextPlayoutTime = input[0].timestamp;
    int		packetsUsed = 0;
    int		numSamples;

    while (1)
    {		
	while (input[i].timestamp <= nextPlayoutTime && packetsUsed == 0)
	{
	    /* Feed next RTP packet to codec. */
	    if (!codecInputPacket (input[i].payload, input[i].size,
				   input[i].timestamp))
	    {
		fprintf (stderr, "ERROR: codec unable to decode packet #%lu\n", i);
		error = 1;
		break;
	    }
	
	    currentTime = input[i].timestamp;
	    i++;

	    /* check if we've used up all the input packets */
	    if (i == inputUsed)
	    {
		packetsUsed = 1;
		break;
	    }
	}

	/* check if we're done */
	if (packetsUsed == 1 && codecNoMorePackets() == 1)
	{
#if CRD_STDOUT
	    printf ("\n Breaking \n");
#endif
	    break;
	}

	while ((packetsUsed == 0 && nextPlayoutTime < input[i].timestamp) || (packetsUsed == 1))
	{
	    if (packetsUsed == 1 && codecNoMorePackets() == 1)
	    {
#if CRD_STDOUT
		printf ("\n Breaking \n");
#endif
		break;
	    }

	    memset (output, 0, PREALLOCATE_OUTPUT_SIZE * sizeof (short));

	    /* Request next block of PCM from codec. */
	    if (!(numSamples = codecOutputPCM (output)))
            {
		fprintf (stderr, "ERROR: codec unable to output PCM!\n"); fflush(stdout);
		error = 1;
		break;
            }

	    /* Save output PCM to file. */
	    if (!fwrite (output, sizeof (short), numSamples, pcm))
            {
		fprintf (stderr, "ERROR: unable to write PCM output!\n");
		error = 1;
		break;
            }

	    currentTime = nextPlayoutTime;
	    if (voip_options->wb) { //Wideband speech
		nextPlayoutTime += (numSamples * 1.0) / 16;
	    }
	    else { //Narrowband speech
		nextPlayoutTime += (numSamples * 1.0) / 8;
	    }
			
#if CRD_STDOUT
	    printf ("\n Next Samples Returned %f %f %d\n", currentTime, nextPlayoutTime, numSamples);
#endif
	}
    }

    /* Deallocate output PCM buffer. */
    if (output)
    {
	free (output);
    }

    /* Deallocate input packet memory. */
    if (input)
    {
	for (i = 0; i < inputUsed; ++i)
        {
	    free (input[i].payload);
        }
	free (input);
    }

    /* Close output file and release codec and  codec library. */
    fclose (pcm);
    codecReleaseInstance ();
    return 0;
}
