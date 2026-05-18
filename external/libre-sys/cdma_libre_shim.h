#pragma once

#include <stddef.h>
#include <stdint.h>

struct mbuf;
struct sa;
struct sip;
struct sip_keepalive;
struct sip_msg;
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

/* ---- Inbound SIP support ---- */

/* Notifies the Rust side that an INVITE arrived. The `msg` pointer is borrowed
 * from libre and is valid only for the duration of the callback unless the
 * Rust side bumps its refcount with cdma_libre_sip_msg_ref(). */
typedef void (*cdma_libre_inbound_invite_h)(void *arg,
					    const struct sip_msg *msg);

struct cdma_libre_inbound_sipsess_handlers {
	cdma_libre_inbound_invite_h invite;
};

struct cdma_libre_inbound_sipsess_ctx {
	const struct cdma_libre_inbound_sipsess_handlers *handlers;
	void *arg;
};

/* Replaces cdma_libre_sipsess_listen — additionally wires the inbound INVITE
 * callback. The ctx must outlive the returned socket. */
int cdma_libre_sipsess_listen_with_handler(
	struct sipsess_sock **sockp, struct sip *sip,
	struct cdma_libre_inbound_sipsess_ctx *ctx);

/* Sends a stateless final/provisional response to `msg` without creating a
 * session. Use for early rejections (404/488/503/etc.) and the unconditional
 * 100 Trying. */
int cdma_libre_sip_treply(struct sip *sip, const struct sip_msg *msg,
			  uint16_t scode, const char *reason);

/* Creates a session and replies with a 1xx provisional response. `desc` is
 * an optional SDP answer body delivered with the provisional (early media);
 * pass NULL to send the provisional with no SDP. libre takes ownership of
 * the mbuf. `sess_ctx` carries the post-accept handlers (close/established)
 * and must outlive the session. */
int cdma_libre_sipsess_accept(struct sipsess **sessp,
			      struct sipsess_sock *sock,
			      const struct sip_msg *msg, uint16_t scode,
			      const char *reason, const char *contact_user,
			      struct mbuf *desc,
			      struct cdma_libre_sipsess_ctx *sess_ctx);

/* Sends a 2xx final response with the supplied SDP answer body. */
int cdma_libre_sipsess_answer(struct sipsess *sess, uint16_t scode,
			      const char *reason, struct mbuf *answer);

/* Sends a 1xx provisional from an accepted session (typically 180 Ringing). */
int cdma_libre_sipsess_progress(struct sipsess *sess, uint16_t scode,
				const char *reason);

/* Sends a final 4xx/5xx/6xx rejection from an accepted-but-not-answered
 * session. Releases the session. */
int cdma_libre_sipsess_reject(struct sipsess *sess, uint16_t scode,
			      const char *reason);

/* Bump / drop the refcount on a sip_msg held across callback boundaries. */
const struct sip_msg *cdma_libre_sip_msg_ref(const struct sip_msg *msg);
void cdma_libre_sip_msg_deref(const struct sip_msg *msg);

/* Accessors that copy fields out of a sip_msg into caller-owned buffers.
 * Each returns the number of bytes written (excluding NUL) or -1 if the
 * destination is too small. The buffer is always NUL-terminated on success. */
int cdma_libre_sip_msg_ruri_user(const struct sip_msg *msg, char *out,
				 size_t out_len);
int cdma_libre_sip_msg_from_user(const struct sip_msg *msg, char *out,
				 size_t out_len);
int cdma_libre_sip_msg_from_display(const struct sip_msg *msg, char *out,
				    size_t out_len);
int cdma_libre_sip_msg_body(const struct sip_msg *msg, uint8_t *out,
			    size_t out_len);

struct mbuf *cdma_libre_mbuf_from_str(const char *value);
void cdma_libre_mbuf_rewind(struct mbuf *mb);
int cdma_libre_strdup(char **dst, const char *src);
