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
#include "fplib_def.h"
#define LOGPOW 5


Word16 MAXSIZE1 = MAXSIZE;
Word16 DIV15[150];
Word16 logof2[150];

VPint::VPint ()
{
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
}

VPint::~VPint ()
{
}


VPint::VPint (Word16 val)
{
    Word16 i;

    for (i = 1; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
    mpvalue.x[0] = val;

}

VPint::VPint (const VPint & vpint)
{
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	mpvalue.x[i] = vpint.mpvalue.x[i];
    }
}

VPint::VPint (BigInt T)
{
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	mpvalue.x[i] = T.x[i];
    }
}

Word16
VPint::getKthWord (Word16 k)
{
    if (((k - MAXSIZE1) > 0) || (k == 0))
	return (0);
    return (mpvalue.x[k - 1]);
}


VPint & VPint::operator= (const VPint & vpint)
{
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	mpvalue.x[i] = vpint.mpvalue.x[i];
    }
    return (*this);
}


VPint & VPint::operator= (Word16 vpint)
{
    Word16 i;

    mpvalue.x[0] = vpint;
    for (i = 1; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
    return (*this);
}

VPint & VPint::operator= (int vpint)
{
    Word16 i;

    mpvalue.x[0] = vpint & 0x7fff;
    mpvalue.x[1] = vpint >> 15;
    for (i = 2; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
    return (*this);
}


VPint
VPint::operator+ (const VPint & vpint)
{
    VPint result;

    result.mpvalue = addBigInt (mpvalue, vpint.mpvalue);
    return (result);
}

VPint
VPint::operator- (const VPint & vpint)
{
    VPint result;

    result.mpvalue = subBigInt (mpvalue, vpint.mpvalue);
    return (result);
}

VPint
VPint::operator* (const VPint & vpint)
{
    VPint result;
    int j;

    for (j = 1; j < MAXSIZE1; j++) {
	if (vpint.mpvalue.x[j] != 0) {
	    result.mpvalue = mulBigInt (mpvalue, vpint.mpvalue);
	    return (result);
	}
    }

    result = (*this) * (int) (vpint.mpvalue.x[0]);
    return (result);
}

VPint
VPint::operator* (Word16 vpint)
{
    VPint result;
    Word16 i;
    Word32 a, carry;

    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	a = carry + (mpvalue.x[i] * vpint);
	carry = (a >> 15);
	result.mpvalue.x[i] = (a & 0x7fff);
    }
    return (result);
}

VPint
VPint::operator* (int vpint)
{
    VPint result, result1;
    Word16 i, x, y;
    Word32 a, carry;

    x = vpint >> 15;
    y = vpint & 0x7fff;

    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	a = carry + (mpvalue.x[i] * y);
	carry = (a >> 15);
	result.mpvalue.x[i] = (a & 0x7fff);
    }
    carry = 0;
    for (i = 0; i < MAXSIZE1; i++) {
	a = carry + (mpvalue.x[i] * x);
	carry = (a >> 15);
	result1.mpvalue.x[i] = (a & 0x7fff);
    }
    for (i = MAXSIZE - 1; i >= 0; i--) {
	result1.mpvalue.x[i] = result1.mpvalue.x[i - 1];
    }
    result1.mpvalue.x[0] = 0;

    return (result + result1);
}

VPint
VPint::operator/ (const VPint & vpint)
{
    VPint result;
    VPint zero ((Word16) 0);
    BigInt c;
    Word16 a;
    int j;

    if (zero == vpint) {
	fprintf (stderr, " Divide By 0\n");
	return (0);
    }
    result.mpvalue = divBigInt (mpvalue, vpint.mpvalue, &c);
    return (result);
}


VPint
VPint::operator/ (Word16 vpint)
{
    VPint result;
    Word16 a;
    int j;

    if (vpint == 0) {
	fprintf (stderr, " Divide By 0\n");
	return (0);
    }
    result.mpvalue = divBigInt (mpvalue, vpint, &a);
    return (result);
}

VPint
VPint::operator/ (int vpint)
{
    VPint result, r1;
    Word16 a;
    int j;

    if (vpint == 0) {
	fprintf (stderr, " Divide By 0\n");
	return (0);
    }
    result.mpvalue = divBigInt (mpvalue, vpint, &a);
    return (result);
}

