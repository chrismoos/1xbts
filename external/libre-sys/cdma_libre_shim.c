#include <stdint.h>
#include <errno.h>
#include <string.h>

#include <re.h>

#include "cdma_libre_shim.h"

static int provide_desc(struct cdma_libre_sipsess_ctx *ctx, struct mbuf **descp)
{
	if (!ctx || !ctx->handlers || !ctx->handlers->desc)
		return EINVAL;

	return ctx->handlers->desc(ctx->arg, descp);
}

#ifdef CDMA_LIBRE_SIPSESS_CONNECT_HAS_DESC_HANDLER
static int desc_handler(struct mbuf **descp, const struct sa *src,
			const struct sa *dst, void *arg)
{
	(void)src;
	(void)dst;

	return provide_desc(arg, descp);
}
#else
static int offer_handler(struct mbuf **descp, const struct sip_msg *msg,
			 void *arg)
{
	(void)msg;

	return provide_desc(arg, descp);
}
#endif

static int auth_handler(char **username, char **password, const char *realm,
			void *arg)
{
	struct cdma_libre_sipsess_ctx *ctx = arg;

	if (!ctx || !ctx->handlers || !ctx->handlers->auth)
		return EINVAL;

	return ctx->handlers->auth(ctx->arg, realm, username, password);
}

static void trace_handler(bool tx, enum sip_transp tp, const struct sa *src,
			  const struct sa *dst, const uint8_t *pkt, size_t len,
			  void *arg)
{
	struct cdma_libre_sip_ctx *ctx = arg;

	if (ctx && ctx->trace)
		ctx->trace(ctx->arg, tx ? 1 : 0, tp, src, dst, pkt, len);
}

#ifdef CDMA_LIBRE_HAS_SIPREG
static int reg_auth_handler(char **username, char **password,
			    const char *realm, void *arg)
{
	struct cdma_libre_sipreg_ctx *ctx = arg;

	if (!ctx || !ctx->handlers || !ctx->handlers->auth)
		return EINVAL;

	return ctx->handlers->auth(ctx->arg, realm, username, password);
}

static void reg_response_handler(int err, const struct sip_msg *msg, void *arg)
{
	struct cdma_libre_sipreg_ctx *ctx = arg;
	const uint8_t *reason = NULL;
	size_t reason_len = 0;
	uint16_t scode = 0;

	if (msg) {
		scode = msg->scode;
		reason = (const uint8_t *)msg->reason.p;
		reason_len = msg->reason.l;
	}

	if (ctx && ctx->handlers && ctx->handlers->response)
		ctx->handlers->response(ctx->arg, err, scode, reason,
					reason_len);

	if (err == 0 && msg && scode >= 200 && scode < 300 && ctx &&
	    ctx->sip && ctx->keepalive_interval_secs > 0 && !ctx->keepalive) {
		(void)sip_keepalive_start(&ctx->keepalive, ctx->sip, msg,
					  ctx->keepalive_interval_secs,
					  NULL, NULL);
	}
}
#endif

static int answer_handler(const struct sip_msg *msg, void *arg)
{
	struct cdma_libre_sipsess_ctx *ctx = arg;
	const uint8_t *body = NULL;
	size_t body_len = 0;
	uint16_t scode = 0;

	if (msg) {
		scode = msg->scode;
		if (msg->mb) {
			body = mbuf_buf(msg->mb);
			body_len = mbuf_get_left(msg->mb);
		}
	}

	if (ctx && ctx->handlers && ctx->handlers->answer)
		ctx->handlers->answer(ctx->arg, scode, body, body_len);

	return 0;
}

static void progress_handler(const struct sip_msg *msg, void *arg)
{
	struct cdma_libre_sipsess_ctx *ctx = arg;
	const uint8_t *body = NULL;
	size_t body_len = 0;
	uint16_t scode = msg ? msg->scode : 0;

	if (msg && msg->mb) {
		body = mbuf_buf(msg->mb);
		body_len = mbuf_get_left(msg->mb);
	}

	if (ctx && ctx->handlers && ctx->handlers->progress)
		ctx->handlers->progress(ctx->arg, scode, body, body_len);
}

static void established_handler(const struct sip_msg *msg, void *arg)
{
	struct cdma_libre_sipsess_ctx *ctx = arg;
	uint16_t scode = msg ? msg->scode : 0;

	if (ctx && ctx->handlers && ctx->handlers->established)
		ctx->handlers->established(ctx->arg, scode);
}

static void close_handler(int err, const struct sip_msg *msg, void *arg)
{
	struct cdma_libre_sipsess_ctx *ctx = arg;
	uint16_t scode = msg ? msg->scode : 0;

	if (ctx && ctx->handlers && ctx->handlers->close)
		ctx->handlers->close(ctx->arg, err, scode);
}

int cdma_libre_sip_alloc(struct sip **sipp, const char *software,
			 struct cdma_libre_sip_ctx *ctx)
{
	struct dnsc_conf dns_conf = {
		.query_hash_size = 16,
		.tcp_hash_size = 2,
		.conn_timeout = 10000,
		.idle_timeout = 30000,
#ifdef CDMA_LIBRE_DNSC_HAS_CACHE_TTL_MAX
		.cache_ttl_max = 1800,
#endif
#ifdef CDMA_LIBRE_DNSC_HAS_GETADDRINFO
		.getaddrinfo = true,
#endif
	};
	struct dnsc *dnsc = NULL;
	int err;

	err = dnsc_alloc(&dnsc, &dns_conf, NULL, 0);
	if (err)
		return err;

