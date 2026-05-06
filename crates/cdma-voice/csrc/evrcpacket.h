#ifndef _EVRC_PACKET_H_
#define _EVRC_PACKET_H_

//#ifdef _MSC_VER
#include <stddef.h>
#if defined _WIN32
#include "stdint_win32.h"
#else
#include <stdint.h>
#endif
//#else
//#include "ptt-config.h"
//#endif

#ifdef __cplusplus
extern "C" {
#endif

typedef enum {
	EVRC8K_RATE_EIGHT			= 0,
	EVRC8K_RATE_CUSTOM			= 1,
	EVRC8K_RATE_HALF			= 2,
	EVRC8K_RATE_FULL			= 3
} EVRC8K_RATE;

#define CUSTOM_KEY_SILENCE		1

#define EVRC8K_EIGHT_SIZE		2
#define EVRC8K_CUSTOM_SIZE		4	
#define EVRC8K_HALF_SIZE		10
#define EVRC8K_FULL_SIZE		22
#define EVRC8K_MAX_FRAME_SIZE	22


typedef struct _Evrc8KFrame {
	uint8_t			rate;
	uint8_t			bits[EVRC8K_MAX_FRAME_SIZE];
} Evrc8KFrame;


#define EVRC8K_MAX_PACKET_FRAMES		0xFF

size_t evrc8k_frame_rate_bytes(uint8_t rate);

size_t evrc8k_packet_packet(uint8_t* packet,const Evrc8KFrame* frames,size_t frame_count);
size_t evrc8k_packet_unpacket(Evrc8KFrame* frames,size_t max_frame_count,const uint8_t* packet,size_t packet_size);

size_t evrc8k_frames_of_packet(const uint8_t* packet,size_t size);


typedef struct {
	size_t				packet_size;
	size_t				frame_count;
	const uint8_t*		packet;
	const uint8_t*		hptr;
	const uint8_t*		dptr;

	size_t				pos;
	uint8_t				rate;
	size_t				frame_size;
	const uint8_t*		frame;
} Evrc8KPacketParser;

int evrc8k_packet_init_parser(const uint8_t* packet,size_t packet_size,Evrc8KPacketParser* parser);
int evrc8k_packet_next_frame(Evrc8KPacketParser* parser);

typedef struct {
	size_t				buffer_size;
	size_t				frame_count;
	uint8_t*			buffer;
	uint8_t*			hptr;
	uint8_t*			dptr;

	size_t				packet_size;
	size_t				pos;
} Evrc8KPacketAppender;

int evrc8k_packet_init_appender(uint8_t* buffer,size_t buffer_size,size_t frame_count,Evrc8KPacketAppender* appender);
int evrc8k_packet_append_frame(Evrc8KPacketAppender* appender,const Evrc8KFrame* frame);
int evrc8k_packet_append_frame_raw(Evrc8KPacketAppender* appender,uint8_t rate,const uint8_t* frame);


#ifdef __cplusplus
}
#endif

#endif
