#ifndef __DSP_MATH_H__
#define __DSP_MATH_H__

#include "typedefs.h"

#if	__ARM_ARCH__	> 5

#ifdef __cplusplus
extern "C" {
#endif


Shortword abs_s(Shortword var1);
Shortword negate(Shortword var1);
Shortword norm_s(Shortword var1);
Shortword saturate(Shortword var1);

Shortword norm_l(Longword L_var1);
Shortword round32(Longword L_var1);

Longword L_abs(Longword L_var1);
Longword L_negate(Longword L_var1);


Shortword add(Shortword var1,Shortword var2);
Shortword sub(Shortword var1,Shortword var2);
Shortword mult(Shortword var1,Shortword var2);
Shortword shr(Shortword var1,Shortword var2);
Shortword shl(Shortword var1,Shortword var2);
Shortword divide_s(Shortword var1,Shortword var2);
Shortword mult_r(Shortword var1,Shortword var2);
Shortword shr_r(Shortword var1,Shortword var2);


Longword L_shr(Longword L_var1,Shortword var2);
Longword L_shl(Longword L_var1,Shortword var2);
Longword L_shr_r(Longword L_var1,Shortword var2);

Longword L_add(Longword L_var1,Longword L_var2);
Longword L_sub(Longword L_var1,Longword L_var2);
Longword div_32(Longword L_var1,Longword L_var2);

Shortword msu_r(Longword L_var3, Shortword var1, Shortword var2);
Shortword mac_r(Longword L_var3, Shortword var1, Shortword var2);
Longword L_mac(Longword L_var3,Shortword var1,Shortword var2);
Longword L_msu(Longword L_var3,Shortword var1,Shortword var2);


Longword L_mult(Shortword var1,Shortword var2);

Shortword shift_r(Shortword var1,Shortword var2);
Longword L_shift_r(Longword L_var1,Shortword var2);



Longword L_mpy_ll(Longword L_var1,Longword L_var2);
Longword L_mpy_ls(Longword L_var2, Shortword var1);

void c_fft_neon(Shortword* array,Shortword isign);

#ifdef __cplusplus
}
#endif

#else
#include "mathevrc.h"
#include "mathdp31.h"
#endif

#define L_deposit_h(var1)	((Longword)( (Longword)(var1) << 16 ))
#define L_deposit_l(var1)	( (Longword)(var1) )
#define extract_h(L_var1)	((Shortword)( 0x0000FFFF & (L_var1 >> 16) ))
#define extract_l(L_var1)	((Shortword)( 0x0000FFFF & (L_var1) ))



#endif