	err = sip_alloc(sipp, dnsc, 32, 32, 32, software, NULL, ctx);
	mem_deref(dnsc);

	if (err)
		return err;

	if (ctx && ctx->trace)
		sip_set_trace_handler(*sipp, trace_handler);

	return 0;
}

int cdma_libre_sip_transp_add(struct sip *sip, enum sip_transp tp,
			      const struct sa *laddr)
{
	return sip_transp_add(sip, tp, laddr);
}

void cdma_libre_sip_close(struct sip *sip, uint8_t force)
{
	sip_close(sip, force != 0);
}

void cdma_libre_sip_deref(struct sip *sip)
{
	mem_deref(sip);
}

int cdma_libre_sipreg_alloc(struct sipreg **regp, struct sip *sip,
			    const struct cdma_libre_registration *registration)
{
#ifdef CDMA_LIBRE_HAS_SIPREG
	sip_auth_h *authh;

	if (!registration || !registration->registrar_uri ||
	    !registration->to_uri || !registration->from_uri ||
	    !registration->contact_user || !registration->ctx)
		return EINVAL;

	registration->ctx->sip = sip;
	registration->ctx->keepalive = NULL;
	registration->ctx->keepalive_interval_secs =
		registration->keepalive_interval_secs;

	authh = registration->auth_enabled && registration->ctx->handlers &&
			registration->ctx->handlers->auth ?
			reg_auth_handler :
			NULL;

	return sipreg_alloc(regp, sip, registration->registrar_uri,
			    registration->to_uri, registration->from_name,
			    registration->from_uri, registration->expires,
			    registration->contact_user, NULL, 0, 0, authh,
			    registration->ctx, false, reg_response_handler,
			    registration->ctx, NULL, NULL);
#else
	(void)regp;
	(void)sip;
	(void)registration;
	return ENOSYS;
#endif
}

int cdma_libre_sipreg_send(struct sipreg *reg)
{
#ifdef CDMA_LIBRE_HAS_SIPREG
	return sipreg_send(reg);
#else
	(void)reg;
	return ENOSYS;
#endif
}

void cdma_libre_sipreg_keepalive_stop(struct cdma_libre_sipreg_ctx *ctx)
{
	if (ctx && ctx->keepalive) {
		mem_deref(ctx->keepalive);
		ctx->keepalive = NULL;
	}
}

void cdma_libre_sipreg_deref(struct sipreg *reg)
{
	mem_deref(reg);
}

int cdma_libre_sipsess_listen(struct sipsess_sock **sockp, struct sip *sip)
{
	return sipsess_listen(sockp, sip, 32, NULL, NULL);
}

void cdma_libre_sipsess_close_all(struct sipsess_sock *sock)
{
	sipsess_close_all(sock);
}

void cdma_libre_sipsess_sock_deref(struct sipsess_sock *sock)
{
	mem_deref(sock);
}

int cdma_libre_sipsess_connect(struct sipsess **sessp,
			       struct sipsess_sock *sock,
			       const struct cdma_libre_outbound_call *call)
{
	sip_auth_h *authh;

	if (!call || !call->to_uri || !call->from_uri || !call->contact_user ||
	    !call->ctx)
		return EINVAL;

	authh = call->auth_enabled && call->ctx->handlers &&
			call->ctx->handlers->auth ?
			auth_handler :
			NULL;

#ifdef CDMA_LIBRE_SIPSESS_CONNECT_HAS_DESC_HANDLER
	return sipsess_connect(sessp, sock, call->to_uri, call->from_name,
			       call->from_uri, call->contact_user, NULL, 0,
			       "application/sdp", authh, call->ctx, false,
#ifdef CDMA_LIBRE_SIPSESS_CONNECT_HAS_CALL_ID
			       call->call_id, desc_handler, NULL,
#else
			       desc_handler, NULL,
#endif
			       answer_handler, progress_handler,
#ifdef CDMA_LIBRE_HAS_SIPSESS_ESTAB_H
			       established_handler,
#endif
			       NULL, NULL, close_handler, call->ctx, NULL);
#else
	struct mbuf *desc = NULL;
	int err = provide_desc(call->ctx, &desc);
	if (err)
		return err;

	return sipsess_connect(sessp, sock, call->to_uri, call->from_name,
			       call->from_uri, call->contact_user, NULL, 0,
			       "application/sdp", desc, authh, call->ctx, false,
			       offer_handler, answer_handler, progress_handler,
#ifdef CDMA_LIBRE_HAS_SIPSESS_ESTAB_H
			       established_handler,
#endif
			       NULL, NULL, close_handler, call->ctx, NULL);
#endif
}

void cdma_libre_sipsess_abort(struct sipsess *sess)
{
#ifdef CDMA_LIBRE_HAS_SIPSESS_ABORT
	sipsess_abort(sess);
#else
	(void)sess;
#endif
}

void cdma_libre_sipsess_deref(struct sipsess *sess)
{
	mem_deref(sess);
}

struct mbuf *cdma_libre_mbuf_from_str(const char *value)
{
	struct mbuf *mb;
	int err;

	if (!value)
		return NULL;

	mb = mbuf_alloc(strlen(value) + 1);
	if (!mb)
		return NULL;

	err = mbuf_write_str(mb, value);
	if (err) {
		mem_deref(mb);
		return NULL;
	}

	mb->pos = 0;
	return mb;
}

void cdma_libre_mbuf_rewind(struct mbuf *mb)
{
	if (mb)
		mb->pos = 0;
}

int cdma_libre_strdup(char **dst, const char *src)
{
	return str_dup(dst, src);
}
