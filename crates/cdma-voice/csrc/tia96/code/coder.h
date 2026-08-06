/**********************************************************************/
/* QCELP Variable Rate Speech Codec - Simulation of TIA IS96-A, service */
/*     option one for TIA IS95, North American Wideband CDMA Digital  */
/*     Cellular Telephony.                                            */
/*                                                                    */
/* (C) Copyright 1993, QUALCOMM Incorporated                          */
/* QUALCOMM Incorporated                                              */
/* 10555 Sorrento Valley Road                                         */
/* San Diego, CA 92121                                                */
/*                                                                    */
/* Note:  Reproduction and use of this software for the design and    */
/*     development of North American Wideband CDMA Digital            */
/*     Cellular Telephony Standards is authorized by                  */
/*     QUALCOMM Incorporated.  QUALCOMM Incorporated does not         */
/*     authorize the use of this software for any other purpose.      */
/*                                                                    */
/*     The availability of this software does not provide any license */
/*     by implication, estoppel, or otherwise under any patent rights */
/*     of QUALCOMM Incorporated or others covering any use of the     */
/*     contents herein.                                               */
/*                                                                    */
/*     Any copies of this software or derivative works must include   */
/*     this and all other proprietary notices.                        */
/**********************************************************************/
/* coder.h - contains most of the parameters for the QCELP algorithm */

#include"defines.h"

/**************************/
/* Basic Coder Parameters */
/**************************/
#define FSIZE      160        /* Overall frame size                     */
#define NUMRATES     5

/**********************/
/* LPC/LSP Parameters */
/**********************/
#define LPCSIZE    160        /* LPC frame size                         */
#define LPCOFFSET   60        /* Offset of LPC frame to Fsize           */
#define LPCORDER    10
#define LPC_CENTER LPCOFFSET+(LPCSIZE)/2

