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
FILE:       WBSmartBlanking.h

SERVICES:   

GENERAL DESCRIPTION:  Implementation of the Smart Blanking Algorithm (Headers)
                      For 4GV WB

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
// WBSmartBlanking.h: interface for the WBSmartBlanking class.
//
//////////////////////////////////////////////////////////////////////


#ifndef WB_SMART_BLANKING_H_
#define WB_SMART_BLANKING_H_

/*======================================================================*/
/*         ..Smart 1/8 Rate Constants                         */
/*----------------------------------------------------------------------*/
#define MS_LSP_CODEBOOK_SIZE                   32	// 5 bits
#define LS_LSP_CODEBOOK_SIZE                   32	// 5 bits


/*======================================================================*/
/*         ..evrc-b quantization levels for 1/8 rate energy        */
/*           they should match the one for silence.c
/*----------------------------------------------------------------------*/
#define NUM_Q_LEVELS        32

/*======================================================================*/
/*  Rate definition bellow MUST match with the one used by vocoder      */
/*            This values are used by ECVRC and 4GV c-SIMs              */
/*----------------------------------------------------------------------*/
#define FULL_RATE           4
#define HALF_RATE           3
#define QUARTER_RATE        2
#define EIGHTH_RATE         1
#define BLANK_RATE          0
#define ERASED_FRAME        0xe

class WBSBEncoder
{
  public:
    WBSBEncoder ();
    void SetVocoderType (int Vocoder);
    virtual unsigned int Encode (short *Rate, short *IBuffer, short refl_flag,
				 short nsid);
    short RateToLength (short Rate);
    short LengthToRate (short Length);

  private:
    void UpdateFilteredPower (unsigned char Power);
    int GetDeltaFromAverage (unsigned char Power);

    void GetQParameters (unsigned short Buffer, unsigned short &Power,
			 unsigned short &MSLSPs, unsigned short &LSLSPs);

    int FirstTime;
    unsigned int InSilence;
    int Vocoder;
    unsigned short EigthRatePrototype;
    unsigned short PrototypeEigthRatePower;
    unsigned short CandidateEigthRatePower;
    unsigned int MSLSPsHist[MS_LSP_CODEBOOK_SIZE];	// 32 Codebooks
    unsigned int LSLSPsHist[LS_LSP_CODEBOOK_SIZE];	// 32 Codebooks
    unsigned char FGVWBEigthRatePower[NUM_Q_LEVELS];	// 32 levels

    unsigned int FilteredPower;
    unsigned int PowerThreshold;
    unsigned int NFramesSinceLastSent;
};

class WBSBDecoder
{
  public:
    WBSBDecoder ();
    void SetVocoderType (int Vocoder);
    virtual unsigned int Decode (short *Rate, short *IBuffer);
    short RateToLength (short Rate);
    short LengthToRate (short Length);

  private:
    int FirstTime;
    unsigned int InSilence;
    int Vocoder;
    int ErasureRun;
    unsigned short EigthRatePrototype;

    short PrototypePlaybackProbability;
};
#endif // !defined(WB_SMART_BLANKING_H_)
