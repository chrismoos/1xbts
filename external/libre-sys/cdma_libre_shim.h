#pragma once

#include <stddef.h>
#include <stdint.h>

struct mbuf;
struct sa;
struct sip;
struct sip_keepalive;
struct sipreg;
struct sipsess;
struct sipsess_sock;

enum sip_transp;

typedef int (*cdma_libre_sipsess_desc_h)(void *arg, struct mbuf **descp);
typedef int (*cdma_libre_sip_auth_h)(void *arg, const char *realm,
				     char **username, char **password);
typedef void (*cdma_libre_sip_trace_h)(void *arg, uint8_t tx,
				       enum sip_transp tp, const struct sa *src,
				       const struct sa *dst, const uint8_t *pkt,
				       size_t len);
typedef void (*cdma_libre_sipreg_resp_h)(void *arg, int err, uint16_t scode,
					 const uint8_t *reason,
					 size_t reason_len);
typedef void (*cdma_libre_sipsess_answer_h)(void *arg, uint16_t scode,
					    const uint8_t *body,
					    size_t body_len);
typedef void (*cdma_libre_sipsess_progress_h)(void *arg, uint16_t scode,
					      const uint8_t *body,
					      size_t body_len);
typedef void (*cdma_libre_sipsess_established_h)(void *arg, uint16_t scode);
typedef void (*cdma_libre_sipsess_close_h)(void *arg, int err,
					   uint16_t scode);

struct cdma_libre_sipsess_handlers {
	cdma_libre_sipsess_desc_h desc;
	cdma_libre_sip_auth_h auth;
	cdma_libre_sipsess_answer_h answer;
	cdma_libre_sipsess_progress_h progress;
	cdma_libre_sipsess_established_h established;
	cdma_libre_sipsess_close_h close;
};

struct cdma_libre_sipsess_ctx {
	const struct cdma_libre_sipsess_handlers *handlers;
	void *arg;
};

struct cdma_libre_sipreg_handlers {
	cdma_libre_sip_auth_h auth;
	cdma_libre_sipreg_resp_h response;
};

struct cdma_libre_sipreg_ctx {
	const struct cdma_libre_sipreg_handlers *handlers;
	void *arg;
	struct sip *sip;
	struct sip_keepalive *keepalive;
	uint32_t keepalive_interval_secs;
};

struct cdma_libre_sip_ctx {
	cdma_libre_sip_trace_h trace;
	void *arg;
};

struct cdma_libre_outbound_call {
	const char *to_uri;
	const char *from_name;
	const char *from_uri;
	const char *contact_user;
	const char *call_id;
	uint8_t auth_enabled;
	struct cdma_libre_sipsess_ctx *ctx;
};

struct cdma_libre_registration {
	const char *registrar_uri;
	const char *to_uri;
	const char *from_name;
	const char *from_uri;
	const char *contact_user;
	uint32_t expires;
	uint32_t keepalive_interval_secs;
	uint8_t auth_enabled;
	struct cdma_libre_sipreg_ctx *ctx;
};

int cdma_libre_sip_alloc(struct sip **sipp, const char *software,
			 struct cdma_libre_sip_ctx *ctx);
int cdma_libre_sip_transp_add(struct sip *sip, enum sip_transp tp,
			      const struct sa *laddr);
void cdma_libre_sip_close(struct sip *sip, uint8_t force);
void cdma_libre_sip_deref(struct sip *sip);

int cdma_libre_sipreg_alloc(struct sipreg **regp, struct sip *sip,
			    const struct cdma_libre_registration *registration);
int cdma_libre_sipreg_send(struct sipreg *reg);
void cdma_libre_sipreg_keepalive_stop(struct cdma_libre_sipreg_ctx *ctx);
void cdma_libre_sipreg_deref(struct sipreg *reg);

int cdma_libre_sipsess_listen(struct sipsess_sock **sockp, struct sip *sip);
void cdma_libre_sipsess_close_all(struct sipsess_sock *sock);
void cdma_libre_sipsess_sock_deref(struct sipsess_sock *sock);
int cdma_libre_sipsess_connect(struct sipsess **sessp,
			       struct sipsess_sock *sock,
			       const struct cdma_libre_outbound_call *call);
void cdma_libre_sipsess_abort(struct sipsess *sess);
void cdma_libre_sipsess_deref(struct sipsess *sess);

struct mbuf *cdma_libre_mbuf_from_str(const char *value);
void cdma_libre_mbuf_rewind(struct mbuf *mb);
int cdma_libre_strdup(char **dst, const char *src);
