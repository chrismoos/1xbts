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
#include <stdio.h>
#include <iostream>
#include <stdexcept>

#include "voip.h"		//VoIP configurations
#include "jitter_buffer.h"

#include "struct.h"		//For 4GV only
#define MAX_ENCODED_ELEMENTS	100

extern VOIP_Options *voip_options;

/* Variables for 4GV codec */
FGV_MEM FGV_A;
EvrcArgs *Eargs;
EvrcArgs VocoderParameters;

// These params added since they are globals defined in main.cc
int16 ibuf_len;
int16 obuf_len;
int write_accshift = 0;

//End: These params added since they are globals defined in main.cc

extern int num_pkts_since_warped;
extern int num_pkts_since_expanded;
extern int NUM_SAMPLES_PER_FRAME;

struct encoded_data_element
{
    int rate;
    int seq_num;
    char data[22];
    int length;
    float arrival_time;
};

struct encoded_data_element encoded_data_array[MAX_ENCODED_ELEMENTS];

static bool g_initDone;
int num_encoded_elements;

double Current_Time;

void
FGVCodec_Init ()
{

    // Initializing Vocoder configuration
    Eargs = &VocoderParameters;
    EvrcArgs *eargs = &VocoderParameters;

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

    /* This is different with EvrcB main.cc */
    eargs->dtx = 1;

    eargs->lowband_decout = 0;


    eargs->phase_matching = 0;
    eargs->double_erasures_pm = 0;
    eargs->time_warping = 0;

    eargs->encode_only = 0;
    eargs->decode_only = 1;

    FGV_A.InitEncoder ();
    FGV_A.InitDecoder ();
}

int
codecNoMorePackets ()
{
    if (num_encoded_elements == 0)
	return 1;
    else
	return 0;
}

void
codecCreateInstance ()
{
    g_initDone = 0;
    num_encoded_elements = 0;
    Current_Time = 0;

    FGVCodec_Init ();

    Init_Jitter_Buffer ();


    return;
}

void
codecReleaseInstance ()
{

}

int
codecInputPacket (const void *packet, unsigned int length,
		  unsigned long received)
{
    int rate;

    /* Parse RTP header for sequence number and transmitted timestamp. */
    const unsigned char *packetBytes = (const unsigned char *) packet;
    const int seq = ((int) packetBytes[2]) << 8 | ((int) packetBytes[3]);
    const unsigned long sent = ((unsigned long) packetBytes[4]) << 24
	| ((unsigned long) packetBytes[5]) << 16
	| ((unsigned long) packetBytes[6]) << 8
	| ((unsigned long) packetBytes[7]);


    /* Determine rate based on payload size. */
    /* 
     * NOTE: The quarter-rate (rate=2) vocoder frame size (5 bytes)
     * fills six bytes on little-endian platforms.  Thus, RTP header
     * (12 bytes) plus vocoder frame is 18 bytes.
     */
    switch (length) {
    case 34:
	rate = 4;
	break;
    case 22:
	rate = 3;
	break;
    case 18:
	rate = 2;
	break;
    case 14:
	rate = 1;
	break;
    default:
	return 0;		/* bad packet */
    }

    /* Initialize the dejitter buffer etc if it is not yet initialized. */
    if (!g_initDone) {
	g_initDone = 1;
	Current_Time = received;
    }

    /* Cache the packet for use later when the packet is returned out of the
     ** dejitter buffer.
     */
    encoded_data_array[num_encoded_elements].rate = rate;
    encoded_data_array[num_encoded_elements].seq_num = seq;
    encoded_data_array[num_encoded_elements].length = length - 12;
    encoded_data_array[num_encoded_elements].arrival_time = received;

    memcpy (encoded_data_array[num_encoded_elements].data, packetBytes + 12,
	    length - 12);


    num_encoded_elements++;

    /* Add packet seq_num to dejitter buffer. */
    Add_To_Jitter_Buffer (seq, sent, (received * 1.0) / 1000, rate);


    return 1;
}

