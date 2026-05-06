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
/*=============================================================================
FILE:       WBSmartBlanking.cpp

SERVICES:  

GENERAL DESCRIPTION:  Implementation of the Smart Blanking Algorithm

INITIALIZATION AND SEQUENCING REQUIREMENTS:
none 

(c) COPYRIGHT 2005 QUALCOMM Incorporated.
All rights reserved.  QUALCOMM proprietary and confidential.

The party receiving this software directly from QUALCOMM (the "Recipient" )
may use this software and make copies thereof as reasonably necessary solely
for the purposes set forth in the agreement between the Recipient and
QUALCOMM (the "Agreement").  The software may be used in source code form
solely by the Recipient's employees.  The Recipient shall have no right to
sublicense, assign, transfer or otherwise provide the source code to any
third party. Subject to the terms and conditions set forth in the Agreement,
this software, in binary form only, may be distributed by the Recipient to
its customers. QUALCOMM retains all ownership rights in and to the software.

This notice shall supercede any other notices contained within the software.

=============================================================================*/
/*======================================================================*/
/*         Include files                         */
/*----------------------------------------------------------------------*/
#include <memory.h>
#include <stdlib.h>
#include <netinet/in.h>
#include "WBSmartBlanking.h"

/*======================================================================*/
/*         ..Smart 1/8 Rate Constants                         */
/*----------------------------------------------------------------------*/

#define FILTER_CONSTANT                 10	// (10/100) = 0.1

#define N_TRANSITORY_FRAMES             2
#define N_ERASURE_TIME_OUT              5

#define DTX_MAX                         32	// in frames (20ms per frame)
#define DTX_MIN					        12	// in frames
#define DTX_HANGOVER                    1	// in frames

/*======================================================================*/
/*         ..evrc-b quantization parameters for 1/8 rate energy        */
/*           they should match silence.c 		                */
/*----------------------------------------------------------------------*/

#define MAX_LOG_ENERGY_WB	(2.0*6.0)	// they should reside in defines.h
#define MIN_LOG_ENERGY_WB	(2.0*1.0)	// parameters are not there jet
#define LOG_ZERO_ENERGY_WB	(2.0*-1.0)	// -2 to 12... huge range

#define LOG_ENERGY_RANGE_WB (MAX_LOG_ENERGY_WB - LOG_ZERO_ENERGY_WB)
#define FGVWB_POWER_1DB                 256.0/(LOG_ENERGY_RANGE_WB * 10)	// Force fractions here
#define FGVWB_POWER_THRESHOLD           int(FGVWB_POWER_1DB * 3)	//  3db (Remove fractions)

/*======================================================================*/
/*         Smart 1/8 Rate blanking helper functions                     */
/*----------------------------------------------------------------------*/
void
BuildPrototype (unsigned short &Buffer, unsigned short Power,
		unsigned short MSLSPs, unsigned short LSLSPs)
{
    Power <<= 1;
    LSLSPs <<= 6;		// Power was 5 bits long
    MSLSPs <<= 11;		// First section of LSP is 5 bits long

    Buffer = 0x0001 | Power | LSLSPs | MSLSPs;	//bug-fix
}

void
UpdateMemory (unsigned char LSPs, unsigned int *Histogram, int Size)
{
    int i;

    for (i = 0; i < Size; i++) {
	if (i == LSPs) {
	    Histogram[i] =
		(Histogram[i] * (100 - FILTER_CONSTANT) +
		 100 * FILTER_CONSTANT) / 100;
	}
	else {
	    Histogram[i] = (Histogram[i] * (100 - FILTER_CONSTANT)) / 100;
	}
    }
}

unsigned char
GetMostPopular (unsigned int *Histogram, int Size)
{
    int i;
    unsigned char Index = 0;


    for (i = 0; i < Size; i++) {
	if (Histogram[i] > Histogram[Index]) {
	    Index = (unsigned char) i;
	}
    }
    return Index;
}

