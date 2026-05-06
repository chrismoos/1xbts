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
#define MAX_MS_BYTE_FOR_54_7       132
short
encode_da_fp (int *daindex, unsigned short fcb[][4])
{
    int i;
    int j, k, h[3], l;
    short dabit, o1, o2, o3;
    short inp[3];

    l = 0;
    for (i = 0; i < 3; i++)
	inp[i] = fcb[i][3];
    for (i = 0; i < 3; i++) {
	h[i] = 0;
	if (daindex[i] != 0)
	    h[i] = 1;
	l += h[i];
    }
    if (l == 0) {
	dabit = 0;
	o1 = MAX_MS_BYTE_FOR_54_7 * inp[1] + inp[2];
	o2 = o1 & 0x7f;
	o1 = o1 >> 7;
	fcb[0][3] = o1;
	fcb[1][3] = o2;
	fcb[2][3] = inp[0];
    }
    else {
	dabit = 1;
	if (l == 1) {
	    o1 = (2 * h[2] + h[1]) * 4 + (daindex[0] + daindex[1] +
					  daindex[2] - 1);
	    o1 = o1 << 3;
	    if (h[2] == 1) {
		o1 += inp[2];
		o2 = inp[0];
		o3 = inp[1];
	    }
	    else {
		if (h[1] == 1) {
		    o2 = MAX_MS_BYTE_FOR_54_7 * inp[1] + inp[2];
		    o3 = inp[0];
		}
		else {
		    o2 = MAX_MS_BYTE_FOR_54_7 * inp[0] + inp[2];
		    o3 = inp[1];
		}
		o1 += (o2 >> 7);
		o2 = o2 & 0x7f;
	    }
	    fcb[0][3] = o1;
	    fcb[1][3] = o2;
	    fcb[2][3] = o3;
	}
	else if (l == 2) {
	    o1 = (3 << 6) + ((2 * (1 - h[2]) + 1 * (1 - h[1])) << 4);
	    if (h[0] == 0) {
		o1 += ((daindex[1] - 1) << 2) + (daindex[2] - 1);
	    }
	    else {
		o1 += ((daindex[0] - 1) << 2) + (daindex[1] + daindex[2] - 1);
	    }
	    o2 = (o1 & 0x1) << 6;
	    o1 = o1 >> 1;
	    if (h[2] == 1) {
		o2 += (inp[2] << 3) + (h[0] * inp[0] + h[1] * inp[1]);
		o3 = ((h[0] == 0) ? inp[0] : inp[1]);
	    }
	    else {
		o3 = (inp[0] << 10) + MAX_MS_BYTE_FOR_54_7 * inp[1] + inp[2];
		o2 = o2 + (o3 >> 7);
		o3 = o3 & 0x7f;
	    }
	    fcb[0][3] = o1;
	    fcb[1][3] = o2;
	    fcb[2][3] = o3;

	}
	else {
	    o1 = (15 << 6) + ((daindex[0] - 1) << 4) +
		((daindex[1] - 1) << 2) + (daindex[2] - 1);

	    o2 = (o1 & 0x7) << 4;
	    o1 = o1 >> 3;
	    o3 = (inp[0] << 6) + (inp[1] << 3) + inp[2];
	    o2 += o3 >> 5;
	    o3 = o3 & 0x1f;
	    fcb[0][3] = o1;
	    fcb[1][3] = o2;
	    fcb[2][3] = o3;
	}
	fcb[0][3] += 128;

    }
    return (dabit);

}

void
decode_da_fp (int *daindex, unsigned short fcb[][4])
{
    int i;
    short o1, o2, o3;
    short inp[3];
    short h;
    int dabit;

    dabit = fcb[0][3] >> 7;
    fcb[0][3] &= 0x7f;

    for (i = 0; i < 3; i++)
	inp[i] = fcb[i][3];
    daindex[0] = daindex[1] = daindex[2] = 0;
    if (dabit == 0) {
	o1 = (inp[0] << 7) + inp[1];
	o2 = o1 % MAX_MS_BYTE_FOR_54_7;
	o1 = o1 / MAX_MS_BYTE_FOR_54_7;
	fcb[0][3] = inp[2];
	fcb[1][3] = o1;
	fcb[2][3] = o2;
    }
    else {
	if (inp[0] < (3 << 5)) {
	    h = inp[0] >> 5;
	    o2 = (inp[0] & 0x1f) >> 3;
	    daindex[h] = o2 + 1;
	    if (h == 2) {
		o1 = inp[0] & 0x7;
		fcb[2][3] = o1;
		fcb[0][3] = inp[1];
		fcb[1][3] = inp[2];
	    }
	    else {
		o2 = inp[0] & 7;
		o1 = (o2 << 7) + inp[1];
		o2 = o1 % MAX_MS_BYTE_FOR_54_7;
		o1 = o1 / MAX_MS_BYTE_FOR_54_7;
		if (h == 1) {
		    fcb[0][3] = inp[2];
		    fcb[1][3] = o1;
		}
		else {
		    fcb[1][3] = inp[2];
		    fcb[0][3] = o1;
		}
		fcb[2][3] = o2;
	    }

	}
	else if (inp[0] < (15 << 3)) {
	    h = (inp[0] >> 3) & 3;
	    daindex[h] = 0;
	    if (h != 2) {
		o1 = (inp[0] >> 1) & 3;
		daindex[0] = o1 + 1;
		o1 = ((inp[0] & 1) << 1) + (inp[1] >> 6);
		daindex[2] = o1 + 1;
		fcb[2][3] = (inp[1] >> 3) & 7;
		if (h != 1) {
		    fcb[1][3] = (inp[1]) & 7;
		    fcb[0][3] = (inp[2]);
		    daindex[1] = daindex[0];
		    daindex[0] = 0;
		}
		else {
		    fcb[0][3] = (inp[1]) & 7;
		    fcb[1][3] = (inp[2]);
		}
	    }
	    else {
		o1 = (inp[0] >> 1) & 3;
		daindex[0] = o1 + 1;
		o1 = ((inp[0] & 1) << 1) + (inp[1] >> 6);
		daindex[1] = o1 + 1;
		fcb[0][3] = (inp[1] >> 3) & 0x7;
		o1 = ((inp[1] & 7) << 7) + inp[2];
		o2 = o1 % MAX_MS_BYTE_FOR_54_7;
		o1 = o1 / MAX_MS_BYTE_FOR_54_7;
		fcb[1][3] = o1;
		fcb[2][3] = o2;
	    }


	}
	else {
	    o1 = (inp[0] >> 1) & 3;
	    daindex[0] = o1 + 1;
	    o1 = ((inp[0] & 1) << 1) + (inp[1] >> 6);
	    daindex[1] = o1 + 1;
	    o1 = (inp[1] >> 4) & 3;
	    daindex[2] = o1 + 1;
	    o1 = ((inp[1] & 0xf) << 5) + inp[2];
	    fcb[0][3] = o1 >> 6;
	    fcb[1][3] = (o1 >> 3) & 7;
	    fcb[2][3] = o1 & 7;

	}
    }
}