VPint
VPint::operator% (const VPint & vpint)
{
    VPint result;
    VPint zero ((int) 0);
    BigInt c;
    Word16 i;

    if (zero == vpint) {
	fprintf (stderr, " Divide By 0\n");
	exit (1);
    }
    result.mpvalue = divBigInt (mpvalue, vpint.mpvalue, &c);
    for (i = 0; i < MAXSIZE1; i++) {
	result.mpvalue.x[i] = c.x[i];
    }
    return (result);
}

VPint & VPint::operator+= (const VPint & vpint)
{
    mpvalue = addBigInt (mpvalue, vpint.mpvalue);
    return (*this);
}

VPint & VPint::operator-= (const VPint & vpint)
{
    mpvalue = subBigInt (mpvalue, vpint.mpvalue);
    return (*this);
}

VPint & VPint::operator++ ()
{
    VPint
	a;

    a = (Word16) 0x01;
    mpvalue = addBigInt (mpvalue, a.mpvalue);
    return (*this);
}

VPint & VPint::operator-- ()
{
    VPint
	a;

    a = (Word16) 0x1;
    mpvalue = subBigInt (mpvalue, a.mpvalue);
    return (*this);
}


Boolean
VPint::operator< (const VPint & vpint)
{
    Boolean retval;

    retval = islessB (mpvalue, vpint.mpvalue);
    return (retval);
}

Boolean
VPint::operator> (const VPint & vpint)
{
    Boolean retval;

    retval = isgreaterB (mpvalue, vpint.mpvalue);
    return (retval);
}

Boolean
VPint::operator<= (const VPint & vpint)
{
    Boolean retval;

    retval = isgreaterB (mpvalue, vpint.mpvalue);
    return (!retval);
}

Boolean
VPint::operator>= (const VPint & vpint)
{
    return (!islessB (mpvalue, vpint.mpvalue));
}

Boolean
VPint::operator== (const VPint & vpint)
{
    return (isequalB (mpvalue, vpint.mpvalue));
}

Boolean
VPint::operator!= (const VPint & vpint)
{
    Boolean retval;

    return (!isequalB (mpvalue, vpint.mpvalue));
}


VPint
VPint::operator| (const VPint & vpint)
{
    VPint result;
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	result.mpvalue.x[i] = mpvalue.x[i] | vpint.mpvalue.x[i];
    }
    return (result);
}

VPint
VPint::operator& (const VPint & vpint)
{
    VPint result;
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	result.mpvalue.x[i] = mpvalue.x[i] & vpint.mpvalue.x[i];
    }
    return (result);
}

VPint
VPint::operator^ (const VPint & vpint)
{
    VPint result;
    Word16 i;

    for (i = 0; i < MAXSIZE1; i++) {
	result.mpvalue.x[i] = mpvalue.x[i] ^ vpint.mpvalue.x[i];
    }
    return (result);
}

VPint
VPint::operator<< (Word16 shift)
{
    VPint result;

    result.mpvalue = shiftleft (mpvalue, (Word16) shift);
    return (result);
}

VPint
VPint::operator>> (Word16 shift)
{
    VPint result;

    result.mpvalue = shiftright (mpvalue, (Word16) shift);
    return (result);
}

Word16 *
VPint::unpackToWordArray (Word16 * array, Word16 size)
{
    Word16 i;

    for (i = 0; i < size; i++) {
	array[i] = mpvalue.x[i];
    }
    return (array);
}

int *
VPint::unpackToWordArray (int *array, Word16 size)
{
    Word16 i;

    for (i = 0; i < size; i++) {
	array[i] = mpvalue.x[i];
    }
    return (array);
}

void
VPint::packFromWordArray (Word16 * array, Word16 size)
{
    Word16 i;

    for (i = 0; i < size; i++) {
	mpvalue.x[i] = array[i];
    }
    for (i = size; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
}

void
VPint::packFromWordArray (int *array, Word16 size)
{
    Word16 i;

    for (i = 0; i < size; i++) {
	mpvalue.x[i] = array[i];
    }
    for (i = size; i < MAXSIZE1; i++) {
	mpvalue.x[i] = 0;
    }
}

void
VPint::clear (Word16 SIZE, Word16 SIZE2)
{
    int i;

    for (i = SIZE; i < SIZE2; i++) {
	mpvalue.x[i] = 0;
    }
}


VPint
VPint::extract (Word16 bits)
{
    Word16 m1, m, i;
    VPint s;
    Word16 a = 0x1;

    m1 = bits / 15;
    m = bits - 15 * m1;
    for (i = 0; i < m1; i++) {
	s.mpvalue.x[i] = mpvalue.x[i];
    }
    s.mpvalue.x[i] = mpvalue.x[i] & ((a << m) - 1);
    return (s);
}
