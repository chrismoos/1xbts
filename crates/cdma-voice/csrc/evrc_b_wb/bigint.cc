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
#include<stdio.h>
#include "fplib_def.h"
extern Word16 MAXSIZE1;
extern Word16 DIV15[];


Word16
isgreaterB (BigInt a, BigInt b)
{
    Word16 i;

    for (i = MAXSIZE1 - 1; i >= 0; i--) {
	if (a.x[i] > b.x[i])
	    return (1);
	else {
	    if (a.x[i] < b.x[i])
		return (0);
	}
    }
    return (0);

}

Word16
islessB (BigInt a, BigInt b)
{
    Word16 i;

    for (i = MAXSIZE1 - 1; i >= 0; i--) {
	if (a.x[i] < b.x[i])
	    return (1);
	else {
	    if (a.x[i] > b.x[i])
		return (0);
	}
    }
    return (0);
}



Word16
isequalB (BigInt a, BigInt b)
{
    Word16 i;

    for (i = MAXSIZE1 - 1; i >= 0; i--) {
	if (a.x[i] != b.x[i])
	    return (0);
    }
    return (1);
}



BigInt
addBigInt (BigInt a, BigInt b)
{
    Word16 tmp, carry, tmp1, i;
    BigInt c;

    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	tmp = a.x[i] + b.x[i];
	if (tmp < 0) {
	    tmp = a.x[i] + MIN_16;
	    tmp = tmp + carry + b.x[i];
	    carry = 1;
	}
	else {
	    tmp += carry;
	    carry = 0;
	    if (tmp < 0) {
		carry = 1;
		tmp = 0;
	    }
	}
	c.x[i] = tmp;
    }
    return (c);
}


BigInt
subBigInt (BigInt a, BigInt b)
{
    BigInt c;
    Word16 tmp, carry, i;

    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	tmp = a.x[i] - carry;
	carry = 0;
	tmp = tmp - b.x[i];
	if (tmp < 0) {
	    tmp = tmp + MAX_16 + 1;
	    carry = 1;
	}
	c.x[i] = tmp;
    }
    return (c);
}




BigInt
mulBigInt (BigInt a, BigInt b)
{
    Word32 tmp, carry;
    BigInt c;
    Word16 i, j;

    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	tmp = carry;
	carry = 0;
	for (j = 0; j <= i; j++) {
	    tmp += (a.x[i - j] * b.x[j]);
	    if (tmp > 0x7fff) {
		carry = carry + (tmp >> 15);
		tmp = tmp & (0x7fff);
	    }
	}
	tmp = tmp & (0x7fff);
	c.x[i] = tmp;
    }

    return (c);

}


BigInt
shiftright (BigInt a, Word16 m)
{
    BigInt result, b;
    Word16 i, j, m1, num, m2, m3, m4;
    Word16 msb;

    m1 = m / 15;
    m = m - m1 * 15;
    num = MAXSIZE1 - m1;
    for (i = 0; i < num; i++) {
	result.x[i] = a.x[i + m1];
    }
    for (i = num; i < MAXSIZE1; i++) {
	result.x[i] = 0;
    }

    j = num - 1;
    b.x[j] = result.x[j];
    result.x[j] = (b.x[j] >> m);
    m2 = (0x1 << m) - 1;
    msb = b.x[j] & m2;
    m3 = 15 - m;
    for (j = num - 2; j >= 0; j--) {
	b.x[j] = result.x[j];
	m4 = b.x[j] >> m;
	m4 |= (msb << m3);
	result.x[j] = m4;
	msb = b.x[j] & m2;
    }
    return (result);
}

BigInt
shiftleft (BigInt a, Word16 m)
{
    BigInt result, b;
    Word16 i, j, m1, m2, m3;
    Word16 msb, num;
    Word16 tmp = m;

    m1 = m / 15;
    m = m - 15 * m1;
    num = (MAXSIZE1 - m1);
    for (i = 1; i <= num; i++) {
	result.x[MAXSIZE1 - i] = a.x[num - i];
    }

    for (i = i; i <= MAXSIZE1; i++) {
	result.x[MAXSIZE1 - i] = 0;
    }

    b.x[m1] = result.x[m1];
    result.x[m1] = b.x[m1] & (MAX_16 >> m);
    result.x[m1] = (result.x[m1] << m);
    m2 = 15 - m;
    msb = (b.x[m1] >> (m2));
    for (j = (m1 + 1); j < MAXSIZE1; j++) {
	b.x[j] = result.x[j];
	m3 = b.x[j] & (MAX_16 >> m);
	m3 = (m3 << m) + msb;
	result.x[j] = m3;
	msb = (b.x[j] >> (m2));
    }
    return (result);
}

BigInt
divBigInt (BigInt a, int b, Word16 * rem)
{
    int i, j;
    BigInt result;
    long long x1, x2, x3, x4, x5;

    x2 = 0;
    for (j = (MAXSIZE1 - 1); j >= 0; j--) {
	x1 = (x2 << 7) + (a.x[j] >> 8);
	x4 = x1 / b;
	x2 = x1 - x4 * b;
	x3 = (x2 << 8) + (a.x[j] & 0xff);
	x5 = x3 / b;
	result.x[j] = (x4 << 8) + x5;
	x2 = x3 - x5 * b;
    }
    return (result);

}

BigInt
divBigInt (BigInt a, BigInt b, BigInt * rem)
{
    Word16 i;
    BigInt result;
    Word16 bits;

    for (i = 0; i < MAXSIZE1; i++) {
	rem->x[i] = 0;
	result.x[i] = a.x[i];
    }
    bits = MAXSIZE1 * 15;
    for (i = 0; i < bits; i++) {
	*rem = shiftleft (*rem, 1);
	if ((result.x[(MAXSIZE1 - 1)] & 16384) != 0)
	    rem->x[0] = (rem->x[0] + 1);
	result = shiftleft (result, 1);
	if (!isgreaterB (b, *rem)) {
	    result.x[0] = (result.x[0] + 1);
	    *rem = subBigInt (*rem, b);
	}
    }
    return (result);
}
