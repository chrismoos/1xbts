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
/*  Lucent Technologies Network Wireless Systems                        */
/*  EVRC Floating-point C Simulation.                                   */
/*                                                                      */
/*  Copyright (C) 1999 Lucent Technologies Incorporated. All rights     */
/*  reserved.                                                           */
/*======================================================================*/

/********************************************************************
* tty_hdr.h
*
*  Contains the information for the tty processing that may
*  be specific to a certain vocoder.
*
*********************************************************************/

#ifndef _TTY_HDR_H_
#define _TTY_HDR_H_

#define FRAMESIZE   160

/*------------------------------------------------------------------
* 1 = MUTE ALL TTY DEBUGGING STATEMENTS
*-------------------------------------------------------------------*/

#define TTY_DEBUG_MUTE          1

/*------------------------------------------------------------------
* When PASS_TONES = 1, 1400 Hz. and 1800 Hz. tones will be passed
*-------------------------------------------------------------------*/

#define PASS_TONES              1

/*------------------------------------------------------------------
* TTY_ENC_MUTE is a flag for turning ON/OFF the muting in tty_enc()
* during the transition from NON_TTY to TTY mode.
*-------------------------------------------------------------------*/

#define TTY_ENC_MUTE            0

/*-------------------------------------------------------------------
* The following are the values that are transmitted in the header
* information that is sent along with the TTY characters in the
* speech packet.  There must be a minimum of 2 bits for
* the header.  One of the 4 values is for TTY_SILENCE_HDR, and a
* minimum of 2 values are needed for the character sequence numbers.
* There used to be a TTY_ONSET message that was sent to the decoder
* but evolution of the software has made this equivalent to TTY_SILENCE,
* so the ONSET message was made to look like TTY_SILENCE, and
* a 3 values of the header are used as sequence numbers.
*
* TTY_SILENCE_CHAR and TTY_ONSET_CHAR are the values that are sent
* in the 5 data bits when there is TTY_ONSET or TTY_SILENCE.  These
* values should be chosen so that the stuffed bits look like PITCH_MAX
* or PITCH_MIN+1, or some equally unlikely, outlying value that is
* unlikely to be sent during speech.  This reduces the false alarms
* for the decoder.  In general, you will want to stay away from using
* PITCH_MIN because it is commonly sent by encoders when the coder
* has zeros for inputs.
*-------------------------------------------------------------------*/

#define TTY_CHAR_HDR_START      1
#define TTY_CHAR_HDR_STOP       2
#define TTY_SILENCE_HDR         3
#define TTY_ONSET_HDR           3   /* Don't use ONSET, make it look like SILENCE */

#define TTY_SILENCE_CHAR        4  
#define TTY_ONSET_CHAR          4

/*-------------------------------------------------------------------
* The following are the values used internally by the TTY processing.
* The number of sequence of numbers used above must match the number
* used below.  The values used below are chosen so that each state
* has a bit assigned to it, making it easier to implement multiple
* conditions in if() statements in DSP assembly language.  None of
* the following values are transmitted to the decoder.
*--------------------------------------------------------------------*/

#define TTY_COUNTER_MIN         0x0001
#define TTY_SILENCE             0x0001
#define TTY_COUNTER_START       0x0002
#define TTY_COUNTER_STOP        0x0004
#define TTY_ONSET               0x0010  
#define NON_TTY                 0x0020  
#define TTY_FER                 0x0040 
#define TTY_EIGHTH_RATE         0x0080  
#define TTY_COUNTER_MAX         0x0080
#define COUNTER_BETWEEN_START_STOP  (0x02|0x04)


/*--------------------------------------------------------------------
* The following are the dit buffer sizes in the decoder.  It is
* placed in this header file in case it needs to be tailored for
* a specific implementation.  The minimum values recommended are:
*
*   #define TTY_BUF_SIZE            11   (9 lookahead + current frame + 1 lookback)
*   #define CURRENT_FRAME            9
*---------------------------------------------------------------------*/

#define TTY_BUF_SIZE            11   /* 9 lookahead + current frame + 1 lookback */
#define CURRENT_FRAME            9

#define DITS_PER_FRAME          10  /* 1 dit = 16 samples */

/*--------------------------------------------------------------------
* DIT2A_NON_TTY_HANGOVER is the number of TTY_ONSET detections that
* must be found before dit_to_ascii() will transition from NON_TTY
* to TTY_ONSET.  0 = no hangover, allowing the transition to occur
* as soon as TTY_ONSET is found. Setting the value to N means that
* N frames of TTY_ONSET will be converted to NON_TTY and the 
* transition will occur on the N+1 detection of TTY_ONSET.
*--------------------------------------------------------------------*/

#if PASS_TONES
#define DIT2A_NON_TTY_HANGOVER      0
#else
#define DIT2A_NON_TTY_HANGOVER      0
#endif

/*--------------------------------------------------------------------
* In tty_dec(), all NON_TTY, TTY_FER, and TTY_EIGHTH_RATE frames are
* converted to silence after the number of TTY_SILENCE frames exceeds
* CONVERT_TO_SILENCE_THRESH.  This is done to prevent TTY tones from
* passing through the decoder before the TTY processing has a chance
* to detect the character and regenerate the Baudot tones.
*--------------------------------------------------------------------*/

#define CONVERT_TO_SILENCE_THRESH   3

/*--------------------------------------------------------------------
In baudot_to_dit(), input signals whose levels are below this
threshold are not detected.  Without this threshold, the detector is
very sensitive to low level inputs and may detect very low level,
reflected tones.  This threshold may vary from coder to coder,
depending on the scaling, but it should be set to reject inputs
below -50 dBm.
*--------------------------------------------------------------------*/

#define TTY_MIN_INPUT_THRESH   9500 /* -50 dBm */

#endif /* TTY_HDR_H */

