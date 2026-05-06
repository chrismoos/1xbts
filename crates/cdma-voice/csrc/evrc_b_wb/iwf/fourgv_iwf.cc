/*======================================================================*/
/*  4GV - Fourth Generation Vocoder Speech Service Option for           */
/*  Wideband Spread Spectrum Digital System                             */
/*  C Source Code Simulation                                            */
/*                                                                      */
/*  Copyright (C) 2004-05 Qualcomm Incorporated. All rights             */
/*  reserved.                                                           */
/*----------------------------------------------------------------------*/

#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#ifdef __SUNOS4__
#else
#include <getopt.h>
#endif
extern void Bitpack(short in, unsigned short *TrWords, short NoOfBits, short *ptr);
extern void BitUnpack(short *out, unsigned short *RecWords, short NoOfBits, short *ptr);


#define WORDS 12
char prog_opts[]="s:i:o:h";
extern char *optarg;
extern int optind;
int main(int argc,char *argv[])
{
  int i, n, j=0;
  char infile[200],outfile[200],sigfile[200];
  FILE *fin,*fout,*fsig;
  unsigned short packet[WORDS];
  int HW=5;
  unsigned char SIG=0;
  unsigned long FC=0,DIMC=0,SIGC=0;
  int required_arg=0;

  /* unpacking variables */
  short delay_idx,mode_bit;
  short lsp_idx[4],acbg_idx[3],dmp[2];
  short PktPtr[2]={16,0};
  short f_rot_idx, a_amp_idx[3],a_power_idx;
  
  
  fprintf(stderr,"|******************************************************************************|\n");
  fprintf(stderr,"| 4GV - Fourth Generation Vocoder Packet Level Signaling InterWorking Function |\n");
  fprintf(stderr,"|       Version 1.0                                                            |\n");
  fprintf(stderr,"|       Converts 4GV full-rate packets to half-rate packets                    |\n");
  fprintf(stderr,"|******************************************************************************|\n");

  while((i=getopt(argc,argv,prog_opts))!=EOF) switch (i) {
  case 's':
    if ((fsig=fopen(argv[optind-1],"rb"))==NULL) {
      fprintf(stderr,"Cannot open signalling pattern file %s\n",argv[optind-1]);
      exit(1);
    }
    required_arg++;
    break;
  case 'i':
    if ((fin=fopen(argv[optind-1],"rb"))==NULL) {
      fprintf(stderr,"Cannot open input file %s\n",argv[optind-1]);
      exit(1);
    }
    required_arg++;
    break;
  case 'o':
    if ((fout=fopen(argv[optind-1],"wb"))==NULL) {
      fprintf(stderr,"Cannot open output file %s\n",argv[optind-1]);
      exit(1);
    }
    required_arg++;
    break;
  case 'h':
    fprintf(stderr,"usage: %s -s Signalling_Pattern_File [-h] input_packet_file output_packet_file\n",argv[0]);
    return(1);
  }
  
  if (required_arg!=3) {
    fprintf(stderr,"Wrong number of arguments\n");
    fprintf(stderr,"usage: %s -s Signalling_Pattern_File [-h] input_packet_file output_packet_file\n",argv[0]);
    exit(-1);
  }
  
  while (!feof(fin)) {
    if ((n=fread(packet,sizeof(unsigned short),WORDS,fin))<WORDS) {
      if (n) fprintf(stderr,"Incomplete packet read for frame number %d\n",j);
      break;
    }
    if (packet[0]==4) FC++; //increment full-rate count

    if (1) {
     if (fread(&SIG,sizeof(unsigned char),1,fsig)<1) {
	fprintf(stderr,"Signalling pattern string shorter than input\n");
	break;
      }
      
      if (SIG==1) {
	if (packet[0]==4) {// Only affects full rate packets, others are <=half
	  PktPtr[0]=16;
	  PktPtr[1]=0;
           
	       // unpacking F-CELP information
	       BitUnpack(&delay_idx ,packet+1,7,PktPtr); 
	       BitUnpack(lsp_idx ,packet+1,6,PktPtr);
	       BitUnpack(lsp_idx+1 ,packet+1,6,PktPtr);
	       BitUnpack(lsp_idx+2 ,packet+1,9,PktPtr);
	       BitUnpack(lsp_idx+3 ,packet+1,7,PktPtr);
	
	       
	       for(i=0;i<3;i++) BitUnpack(acbg_idx+i ,packet+1,3,PktPtr);
	       //printf("\n %d %d %d %d %d %d %d %d\n",delay_idx,lsp_idx[0],lsp_idx[1],lsp_idx[2],lsp_idx[3],acbg_idx[0],acbg_idx[1],acbg_idx[2]);
	         
	       //Packing spl. 1/2 CELP packet
	       PktPtr[0]=16;PktPtr[1]=0;
	       for (i=0; i<WORDS; i++) {
		 packet[i]=0;
	       }
            
	       Bitpack(0x7B ,packet+1,7,PktPtr); //123 in dec, spl_hcelp packet identifier for WB
                  
	       Bitpack(lsp_idx[0] ,packet+1,6,PktPtr);
	       Bitpack(lsp_idx[1] ,packet+1,6,PktPtr);
	       Bitpack(lsp_idx[2] ,packet+1,9,PktPtr);
	       Bitpack(lsp_idx[3] ,packet+1,7,PktPtr);
	   
	       for(i=0;i<3;i++) Bitpack(acbg_idx[i] ,packet+1,3,PktPtr);
	       Bitpack(delay_idx ,packet+1,7,PktPtr);
	       
	
	
	   for (packet[0]=3,i=HW+1;i<WORDS;i++) packet[i]=0;
	  DIMC++;          // Increment count of dimmed frames
	}
	SIGC++;            // Increment signalling count
      }
  }
    fwrite(packet,sizeof(unsigned short),WORDS,fout);
    j++;
    
  }


  fprintf(stdout,"Dimmed Frames: %d out of %d full-rate frames (%.2f%%)\n",DIMC,FC,DIMC*100.0/FC);
     
  fclose(fin);fclose(fout);

  
  return(0);
      
}
