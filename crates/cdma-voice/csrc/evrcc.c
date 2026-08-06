#include "evrcc.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include "globs.h"

#include "typedefs.h"
#include "dsp_math.h"
#include "macro.h"
#include "bqiir.h"
#include "decode.h"
#include "encode.h"
#include "ns127.h"
#include "rda.h"
#include "evrc_state.h"

#include "evrcpacket.h"

#define  SPEECH_FRAME_SAMPLES       160
#define  LOOKAHEAD_LEN              80
#define  BITSTREAM_FRAME_MAX_BYTES		24	/*  data + 1 word for rate  */
#define BITSTREAM_FRAME_MIN_BYTES	4

#define SWAP_16(x) \
((uint16_t)( \
	(((uint16_t)(x) & (uint16_t)0x00ffU) << 8) | \
	(((uint16_t)(x) & (uint16_t)0xff00U) >> 8) ))

typedef struct EvrcEncoderContext {
	EvrcNativeContext native;
	Shortword beta;
	Longword R[17];
	int16_t min_rate;
	int16_t max_rate;
	int16_t noise_suppression;
} EvrcEncoderContext;

typedef struct EvrcDecoderContext {
	EvrcNativeContext native;
} EvrcDecoderContext;


void* evrc_encoder_init(int16_t min_rate,int16_t max_rate,int16_t noise_suppression) {
	EvrcEncoderContext* context = (EvrcEncoderContext*)malloc(sizeof(EvrcEncoderContext));
	if (context == NULL) {
		return NULL;
	}
	memset(context,0,sizeof(EvrcEncoderContext));
	evrc_state_init(&context->native);
	context->min_rate = min_rate;
	context->max_rate = max_rate;
	context->noise_suppression = noise_suppression;

	EvrcNativeContext *previous = evrc_set_current_context(&context->native);
	InitEncoder();
	evrc_set_current_context(previous);
	return context;
}

void evrc_encoder_uninit(void* context) {
	if( context ) {
		free(context);
	}
}

const static size_t FRAME_SIZE[] = { 0,2+2,0,10+2,22+2,0,0,0,0,0,0,0,0,0,0,0,0 };
const static size_t FRAME_DATA_SIZE[] = { 0,2,0,10,22,0,0,0,0,0,0,0,0,0,0,0,0 };
const static size_t FRAME_DATA_WORDS[] = { 0,1,0,5,11,0,0,0,0,0,0,0,0,0,0,0,0 };

int evrc_encoder_max_encode(void* c,size_t speech_samples) {
	EvrcEncoderContext* context = (EvrcEncoderContext*)c;
	if( context == NULL ) return 0;
	if( FRAME_SIZE[ context->max_rate ] == 0 ) return 0;
	return (speech_samples / SPEECH_FRAME_SAMPLES ) * FRAME_SIZE[ context->max_rate ];
}

static int evrc_encoder_encode_frame(EvrcEncoderContext* context,int16_t* speech,uint8_t* bits) {
	int k;
	uint8_t rate = 0;
	int16_t* bits_16 = NULL;
	int16_t bits_frame[22];

	if( speech == 0 || bits == 0 )
		return 0;

	EvrcNativeContext *previous = evrc_set_current_context(&context->native);
	bqiir(speech);
	if( context->noise_suppression ) {
		noise_suprs(speech);
		noise_suprs(speech + SPEECH_FRAME_SAMPLES/2);
	}
	else
	{
		for (k=0; k<SPEECH_FRAME_SAMPLES; k++)
		{
			speech[k] = shift_r(speech[k],-1);
		}
	}

	context->beta = pre_encode(speech, context->R);
	rate =  (uint8_t)select_rate(context->R,context->max_rate,context->min_rate,context->beta);
	*bits = rate;
	bits += 1;
	*bits = FRAME_DATA_SIZE[rate];
	bits += 1;
	
	bits_16 = (int16_t*)bits;
	
	encode(rate,(int16_t*)bits_frame);
	post_encode();
	for(k=0;k<FRAME_DATA_WORDS[rate];++k){
		bits_16[k] = SWAP_16(bits_frame[k]);
	}
	evrc_set_current_context(previous);
	return FRAME_SIZE[rate];
}