int
codecOutputPCM (short *pcm)
{
    /* Loop until enough PCM is output. */
    float time_warp_fraction;
    int i, j;

    obuf_len = 160;
    int run_length, phase_offset = 10;
    float arrival_time;
    int opcm_len;

    /* Remove packet from the dejitter buffer. */
    /* [DJS]
     * NOTE:  Here, <rate> is distinct from <frameRate> and has the
     *        following meaning:
     *             14:  Erasure during non-silence
     *              4:  Regular packet (silence, NELP, CELP)
     *              1:  Erasure during silence (DTX)
     */
    int seq = -1;
    int rate =
	Remove_Packet_From_Buffer ((Current_Time * 1.0) / 1000,
				   &time_warp_fraction, &seq, &run_length,
				   &phase_offset);

    if (voip_options->wb && phase_offset != 10) {
	printf ("phase_offset = 10 && seq=%d\n", seq);
	exit (-1);
    }
    FGV_A.phase_offset = phase_offset;
    FGV_A.run_length = run_length;

    /* Decode the packet to PCM. */
    short frameRate;
    static int DecodingStarted = 0;

    if ((rate == 1) || (rate == 14)) {

	frameRate = 0;

	if (!DecodingStarted) {
	    FGV_A.rate = 0;
	    for (j = 0; j < NUM_SAMPLES_PER_FRAME; j++)
		FGV_A.buf16[j] = 0;
	    obuf_len = 160;
	}


	if (DecodingStarted) {
	    FGV_A.rate = frameRate;
	    FGV_A.WB_decoder (time_warp_fraction);
	}
    }
    else {
	DecodingStarted = 1;

	int found = -1;

	for (j = 0; j < num_encoded_elements; j++) {
	    if (seq == encoded_data_array[j].seq_num) {
		found = j;
		break;
	    }
	}

	frameRate = encoded_data_array[found].rate;
	arrival_time = encoded_data_array[found].arrival_time;

	//printf ("\n Encoded Found %d %d %d", found, seq, frameRate);

	memcpy (FGV_A.buf16, encoded_data_array[found].data,
		encoded_data_array[found].length);

	if (found >= 0) {
	    for (j = found; j < num_encoded_elements - 1; j++) {
		encoded_data_array[j].rate = encoded_data_array[j + 1].rate;
		encoded_data_array[j].seq_num =
		    encoded_data_array[j + 1].seq_num;
		encoded_data_array[j].arrival_time =
		    encoded_data_array[j + 1].arrival_time;

		for (i = 0; i < encoded_data_array[j + 1].length; i++)
		    encoded_data_array[j].data[i] =
			encoded_data_array[j + 1].data[i];

		encoded_data_array[j].length =
		    encoded_data_array[j + 1].length;
	    }
	    num_encoded_elements--;
	}

	//Remove older packets from Jitter Buffer
	while (1) {
	    found = -1;
	    for (j = 0; j < num_encoded_elements; j++) {
		if (seq > encoded_data_array[j].seq_num) {
		    found = j;
		    break;
		}
	    }

	    if (found >= 0) {
		for (j = found; j < num_encoded_elements - 1; j++) {
		    encoded_data_array[j].rate =
			encoded_data_array[j + 1].rate;
		    encoded_data_array[j].seq_num =
			encoded_data_array[j + 1].seq_num;
		    encoded_data_array[j].arrival_time =
			encoded_data_array[j + 1].arrival_time;

		    for (i = 0; i < encoded_data_array[j + 1].length; i++)
			encoded_data_array[j].data[i] =
			    encoded_data_array[j + 1].data[i];

		    encoded_data_array[j].length =
			encoded_data_array[j + 1].length;
		}
		num_encoded_elements--;
	    }
	    else
		break;
	}
	FGV_A.rate = frameRate;
	FGV_A.WB_decoder (time_warp_fraction);	//does WB decoding or NB decoding depending on the flag setting 

    }

    if (voip_options->wb || FGV_A.data_packet.WB_MODE_BIT == 1) {
	if (!Eargs->lowband_decout)
	    opcm_len = 2 * obuf_len;
	else
	    opcm_len = obuf_len;
    }
    else {
	opcm_len = obuf_len;
    }

    memcpy (pcm, FGV_A.buf16, opcm_len * 2);

    if (obuf_len == 160)
	num_pkts_since_warped++;
    else
	num_pkts_since_warped = 0;

    if (obuf_len > 160)
	num_pkts_since_expanded = 0;
    else
	num_pkts_since_expanded++;


    /* Increment time: the following line works for both WB and NB */
    Current_Time += (20 * (obuf_len) * 1.0) / 160;

    //printf ("%u %f PCM samples output\n", opcm_len, Current_Time);

    return (opcm_len);
}