/*======================================================================*/
/*         Smart blanking contructor and destructors                    */
/*----------------------------------------------------------------------*/
WBSBEncoder::WBSBEncoder ()
{
    FirstTime = 1;
    InSilence = 0;

    // little ugly but very efficient...  Initializing memory
    memset (MSLSPsHist, 0, (MS_LSP_CODEBOOK_SIZE * sizeof (int)));
    memset (LSLSPsHist, 0, (LS_LSP_CODEBOOK_SIZE * sizeof (int)));

    // Initializing Power array
    for (int idxcbg = 0; idxcbg < NUM_Q_LEVELS; idxcbg++) {
	if (idxcbg == 0) {
	    FGVWBEigthRatePower[idxcbg] = 0;
	}
	else {
	    // Calculate encoded energy
	    double Energy =
		MIN_LOG_ENERGY_WB + (float) idxcbg * (MAX_LOG_ENERGY_WB -
						      MIN_LOG_ENERGY_WB) /
		NUM_Q_LEVELS;
	    // Normalize to 1 and quantize to 8 bits.
	    FGVWBEigthRatePower[idxcbg] =
		int (((Energy -
		       LOG_ZERO_ENERGY_WB) / LOG_ENERGY_RANGE_WB) * 255);
	}
    }

    PowerThreshold = FGVWB_POWER_THRESHOLD;

    NFramesSinceLastSent = 0;
}


WBSBDecoder::WBSBDecoder ()
{
    FirstTime = 1;
    InSilence = 1;		// Decoder is considered to be in silence since it can take a while for it to rx good frames

    // Prototype should be initialized
    BuildPrototype (EigthRatePrototype, 0, 10, 10);	//Only energy is very important (0= minimum), arbitrary lsp's 

    PrototypePlaybackProbability = 100;	// This value is hardcoded but can be changed depending on "Taste" (MOS)
}

/*======================================================================*/
/*         Smart blanking member helper funtions                   */
/*----------------------------------------------------------------------*/

void
WBSBEncoder::UpdateFilteredPower (unsigned char Power)
{
    FilteredPower =
	(FilteredPower * 90 + FGVWBEigthRatePower[Power] * 10) / 100;
}

int
WBSBEncoder::GetDeltaFromAverage (unsigned char Power)
{
    int DeltaFromAverage = abs ((int) (FilteredPower - FGVWBEigthRatePower[Power]));

    return DeltaFromAverage;
}

/*======================================================================*/
/*         Smart blanking encode and decode functions                   */
/*----------------------------------------------------------------------*/
void
WBSBEncoder::GetQParameters (unsigned short Buffer, unsigned short &Power,
			     unsigned short &MSLSPs, unsigned short &LSLSPs)
{
    Buffer >>= 1;		// Get positioned for extraction of energy
    Power = Buffer & 0x001f;	// 5 bit for the power
    Buffer >>= 5;
    LSLSPs = Buffer & 0x001f;	// 5 bits for LSP
    Buffer >>= 5;
    MSLSPs = Buffer & 0x001f;	// 5 bits for Second portion of LSPs
}