int evrc_encoder_encode_to_stream(void* c,int16_t* speech,size_t speech_samples,uint8_t* bits,size_t bits_max_bytes) {
	EvrcEncoderContext* context = (EvrcEncoderContext*)c;
	size_t encode_samples = 0;
	size_t output_bytes = 0;
	size_t bytes = 0;

	if( c == 0 || speech == 0 || speech_samples < SPEECH_FRAME_SAMPLES || bits == 0 || bits_max_bytes <= 0)
		return 0;
	while( encode_samples + SPEECH_FRAME_SAMPLES <= speech_samples && 
			output_bytes + BITSTREAM_FRAME_MAX_BYTES <= bits_max_bytes ){
		bytes = evrc_encoder_encode_frame(context,speech,bits);
		output_bytes += bytes;
		encode_samples += SPEECH_FRAME_SAMPLES;
		speech += SPEECH_FRAME_SAMPLES;
		bits += bytes;
	}
	return output_bytes;
}

void* evrc_decoder_init() {
	EvrcDecoderContext* context = (EvrcDecoderContext*)malloc(sizeof(EvrcDecoderContext));
	if (context == NULL) {
		return NULL;
	}
	memset(context,0,sizeof(EvrcDecoderContext));
	evrc_state_init(&context->native);
	EvrcNativeContext *previous = evrc_set_current_context(&context->native);
	InitDecoder();
	evrc_set_current_context(previous);
	return context;
}

void evrc_decoder_uninit(void* c) {
	if( c ) {
		free(c);
	}
}

int evrc_get_stream_frame_count(const uint8_t* bits,size_t bits_bytes) {
	const uint8_t* end = bits + bits_bytes;
	size_t frame = 0;
	size_t size = 0;

	if( bits == NULL || bits_bytes <= 0 )
		return 0;

	while( bits < end ) {
		if( (*bits) > 0x04 )
			return 0;
		size = FRAME_SIZE[ (*bits) & 0x0F ];
		if( size == 0 ) {
			return 0;
		}
		++frame;
		bits += size;
	}
	return frame;
}

int evrc_decoder_stream_max_sample(const uint8_t* bits,size_t bits_bytes) {
	return evrc_get_stream_frame_count(bits,bits_bytes) * SPEECH_FRAME_SAMPLES ;
}

static int evrc_decoder_decode_frame_raw(void* c, const uint8_t* bits,size_t bits_bytes,uint8_t rate,int16_t* speech) {
	EvrcDecoderContext* context = (EvrcDecoderContext*)c;
	const int16_t* bits_16 = (int16_t*)bits;
	size_t i = 0;
	int16_t bits_frame[11];

	if( context == NULL || rate > 0x4 || FRAME_DATA_SIZE[rate] == 0 || FRAME_DATA_SIZE[rate] > bits_bytes ) return 0;
	memset(bits_frame,0,sizeof(int16_t)*11);

	for(i=0;i< FRAME_DATA_WORDS[rate];++i) {
		bits_frame[i] = SWAP_16(bits_16[i]);
	}
	EvrcNativeContext *previous = evrc_set_current_context(&context->native);
	decode((int16_t*)bits_frame, rate, /*post_filter*/ 0,speech);
	evrc_set_current_context(previous);
	return FRAME_SIZE[rate];
}

int evrc_decoder_decode_stream_frame(void* c, const uint8_t* bits,size_t bits_bytes,int16_t* speech) {
	int16_t rate = bits[0];
	return evrc_decoder_decode_frame_raw(c,bits + sizeof(int16_t),bits_bytes - sizeof(int16_t),rate,speech);
}

int evrc_decoder_stream_frame_bytes(const uint8_t* bits) {
	return FRAME_SIZE[ (*bits) & 0xF ];
}

