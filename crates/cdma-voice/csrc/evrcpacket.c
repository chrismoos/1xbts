#include "evrcpacket.h"
#include <string.h>

#ifndef MEMCPY
#define MEMCPY	memcpy
#endif

#ifndef MEMSET
#define MEMSET	memset
#endif


#define EVRC8K_FRAMES_HEADER_SIZE(frame_count)	(((frame_count)+3)/4)
#define EVRC8K_PACKET_HEADER_SIZE(frame_count)	(EVRC8K_FRAMES_HEADER_SIZE(frame_count) + 1)

#define EVRC8K_HEADER_BYTE_OFFSET(frame_index)	( (frame_index) / 4 )
#define EVRC8K_HEADER_BITS_OFFSET(frame_index)	( 6 - (2 * ((frame_index) & 0x3)) )

#define EVRC8K_HEADER_BITS_MARK(frame_index,rate)	( ((rate) & 0x3) << EVRC8K_HEADER_BITS_OFFSET(frame_index))

size_t evrc8k_frame_rate_bytes(uint8_t rate) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };
	rate &= 0x3;
	if( rate < 4 ) {
		return frame_size[ rate ];
	} else {
		return 0;
	}
}

size_t evrc8k_packet_header_size(size_t frame_count) {
	if( frame_count > EVRC8K_MAX_PACKET_FRAMES )
		return 0;
	return 1 + ((frame_count + 3) / 4);
}

size_t evrc8k_packet_packet(uint8_t* packet,const Evrc8KFrame* frames,size_t frame_count) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };
	size_t i = 0;
	uint8_t* ptr_frames_header = NULL;
	uint8_t* ptr_data = NULL;
	size_t data_size = 0;

	if( NULL == packet || NULL == frames || frame_count == 0 || frame_count > EVRC8K_MAX_PACKET_FRAMES )
		return 0;

	*packet = (uint8_t)( frame_count & 0x000000FF );

	ptr_frames_header = packet + 1;
	ptr_data = ptr_frames_header + EVRC8K_FRAMES_HEADER_SIZE(frame_count);

	MEMSET(ptr_frames_header,0,EVRC8K_PACKET_HEADER_SIZE(frame_count));

	for(i = 0; i < frame_count; ++i) {
		ptr_frames_header[ EVRC8K_HEADER_BYTE_OFFSET(i) ] |= EVRC8K_HEADER_BITS_MARK(i,frames[i].rate);
		if( frames[i].rate > 3 )
			return 0;
		MEMCPY(ptr_data + data_size,frames[i].bits,frame_size[ frames[i].rate ] );
		data_size += frame_size[ frames[i].rate ];
	}

	return EVRC8K_PACKET_HEADER_SIZE(frame_count) + data_size;
}

size_t evrc8k_packet_unpacket(Evrc8KFrame* frames,size_t max_frame_count,const uint8_t* packet,size_t packet_size) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };
	size_t frame_count = 0;
	size_t i = 0;
	const uint8_t* ptr_frames_header = NULL;
	const uint8_t* ptr_data = NULL;
	size_t data_size = 0;

	if( NULL == frames || max_frame_count == 0 || NULL == packet || packet_size < 4 )
		return 0;

	frame_count = *packet;
	ptr_frames_header = packet + 1;
	ptr_data = ptr_frames_header + EVRC8K_FRAMES_HEADER_SIZE(frame_count);

	if( frame_count > max_frame_count || ptr_data > packet + packet_size )
		return 0;

	for(i = 0; i < frame_count; ++i) {
		frames[i].rate = (ptr_frames_header[ EVRC8K_HEADER_BYTE_OFFSET(i) ] >> EVRC8K_HEADER_BITS_OFFSET(i) ) & 0x3;

		if( EVRC8K_PACKET_HEADER_SIZE(frame_count) + data_size + frame_size[ frames[i].rate ] > packet_size )
			return 0;

		MEMCPY(frames[i].bits,ptr_data + data_size,frame_size[ frames[i].rate ] );
		data_size += frame_size[ frames[i].rate ];
	}

	return EVRC8K_PACKET_HEADER_SIZE(frame_count) + data_size;
}