unsigned int
WBSBEncoder::Encode (short *Rate, short *IBuffer, short refl_flag, short nsid)
{

    if (*Rate > EIGHTH_RATE)	// It is assumed that encoder does not generate erasures
    {				// or blank frames
	InSilence = 0;
    }
    else {
	InSilence++;		// warp arround after 21+ minutes (16 bits), good enough
	if (FirstTime) {
	    EigthRatePrototype = *IBuffer;	//ntohs

	    unsigned short Power;
	    unsigned short MSLSPs;
	    unsigned short LSLSPs;

	    GetQParameters (EigthRatePrototype, Power, MSLSPs, LSLSPs);

	    UpdateMemory ((unsigned char) MSLSPs, MSLSPsHist,
			  MS_LSP_CODEBOOK_SIZE);
	    UpdateMemory ((unsigned char) LSLSPs, LSLSPsHist,
			  LS_LSP_CODEBOOK_SIZE);

	    FilteredPower = FGVWBEigthRatePower[Power];
	    CandidateEigthRatePower = Power;
	    PrototypeEigthRatePower = Power;

	    FirstTime = 0;
	}
    }

    // Blank if handover is done
    if (InSilence > DTX_HANGOVER)	// Handover is over, blank this frame
    {
	*Rate = BLANK_RATE;
	NFramesSinceLastSent++;
    }
    else if (InSilence <= 1)	// First 1/8 rate is send as is
    {
	NFramesSinceLastSent = 0;	// a frame will be transmitted
    }
    else			// Handover greater than 1. Prototypes are sent after first one
    {
	// "Send prototype"
	*IBuffer = EigthRatePrototype;	//htons
	NFramesSinceLastSent = 0;	// a frame will be transmitted
    }

    // Update statistics
    if (InSilence > N_TRANSITORY_FRAMES || (refl_flag == 1))	// number after which encoder is assumed stable
    {
	// Upper layer is assumed to be network endianess
	unsigned short Current8Frame = *IBuffer;	//ntohs

	// Update memory, filters
	unsigned short Power;
	unsigned short MSLSPs;
	unsigned short LSLSPs;

	GetQParameters (Current8Frame, Power, MSLSPs, LSLSPs);

	UpdateMemory ((unsigned char) MSLSPs, MSLSPsHist,
		      MS_LSP_CODEBOOK_SIZE);
	UpdateMemory ((unsigned char) LSLSPs, LSLSPsHist,
		      LS_LSP_CODEBOOK_SIZE);

	UpdateFilteredPower (Power);

	// Update candidate prototype power if required
	// This can be substituted by "real encoding" of the average.
	if (GetDeltaFromAverage (CandidateEigthRatePower) >
	    GetDeltaFromAverage (Power)) {
	    CandidateEigthRatePower = Power;
	}

	unsigned int PowerDelta =
	    GetDeltaFromAverage (PrototypeEigthRatePower);

	if (nsid == 0) {

	    // Send Prototype if background noise update is required
	    if ((PowerDelta > PowerThreshold)
		&& (NFramesSinceLastSent > DTX_MIN)
		&& (PrototypeEigthRatePower != CandidateEigthRatePower)
		|| (refl_flag == 1)) {
		// build new prototype 
		MSLSPs = GetMostPopular (MSLSPsHist, MS_LSP_CODEBOOK_SIZE);
		LSLSPs = GetMostPopular (LSLSPsHist, LS_LSP_CODEBOOK_SIZE);

		BuildPrototype (EigthRatePrototype, CandidateEigthRatePower,
				MSLSPs, LSLSPs);

		PrototypeEigthRatePower = CandidateEigthRatePower;

		// "Send prototype"
		*IBuffer = EigthRatePrototype;	//htons
		*Rate = EIGHTH_RATE;
		NFramesSinceLastSent = 0;
	    }

	}
	if (NFramesSinceLastSent >= DTX_MAX) {
	    // "Send prototype"
	    *IBuffer = EigthRatePrototype;	//htons
	    *Rate = EIGHTH_RATE;
	    NFramesSinceLastSent = 0;
	}
    }

    return InSilence;
}

unsigned int
WBSBDecoder::Decode (short *Rate, short *IBuffer)
{

    if (*Rate > EIGHTH_RATE)	// It is assumed that erasure is fed as blank frame
    {
	InSilence = 0;
	ErasureRun = 0;
    }
    else			// We have a 1/8 rate frame or an erasure
    {
	if (*Rate == EIGHTH_RATE)	// If we got a 1/8 rate frame.. we transition to silence
	{
	    if (FirstTime) {
		// Asume that first received 1/8 Rate is the prototype
		EigthRatePrototype = *IBuffer;	//ntohs
		FirstTime = 0;
	    }
	    InSilence++;
	}
	else			// we have an erasure or blank frame (same for the algorithm)   
	{
	    if (InSilence || (ErasureRun++ >= N_ERASURE_TIME_OUT)) {
		InSilence++;	// warp arround after 21+ minutes (16 bits), good enough 
	    }
	}
    }

    if (InSilence > 1)		// first 1/8 rate gets played as is. It is considered "speech"
    {
	if (*Rate == EIGHTH_RATE)	// a new prototype has arrived
	{
	    EigthRatePrototype = *IBuffer;	//ntohs          
	}
	else			// play stored prototype
	{
	    //if ((InSilence == 2) )//||        // Always play prototype after first received 1/8 rate frame to settle noise
	    //                      (rand() < (PrototypePlaybackProbability * RAND_MAX)/100)) // randomize after that
//            {
	    *IBuffer = EigthRatePrototype;	//htons
	    *Rate = EIGHTH_RATE;
	    //          }
	}
    }

    if (*Rate < EIGHTH_RATE)	// If SB desides to pass the frame "promote" any blanked frame to erasure
    {
	*Rate = ERASED_FRAME;
    }

    return InSilence;
}