int evrc_decoder_is_silence_stream_frame(const uint8_t* bits) {
	return FRAME_DATA_SIZE[ (*bits) & 0xF ] <= 2;
}

#define PACKET_TO_EVRC_RATE(rate)	((rate) + 1)
#define EVRC_TO_PACKET_RATE(rate)	((rate) - 1)

int evrc_packet_to_stream(const uint8_t* packet,size_t packet_size,uint8_t* stream,size_t stream_size) {
	Evrc8KPacketParser parser;
	size_t writedbytes = 0;
	const uint8_t* stream_end = stream + stream_size;

	if( NULL == packet || packet_size == 0 || NULL == stream || stream_size == 0 )
		return 0;

	if( ! evrc8k_packet_init_parser(packet,packet_size,&parser) ) {
		return 0;
	}

	do {
		if( stream + parser.frame_size + 2 >= stream_end )
			return 0;

		stream[0] = PACKET_TO_EVRC_RATE(parser.rate);
		stream[1] = FRAME_DATA_SIZE[ PACKET_TO_EVRC_RATE(parser.rate) & 0xF ];
		memcpy(&(stream[2]),parser.frame,parser.frame_size);

		stream += (2 + parser.frame_size);
		writedbytes += (2 + parser.frame_size);
	} while( evrc8k_packet_next_frame(&parser) );

	return writedbytes;
}


int evrc_stream_to_packet(const uint8_t* stream,size_t frame_count,uint8_t* packet,size_t packet_size) {
	Evrc8KPacketAppender appender;
	uint8_t rate = 0;

	if( NULL == packet || packet_size == 0 || NULL == stream || frame_count == 0 )
		return 0;
	
	if( ! evrc8k_packet_init_appender(packet,packet_size,frame_count,&appender) ) {
		return 0;
	}

	do {
		rate = stream[0];
		if( ! evrc8k_packet_append_frame_raw(&appender,EVRC_TO_PACKET_RATE(rate),&(stream[2])) )
			return 0;
		stream += FRAME_SIZE[ rate & 0xF ];
		--frame_count;
	} while( frame_count );

	return appender.packet_size;
}

int evrc_encoder_encode_to_packet(void* c,int16_t* speech,size_t speech_samples,uint8_t* packet,size_t packet_max_bytes) {
	uint8_t evrc_frame[32];
	size_t encode_samples = 0;
	int bytes = 0;
	uint8_t rate = 0;
	Evrc8KPacketAppender appender;
	EvrcEncoderContext* context = (EvrcEncoderContext*)c;

	if( NULL == c || NULL == speech || NULL == packet )
		return EVRC_CODEC_ERROR;

	speech_samples = (speech_samples / SPEECH_FRAME_SAMPLES ) * SPEECH_FRAME_SAMPLES ;
	if( speech_samples == 0 )
		return 0;

	if( ! evrc8k_packet_init_appender(packet,packet_max_bytes,speech_samples / SPEECH_FRAME_SAMPLES,&appender) ) {
		return EVRC_CODEC_BUFFER_SMALL;
	}

	while( encode_samples + SPEECH_FRAME_SAMPLES <= speech_samples ) { 
		bytes = evrc_encoder_encode_frame(context,speech,evrc_frame);

		if( bytes <= 0 )
			return EVRC_CODEC_ERROR;

		encode_samples += SPEECH_FRAME_SAMPLES;
		speech += SPEECH_FRAME_SAMPLES;

		rate = evrc_frame[0];
		if( ! evrc8k_packet_append_frame_raw(&appender,EVRC_TO_PACKET_RATE(rate),&(evrc_frame[2])) )
			return EVRC_CODEC_ERROR;
	}

	return appender.packet_size;
}

