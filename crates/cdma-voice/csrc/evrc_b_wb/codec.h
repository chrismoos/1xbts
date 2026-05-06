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
 **     codec
 **
 ** DESCRIPTIONS
 **     This module defines the interface between the main tester program and
 **     the speech codec (plus supporting modules such as dejitter buffer) to
 **     be tested.  The interface consists of C functions exported by the
 **     speech codec module, which the tester calls to input RTP packets and
 **     query for PCM output.
 *****************************************************************************/

#ifdef __cplusplus
//extern "C" {
#endif /* __cplusplus */

void codecCreateInstance ();

/******************************************************************************
 ** This function creates an instance of a codec, with any supporting
 ** components such as dejitter buffer, that conforms to this interface.
 **
 ** IN const char *id
 **     This is a string which can be used to provide codec-specific
 **     identification or configuration.  It is specified by the user of the
 **     test application.
 **
 ** RETURNS void *
 **     The handle to a new codec instance is returned, if successful.
 **     Otherwise, zero is returned, indicating failure.
 *****************************************************************************/

void codecReleaseInstance ();

/******************************************************************************
 ** This function releases a codec instance created by codecCreateInstance.
 **
 ** IN void *codec
 **     This is the handle to the codec instance to release.  It is the handle
 **     returned by a previous call to codecCreateInstance.
 *****************************************************************************/

int codecInputPacket (const void *packet, unsigned int length,
		      unsigned long received);
/******************************************************************************
 ** This function passes the next RTP packet to the codec under test.  The RTP
 ** packet consists of all octets received by the underlying transport
 ** protocol (e.g. UDP packet) not including any overhead required by that
 ** protocol.  In other words, all RTP headers are included.
 **
 ** The "received" time indicated by this call also indicates the "current"
 ** simulated elapsed time of the media stream or session.  The codec will not
 ** be queried for PCM samples covering any period of media stream or session
 ** time for which the very next RTP packet input does not cover.
 **
 ** IN void *codec
 **     This is the handle to the codec under test.  It is returned by a
 **     previous call to codecCreateInstance.
 **
 ** IN const void *packet
 **     This points to a buffer containing the RTP packet to input.
 **
 ** IN unsigned int length
 **     This is the number of octets in the RTP packet.
 **
 ** IN unsigned long received
 **     This is the time, in units of the RTP clock for the stream, at which
 **     the RTP packet was "received" by the tester.  It must be equal to or
 **     greater than the times of all RTP packets passed via previous calls to
 **     this function, even if the RTP packet is out of sequence (which
 **     happens if an RTP packet is delayed, causing the RTP packet after it
 **     to be delivered first).
 **
 ** RETURNS int
 **     A non-zero value is returned if the function completes successfully.
 **     Otherwise, a zero is returned, indicating the codec has
 **     encountered a problem and the test cannot be continued.
 *****************************************************************************/

int codecOutputPCM (short *pcm);

/******************************************************************************
 ** This function queries the codec under test for more of its PCM output
 ** stream.  The amount of time corresponding to the requested number of PCM
 ** samples will not exceed the time of the next RTP packet input to the
 ** codec.  In other words, PCM will not be requested for any part of stream
 ** time ahead of the times "received" RTP packets are input.
 **
 ** OUT short *pcm
 **     This is a buffer in which to store the PCM output samples.  It must
 **     have enough space to store the requested number of PCM samples.
 **
 **
 ** RETURNS int
 **     Returns the number of PCM samples produced
 *****************************************************************************/

int codecNoMorePackets ();

/******************************************************************************
 **This function returns 1 if no more packets are in the jitter buffer of the
 VoIP Client. This is used to terminate the NCIM simulation.
*****************************************************************************/

#ifdef __cplusplus
//} // extern "C"
#endif /* __cplusplus */