static const float MIN_DELTA_LSP[NUMRATES][LPCORDER] =
{{ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
{ -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01},
{ -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01, -.01},
{ -.015, -.015, -.03, -.03, -.03, -.02, -.02, -.02, -.02, -.02 },
{ .015-.04545, .025-.09091, .05-.13636, .085-.18182, .120-.22727,
.170-.27273, .215-.31818, .265-.36364, .335-.40909, .38-.45455}};

static const float MAX_DELTA_LSP[NUMRATES][LPCORDER] =
{{ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
{  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01},
{  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01,  .01},
{  .015,  .015,  .03,  .03,  .03,  .02,  .02,  .02,  .02,  .02 },
{ .100-.04545, .125-.09091, .190-.13636, .245-.18182, .290-.22727,
.325-.27273, .370-.31818, .40-.36364, .440-.40909, .47-.45455}};


static const INTTYPE NUM_LSP_QLEVELS[NUMRATES][LPCORDER] =
{{ 0, 0, 0, 0, 0, 0, 0, 0, 0, 0},
{ 2, 2, 2, 2, 2, 2, 2, 2, 2, 2},
{ 2, 2, 2, 2, 2, 2, 2, 2, 2, 2},
{ 4, 4, 4, 4, 4, 4, 4, 4, 4, 4},
{ 16, 16, 16, 16, 16, 16, 16, 16, 16, 16}};

static const float LSP_DPCM_DECAY[NUMRATES]={ .90625, .90625, .90625, .90625, 0.0};
#define LSP_SPREAD_FACTOR           .01
static const float SMOOTH_FACTOR[NUMRATES][2] =
{{0.0, 0.0}, {0.125, 0.9}, {0.125, 0.9}, {0.125, 0.125}, {0.0, 0.0}};
#define BWE_FACTOR                  .9883
#define PERCEPT_WGHT_FACTOR         .8
#define PF_ZERO_WGHT_FACTOR         .5
#define PF_POLE_WGHT_FACTOR         .8
#define AGC_FACTOR                  .9375

/****************************/
/* Rate Decision Parameters */
/****************************/
static const float RATE_THRESH[2][3][3] =
{ {{ -0.5544613/100000.0,  4.047152,  362},
   { -1.5297330/100000.0,  8.750045, 1136},
   { -3.9570500/100000.0, 18.89962,  3347}},
  {{  0.9043945/10000000.0, 3.535748, -62071},
   { -1.9860070/10000000.0, 4.941658, 223951},
   { -4.8384770/10000000.0, 8.63002,  645864}} };
#define LOW_THRESH_LIM        160000
#define HIGH_THRESH_LIM      5059644
#define THRESH_UPDATE_FACTOR 1.00547

/********************/
/* Pitch Parameters */
/********************/
static const INTTYPE PITCHSF[NUMRATES] = {0, 0, 1, 2, 4};
#define MAX_PITCH_SF 4             /* used for array size definitions */
#define LENGTH_OF_IMPULSE_RESPONSE  20 /* used in pitch & cb searches */
#define MINB                 0.0
#define MAXB                 2.0
#define NUMBER_OF_B_LEVELS   9
#define MINLAG      17          /* The minimum and maximum pitch lags */
#define MAXLAG     143
                               /* pitch gain saturation for erasures  */
                               /* depending on the # of errs in a row */
static const float ERASE_B_SAT[5]= {2.0, 0.9, 0.6, 0.3, 0.0};
                               /* pitch gain code saturation for FRLs */
                               /* depending on the # of errs in a row */
static const float FRL_B_SAT[5]  = {2.0, 0.75, 0.5, 0.25, 0.0};

/***********************/
/* Codebook Parameters */
/***********************/
static const INTTYPE CBSF[NUMRATES] = {0, 1, 2, 4, 8};
#define GORDER               2

#define CBLENGTH   128

static const float CODEBOOK[CBLENGTH]= {
    0.0, -2.0,  0.0, -1.5,  0.0,  0.0,  0.0,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,
    0.0, -1.5, -1.0,  0.0,  0.0,  0.0,  0.0,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  2.5,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  2.0,  0.0,
    0.0,  1.5,  1.0,  0.0,  1.5,  2.0,  0.0,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  1.5,  0.0,  0.0,
   -1.5,  1.5,  0.0,  0.0, -1.0,  0.0,  1.5,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0, -2.5,  0.0,
    0.0,  0.0,  0.0,  1.5,  0.0,  0.0,  0.0,  1.5,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  2.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,
    0.0,  1.5,  3.0, -1.5, -2.0,  0.0, -1.5, -1.5,
    1.5, -1.5,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,
    0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0,  0.0 };

static const INTTYPE FG[73] =
{  -2, -2, -2, -2, -1,  0,  0,  0,  1,  2,  3,  4,  5,
    6,  7,  8,  9, 10, 11, 12, 13, 14, 15, 16, 17, 18,
   18, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27, 27, 28,
   29, 30, 31, 32, 33, 34, 35, 36, 36, 37, 38, 39, 40,
   41, 42, 43, 44, 45, 45, 46, 47, 48, 49, 50, 51, 52,
   53, 54, 54, 55, 56, 57, 58, 58};

static const INTTYPE QG[NUMRATES][4]=
{{0, 0, 0, 0}, {-4, -2, 0, 2}, {-4, -2, 0, 2}, {-4, 0, 4, 8}, {-4, 0, 4, 8}};

static const float GA[73] =
{   .5, .5, .625, .75, .75, .875, 1.0, 1.125, 1.25, 1.375, 1.625,
    1.75, 2.0, 2.25, 2.5, 2.875, 3.125, 3.5, 4.0, 4.5, 5.0,
    5.625, 6.25, 7.125, 8.0, 8.875, 10.0, 11.25, 12.625, 14.125,
    15.875, 17.75, 20.0, 22.375, 25.125, 28.125, 31.625,
    35.5, 39.75, 44.625, 50.125, 56.25, 63.125, 70.75, 79.375,
    89.125, 100.0, 112.25, 125.875, 141.25, 158.5, 177.875, 199.5,
    223.875, 251.25, 281.875, 316.25, 354.875, 398.125, 446.625,
    501.125, 562.375, 631.0, 708.0, 794.375, 891.25, 1000.0, 1122.0,
    1258.875, 1412.5, 1584.875, 1778.25, 1995.25};

/***************************/
/* Data Packing Parameters */
/***************************/
#define WORDS_PER_PACKET 12
static const INTTYPE NUMBITS[NUMRATES] = {0, 16, 40, 80, 171};

/* In these tables, "0" corresponds to the first parameter in a set */
/* rather than "1" as specified in PN 3119                          */
/* This allows indices starting at 0, as is the norm in C programs  */
/* For example, the lowest LSP is LSP0, not LSP1                    */
static const INTTYPE BIT_DATA4[171][3] =
{  {LSP, 0, 2}, {LSP, 0, 3}, {LSP, 1, 2}, {LSP, 1, 3}, /* 167 */
    {LSP, 2, 2}, {LSP, 2, 3}, {LSP, 3, 2}, {LSP, 3, 3}, /* 163 */
    {LSP, 4, 2}, {LSP, 4, 3}, {LSP, 5, 2}, {LSP, 5, 3}, /* 159 */
    {LSP, 6, 2}, {LSP, 6, 3}, {LSP, 7, 2}, {LSP, 7, 3}, /* 155 */
    {LSP, 8, 2}, {LSP, 8, 3}, {LSP, 9, 2}, {LSP, 9, 3}, /* 151 */
    {LSP, 0, 1}, {LSP, 0, 0}, {LSP, 1, 1}, {LSP, 1, 0}, /* 147 */
    {LSP, 2, 1}, {LSP, 2, 0}, {LSP, 3, 1}, {CBGAIN, 0, 1}, /* 143 */
    {LSP, 3, 0}, {LSP, 4, 1}, {LSP, 4, 0}, {LSP, 5, 1}, /* 139 */
    {LSP, 5, 0}, {LSP, 6, 1}, {LSP, 6, 0}, {CBGAIN, 1, 1}, /* 135 */
    {LSP, 7, 1}, {LSP, 7, 0}, {LSP, 8, 1}, {LSP, 8, 0}, /* 131*/
    {LSP, 9, 1}, {LSP, 9, 0}, {PGAIN, 0, 2}, {CBGAIN, 2, 1}, /* 127 */
    {PGAIN, 0, 1}, {PGAIN, 0, 0}, {PLAG, 0, 6}, {PLAG, 0, 5}, /* 123 */
    {PLAG, 0, 4}, {PLAG, 0, 3}, {PLAG, 0, 2}, {CBGAIN, 3, 1}, /* 119 */
    {PLAG, 0, 1}, {PLAG, 0, 0}, {CBINDEX, 0, 6}, {CBINDEX, 0, 5}, /* 115 */
    {CBINDEX, 0, 4}, {CBINDEX, 0, 3}, {CBINDEX, 0, 2}, {CBGAIN, 4, 1},/* 111 */
    {CBINDEX, 0, 1}, {CBINDEX, 0, 0}, {CBGAIN, 0, 2}, {CBGAIN, 0, 0}, /* 107 */
    {CBINDEX, 1, 6}, {CBINDEX, 1, 5}, {CBINDEX, 1, 4}, {CBGAIN, 5, 1},/* 103 */
    {CBINDEX, 1, 3}, {CBINDEX, 1, 2}, {CBINDEX, 1, 1}, {CBINDEX, 1, 0},/* 99 */
    {CBGAIN, 1, 2}, {CBGAIN, 1, 0}, {PGAIN, 1, 2}, {CBGAIN, 6, 1}, /* 95 */
    {PGAIN, 1, 1}, {PGAIN, 1, 0}, {PLAG, 1, 6}, {PLAG, 1, 5}, /* 91 */
    {PLAG, 1, 4}, {PLAG, 1, 3}, {PLAG, 1, 2}, {CBGAIN, 7, 1}, /* 87 */
    {PLAG, 1, 1}, {PLAG, 1, 0}, {CBINDEX, 2, 6}, {CBINDEX, 2, 5}, /* 83 */
    {CBINDEX, 2, 4}, {CBINDEX, 2, 3}, {CBINDEX, 2, 2}, {PCB, 0, 10}, /* 79 */
    {CBINDEX, 2, 1}, {CBINDEX, 2, 0}, {CBGAIN, 2, 2}, {CBGAIN, 2, 0}, /* 75 */
    {CBINDEX, 3, 6}, {CBINDEX, 3, 5}, {CBINDEX, 3, 4}, {PCB, 0, 9}, /* 71 */
    {CBINDEX, 3, 3}, {CBINDEX, 3, 2}, {CBINDEX, 3, 1}, {CBINDEX, 3, 0},/* 67 */
    {CBGAIN, 3, 2}, {CBGAIN, 3, 0}, {PGAIN, 2, 2}, {PCB, 0, 8}, /* 63 */
    {PGAIN, 2, 1}, {PGAIN, 2, 0}, {PLAG, 2, 6}, {PLAG, 2, 5}, /* 59 */
    {PLAG, 2, 4}, {PLAG, 2, 3}, {PLAG, 2, 2}, {PCB, 0, 7}, /* 55 */
    {PLAG, 2, 1}, {PLAG, 2, 0}, {CBINDEX, 4, 6}, {CBINDEX, 4, 5}, /* 51 */
    {CBINDEX, 4, 4}, {CBINDEX, 4, 3}, {CBINDEX, 4, 2}, {PCB, 0, 6}, /* 47 */
    {CBINDEX, 4, 1}, {CBINDEX, 4, 0}, {CBGAIN, 4, 2}, {CBGAIN, 4, 0}, /* 43 */
    {CBINDEX, 5, 6}, {CBINDEX, 5, 5}, {CBINDEX, 5, 4}, {PCB, 0, 5}, /* 39 */
    {CBINDEX, 5, 3}, {CBINDEX, 5, 2}, {CBINDEX, 5, 1}, {CBINDEX, 5, 0},/* 35 */
    {CBGAIN, 5, 2}, {CBGAIN, 5, 0}, {PGAIN, 3, 2}, {PCB, 0, 4}, /* 31 */
    {PGAIN, 3, 1}, {PGAIN, 3, 0}, {PLAG, 3, 6}, {PLAG, 3, 5}, /* 27 */
    {PLAG, 3, 4}, {PLAG, 3, 3}, {PLAG, 3, 2}, {PCB, 0, 3}, /* 23 */
    {PLAG, 3, 1}, {PLAG, 3, 0}, {CBINDEX, 6, 6}, {CBINDEX, 6, 5}, /* 19 */
    {CBINDEX, 6, 4}, {CBINDEX, 6, 3}, {CBINDEX, 6, 2}, {PCB, 0, 2}, /* 15 */
    {CBINDEX, 6, 1}, {CBINDEX, 6, 0}, {CBGAIN, 6, 2}, {CBGAIN, 6, 0}, /* 11 */
    {CBINDEX, 7, 6}, {CBINDEX, 7, 5}, {CBINDEX, 7, 4}, {PCB, 0, 1}, /* 7 */
    {CBINDEX, 7, 3}, {CBINDEX, 7, 2}, {CBINDEX, 7, 1}, {CBINDEX, 7, 0}, /* 3 */
    {CBGAIN, 7, 2}, {CBGAIN, 7, 0}, {PCB, 0, 0}}; /* 0 */

static const INTTYPE BIT_DATA3[80][3] =
{ {LSP, 0, 1},  {LSP, 0, 0},  {LSP, 1, 1},  {LSP, 1, 0},  {LSP, 2, 1},
  {LSP, 2, 0},  {LSP, 3, 1},  {LSP, 3, 0},  {LSP, 4, 1},  {LSP, 4, 0},
  {LSP, 5, 1},  {LSP, 5, 0},  {LSP, 6, 1},  {LSP, 6, 0},  {LSP, 7, 1},
  {LSP, 7, 0},  {LSP, 8, 1},  {LSP, 8, 0},  {LSP, 9, 1},  {LSP, 9, 0},
  {PGAIN, 0, 2}, {PGAIN, 0, 1}, {PGAIN, 0, 0},
  {PLAG, 0, 6}, {PLAG, 0, 5}, {PLAG, 0, 4}, {PLAG, 0, 3},
  {PLAG, 0, 2}, {PLAG, 0, 1}, {PLAG, 0, 0},
  {CBINDEX, 0, 6}, {CBINDEX, 0, 5}, {CBINDEX, 0, 4},
  {CBINDEX, 0, 3}, {CBINDEX, 0, 2}, {CBINDEX, 0, 1}, {CBINDEX, 0, 0},
  {CBGAIN, 0, 2}, {CBGAIN, 0, 1}, {CBGAIN, 0, 0},
  {CBINDEX, 1, 6}, {CBINDEX, 1, 5}, {CBINDEX, 1, 4},
  {CBINDEX, 1, 3}, {CBINDEX, 1, 2}, {CBINDEX, 1, 1}, {CBINDEX, 1, 0},
  {CBGAIN, 1, 2}, {CBGAIN, 1, 1}, {CBGAIN, 1, 0},
  {PGAIN, 1, 2}, {PGAIN, 1, 1}, {PGAIN, 1, 0},
  {PLAG, 1, 6}, {PLAG, 1, 5}, {PLAG, 1, 4}, {PLAG, 1, 3},
  {PLAG, 1, 2}, {PLAG, 1, 1}, {PLAG, 1, 0},
  {CBINDEX, 2, 6}, {CBINDEX, 2, 5}, {CBINDEX, 2, 4},
  {CBINDEX, 2, 3}, {CBINDEX, 2, 2}, {CBINDEX, 2, 1}, {CBINDEX, 2, 0},
  {CBGAIN, 2, 2}, {CBGAIN, 2, 1}, {CBGAIN, 2, 0},
  {CBINDEX, 3, 6}, {CBINDEX, 3, 5}, {CBINDEX, 3, 4},
  {CBINDEX, 3, 3}, {CBINDEX, 3, 2}, {CBINDEX, 3, 1}, {CBINDEX, 3, 0},
  {CBGAIN, 3, 2}, {CBGAIN, 3, 1}, {CBGAIN, 3, 0}};

static const INTTYPE BIT_DATA2[40][3] =
{ {LSP, 0, 0}, {LSP, 1, 0}, {LSP, 2, 0}, {LSP, 3, 0}, {LSP, 4, 0},
  {LSP, 5, 0}, {LSP, 6, 0}, {LSP, 7, 0}, {LSP, 8, 0}, {LSP, 9, 0},
  {PGAIN, 0, 2}, {PGAIN, 0, 1}, {PGAIN, 0, 0},
  {PLAG, 0, 6}, {PLAG, 0, 5}, {PLAG, 0, 4}, {PLAG, 0, 3},
  {PLAG, 0, 2}, {PLAG, 0, 1}, {PLAG, 0, 0},
  {CBINDEX, 0, 6}, {CBINDEX, 0, 5}, {CBINDEX, 0, 4},
  {CBINDEX, 0, 3}, {CBINDEX, 0, 2}, {CBINDEX, 0, 1}, {CBINDEX, 0, 0},
  {CBGAIN, 0, 2}, {CBGAIN, 0, 1}, {CBGAIN, 0, 0},
  {CBINDEX, 1, 6}, {CBINDEX, 1, 5}, {CBINDEX, 1, 4},
  {CBINDEX, 1, 3}, {CBINDEX, 1, 2}, {CBINDEX, 1, 1}, {CBINDEX, 1, 0},
  {CBGAIN, 1, 2}, {CBGAIN, 1, 1}, {CBGAIN, 1, 0}};

static const INTTYPE BIT_DATA1[16][3] =
/* CBSEED bits actually correspond to the bits chosen from SD */
{ {CBSEED, 0, 15}, {LSP, 0, 0}, {LSP, 1, 0}, {LSP, 2, 0},
  {CBSEED, 0, 11}, {LSP, 3, 0}, {LSP, 4, 0}, {LSP, 5, 0},
  {CBSEED, 0, 7}, {LSP, 6, 0}, {LSP, 7, 0}, {LSP, 8, 0},
  {CBSEED, 0, 3}, {LSP, 9, 0}, {CBGAIN, 0, 1}, {CBGAIN, 0, 0}};