int evrc_decoder_decode_from_stream(void* c,const uint8_t* stream,size_t stream_bytes,int16_t* speech,size_t speech_max_samples) {
	uint8_t rate = 0;
	size_t frame_size = 0;
	size_t frame_count = 0;
	const uint8_t* s_end = stream + stream_bytes;
	const int16_t* o_end = speech + speech_max_samples;

	if(  NULL == stream || stream_bytes == 0 || NULL == speech || speech_max_samples < SPEECH_FRAME_SAMPLES )
		return 0;

	while( stream + 4 <= s_end && speech + SPEECH_FRAME_SAMPLES <= o_end) {
		rate = *stream;
		frame_size = FRAME_DATA_SIZE[ rate & 0xF ];
		if( stream + 2 + frame_size > s_end )
			return EVRC_CODEC_ERROR;

		if( evrc_decoder_decode_frame_raw(c,stream + 2,frame_size,rate,speech) <= 0 )
			return EVRC_CODEC_ERROR;

		stream += (2 + frame_size);
		speech += SPEECH_FRAME_SAMPLES ;
		++frame_count;
	}

	return frame_count * 320;
}


int evrc_decoder_decode_from_packet(void* c,const uint8_t* packet,size_t packet_bytes,int16_t* speech,size_t speech_max_samples) {
	Evrc8KPacketParser parser;
	uint8_t rate = 0;
	size_t frame_size = 0;
	size_t writedbytes = 0;

	if( NULL == c || NULL == packet || packet_bytes == 0 || NULL == speech )
		return EVRC_CODEC_ERROR;

	if( ! evrc8k_packet_init_parser(packet,packet_bytes,&parser) ) {
		return 0;
	}

	if( speech_max_samples < parser.frame_count * SPEECH_FRAME_SAMPLES  )
		return EVRC_CODEC_BUFFER_SMALL;

	do {
		rate = PACKET_TO_EVRC_RATE(parser.rate);
		frame_size = FRAME_DATA_SIZE[ rate & 0xF ];

		if( evrc_decoder_decode_frame_raw(c,parser.frame,parser.frame_size,rate,speech) <= 0 )
            return EVRC_CODEC_ERROR;
		speech += SPEECH_FRAME_SAMPLES ;
	} while( evrc8k_packet_next_frame(&parser) );

	return parser.frame_count * SPEECH_FRAME_SAMPLES * sizeof(int16_t);
}

int evrc_decoder_decode_erasure(void* c,int16_t* speech,size_t speech_max_samples) {
	EvrcDecoderContext* context = (EvrcDecoderContext*)c;
	int16_t bits_frame[11];
	EvrcNativeContext *previous;

	if( context == NULL || speech == NULL || speech_max_samples < SPEECH_FRAME_SAMPLES )
		return EVRC_CODEC_ERROR;

	memset(bits_frame,0,sizeof(bits_frame));
	previous = evrc_set_current_context(&context->native);
	decode(bits_frame, 0x0e, /*post_filter*/ 0, speech);
	evrc_set_current_context(previous);
	return SPEECH_FRAME_SAMPLES * sizeof(int16_t);
}



/*
int evrc_decoder_decode(void* c,uint8_t* bits,size_t bits_bytes,int16_t* speech,size_t speech_max_words) {
	size_t decode_bytes = 0;
	size_t output_samples = 0;
	size_t bytes = 0;

	if( bits == 0 || bits_bytes < BITSTREAM_FRAME_MAX_BYTES || speech == 0 || speech_max_words < SPEECH_FRAME_SAMPLES )
		return 0;

	while( decode_bytes + BITSTREAM_FRAME_MIN_BYTES <= bits_bytes &&
			output_samples + SPEECH_FRAME_SAMPLES <= speech_max_words) {
		bytes = evrc_decoder_decode_frame(c, bits,bits_bytes - decode_bytes,speech);
		if( bytes <= 0 ) break;
		decode_bytes += bytes;
		output_samples += SPEECH_FRAME_SAMPLES;
		bits += bytes;
		speech += SPEECH_FRAME_SAMPLES;
	}
	return output_samples;
}
*/
