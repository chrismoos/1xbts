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
    "$Id: io.cc,v 1.8.22.4 2007/06/24 06:43:10 apadmana Exp $";

/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for             */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 1999 Qualcomm Incorporated. All rights                */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <fcntl.h>
#include <memory.h>
#include <sys/types.h>
#include <sys/file.h>
#include <sys/stat.h>
#include <stdio.h>
#include <unistd.h>
#include <stdlib.h>

/* open_binary_input_file - opens a binary input file		*/
/* 19911017							*/
void
open_binary_input_file (int *fin, char *filename, int verbose)
{
    if ((*fin = open (filename, 0)) == -1) {
	fprintf (stderr, "Error opening file - %s\n", filename);
	exit (-1);
    }
    if (verbose)
	fprintf (stderr, "Binary input file  - %s\n", filename);
    lseek (*fin, 0, L_SET);
}

/* open_binary_output_file - opens a binary output file		*/
/* 19911017							*/
void
open_binary_output_file (int *fout, char *filename, int verbose)
{
    if (verbose)
	fprintf (stderr, "Binary output file - %s\n", filename);
    if ((*fout = creat (filename, 0644)) == -1) {
	fprintf (stderr, "Error opening file - %s\n", filename);
	exit (-1);
    }
    lseek (*fout, 0, L_SET);
}

/* read_samples - reads "insamples" number of samples from 	*/
/*	binary input file "fin" to float array "inbuf"		*/
/* 	The samples range from -2^15 to 2^15 -1.		*/
/* 	Returns the number of samples read.			*/
/* 19911017							*/
int
read_samples (int fin, float *inbuf, int insamples)
{

    int i;
    int bytes;
    int samplesread;
    short swap;
    short *tmpbuf;

    tmpbuf = new short[insamples];

    bytes = read (fin, (char *) tmpbuf, insamples * sizeof (short));

    samplesread = bytes / sizeof (short);

    for (i = 0; i < samplesread; i++)
	inbuf[i] = (float) tmpbuf[i];
    delete[]tmpbuf;

    return (samplesread);
}

int
read_ints (int fin, int *inbuf, int insamples)
{

    int i;
    int bytes;
    int samplesread;
    short swap;
    short *tmpbuf;

    tmpbuf = new short[insamples];

    bytes = read (fin, (char *) tmpbuf, insamples * sizeof (short));

    samplesread = bytes / sizeof (short);
    for (i = 0; i < samplesread; i++)
	inbuf[i] = (int) tmpbuf[i];
    delete[]tmpbuf;

    return (samplesread);
}

/* write_samples - writes "outsamples" number of samples from	*/
/*	float array "outsamples" to binary output file "fout"	*/
/*	Saturates the samples to the allowable range.		*/
/*	Returns the number of samples written.			*/
int
write_samples (int fout, float *outbuf, int outsamples)
{

    int bytes;
    int i;
    short swap;
    float ftmp;
    short *tmpbuf;

    tmpbuf = new short[outsamples];

    for (i = 0; i < outsamples; i++) {
	ftmp = *(outbuf + i);
	ftmp = (ftmp < -32768.0) ? -32768.0 : ftmp;
	ftmp = (ftmp > 32767.0) ? 32767.0 : ftmp;
	tmpbuf[i] = (short) ftmp;
    }

    if ((bytes = write (fout, (char *) tmpbuf, outsamples * sizeof (short)))
	!= outsamples * sizeof (short)) {
	fprintf (stderr, "Error writing data to output file\n");
	exit (-1);
    }

    delete[]tmpbuf;
    return (bytes / sizeof (short));
}


/* write_ints - writes "outsamples" number of samples from	*/
/*	float array "outsamples" to binary output file "fout"	*/
/*	Saturates the samples to the allowable range.		*/
/*	Returns the number of samples written.			*/
int
write_ints (int fout, int *outbuf, int outsamples)
{

    int bytes;
    int i;
    short swap;
    float ftmp;
    short *tmpbuf;

    tmpbuf = new short[outsamples];

    for (i = 0; i < outsamples; i++) {
	ftmp = *(outbuf + i);
	ftmp = (ftmp < -32768.0) ? -32768.0 : ftmp;
	ftmp = (ftmp > 32767.0) ? 32767.0 : ftmp;
	tmpbuf[i] = (short) ftmp;
    }

    if ((bytes = write (fout, (char *) tmpbuf, outsamples * sizeof (short)))
	!= outsamples * sizeof (short)) {
	fprintf (stderr, "Error writing data to output file\n");
	exit (-1);
    }

    delete[]tmpbuf;
    return (bytes / sizeof (short));
}


int
read_floats (int fin, float *inbuf, int insamples)
{

    int bytes;

    bytes = read (fin, (char *) inbuf, insamples * sizeof (float));
    return (bytes / sizeof (float));

}

int
write_floats (int fout, float *outbuf, int outsamples)
{

    int bytes;

    bytes = write (fout, (char *) outbuf, outsamples * sizeof (float));
    return (bytes / sizeof (float));
}

int
read_doubles (int fin, double *inbuf, int insamples)
{

    int bytes;

    bytes = read (fin, (char *) inbuf, insamples * sizeof (double));
    return (bytes / sizeof (double));

}

int
write_doubles (int fout, double *outbuf, int outsamples)
{

    int bytes;

    bytes = write (fout, (char *) outbuf, outsamples * sizeof (double));
    return (bytes / sizeof (double));
}