size_t evrc8k_frames_of_packet(const uint8_t* packet,size_t size) {
	size_t frame_count = *packet;
	if( EVRC8K_PACKET_HEADER_SIZE(frame_count) + frame_count * EVRC8K_EIGHT_SIZE <= size )
		return frame_count;
	else
		return 0;
}


int evrc8k_packet_init_parser(const uint8_t* packet,size_t packet_size,Evrc8KPacketParser* parser) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };
	if( NULL == parser || NULL == packet || packet_size < 4 )
		return 0;

	parser->packet_size = packet_size;
	parser->packet = packet;
	parser->frame_count = *packet;
	parser->hptr = packet + 1;
	parser->dptr = parser->hptr + EVRC8K_FRAMES_HEADER_SIZE(parser->frame_count);

	parser->pos = 0;
	parser->rate = (parser->hptr[ EVRC8K_HEADER_BYTE_OFFSET(0) ] >> EVRC8K_HEADER_BITS_OFFSET(0) ) & 0x3;
	parser->frame = parser->dptr;
	parser->frame_size = frame_size[ parser->rate ];
	return 1;
}

int evrc8k_packet_next_frame(Evrc8KPacketParser* parser) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };

	if( NULL == parser )
		return 0;

	if( parser->pos + 1 >= parser->frame_count )
		return 0;
	if( (parser->frame + parser->frame_size) >= parser->packet + parser->packet_size )
		return 0;

	parser->pos++;
	parser->rate = (parser->hptr[ EVRC8K_HEADER_BYTE_OFFSET(parser->pos) ] >> EVRC8K_HEADER_BITS_OFFSET(parser->pos) ) & 0x3;
	parser->frame += parser->frame_size;
	parser->frame_size = frame_size[ parser->rate ];
	return 1;
}

int evrc8k_packet_init_appender(uint8_t* buffer,size_t buffer_size,size_t frame_count,Evrc8KPacketAppender* appender) {
	
	if( NULL == buffer || buffer_size < 4 || frame_count == 0 || frame_count > 255 || NULL == appender )
		return 0;

	appender->buffer = buffer;
	*buffer = (uint8_t) (frame_count & 0xFF);

	appender->buffer_size = buffer_size;
	appender->frame_count = frame_count;
	appender->hptr = buffer + 1;
	appender->dptr = appender->hptr + EVRC8K_FRAMES_HEADER_SIZE(appender->frame_count);
	appender->pos = 0;
	appender->packet_size = EVRC8K_PACKET_HEADER_SIZE(appender->frame_count);
	MEMSET(appender->hptr,0,EVRC8K_FRAMES_HEADER_SIZE(appender->frame_count));
	return 1;
}

int evrc8k_packet_append_frame_raw(Evrc8KPacketAppender* appender,uint8_t rate,const uint8_t* frame) {
	const size_t frame_size[] = { EVRC8K_EIGHT_SIZE,EVRC8K_CUSTOM_SIZE,EVRC8K_HALF_SIZE,EVRC8K_FULL_SIZE };

	if( NULL == appender || NULL == frame || rate > 3 )
		return 0;


	if( appender->pos + 1 > appender->frame_count || appender->dptr + frame_size[ rate ] >= appender->buffer + appender->buffer_size )
		return 0;

	appender->hptr[ EVRC8K_HEADER_BYTE_OFFSET(appender->pos) ] |= EVRC8K_HEADER_BITS_MARK(appender->pos,rate);
	MEMCPY(appender->dptr,frame,frame_size[ rate ] );
	appender->dptr += frame_size[ rate ];
	appender->packet_size += frame_size[ rate ];
	appender->pos++;
	
	return 1;
}


int evrc8k_packet_append_frame(Evrc8KPacketAppender* appender,const Evrc8KFrame* frame) {
	if( NULL == appender || NULL == frame || frame->rate > 3 )
		return 0;
	return evrc8k_packet_append_frame_raw(appender,frame->rate,frame->bits);
}

