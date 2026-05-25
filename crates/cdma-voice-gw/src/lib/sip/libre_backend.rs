use parking_lot::Mutex;
use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr, TcpListener, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use cdma_voice::VoiceCodec;
use tokio::runtime::Handle;
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;
use tokio::time::sleep;
use uuid::Uuid;

use crate::config::{CallConfig, LoggingConfig, NatMode, VoiceGatewayConfig};
use crate::media::{G711Codec, RtpMediaSession};
use crate::proto::{AirVoiceFrame, GatewayVoiceFrame, ReleaseReason};

use super::{
    OutboundSipCall, Result, SipBackend, SipBackendError, SipBackendEvent, SipBackendEventSender,
};

pub struct LibreSipBackend {
    config: VoiceGatewayConfig,
    sessions: Mutex<HashMap<String, ActiveSession>>,
    inbound_sessions: Arc<Mutex<HashMap<String, InboundSessionState>>>,
    rtp_ports: Arc<Mutex<RtpPortAllocator>>,
    sip_auth: Option<libre::SipCredentials>,
    gateway_voice_tx: broadcast::Sender<GatewayVoiceFrame>,
    _registration: Option<libre::SipRegistration>,
    session_socket: Arc<libre::SipSessionSocket>,
    _sip_stack: Arc<libre::SipStack>,
    _event_loop: libre::EventLoop,
    inbound_events: Arc<Mutex<Option<SipBackendEventSender>>>,
    _inbound_listener: libre::InboundListener,
}

struct InboundSessionState {
    session: libre::InboundSipSession,
    _rtp_lease: RtpPortLease,
    media: Arc<RtpMediaSession>,
    sdp_answer: String,
    chosen_codec: G711Codec,
    /// CAS-claim ("finalized") — true once any final disposition has been
    /// sent (200, reject, 408 watchdog, or MSC-initiated release). Prevents
    /// races between concurrent paths and tells `close_h` not to emit
    /// `InboundCancel` for clears we drove ourselves.
    answered: Arc<AtomicBool>,
    /// True once a 2xx final response (200 OK) was sent and the dialog is
    /// confirmed. `close_h` reads this to distinguish a post-answer trunk
    /// BYE (→ emit `Released`) from a pre-answer CANCEL or MSC-initiated
    /// teardown.
    answered_with_2xx: Arc<AtomicBool>,
}

struct InboundDispatcher {
    invite_tx: mpsc::UnboundedSender<libre::InboundSipMessage>,
}

impl libre::InboundSipSessionHandler for InboundDispatcher {
    fn on_invite(&self, msg: libre::InboundSipMessage) {
        if let Err(error) = self.invite_tx.send(msg) {
            log::warn!("inbound SIP dispatcher receiver dropped: {error}");
        }
    }
}

fn send_backend_event(
    events: &SipBackendEventSender,
    event: SipBackendEvent,
    context: &'static str,
) {
    if let Err(error) = events.send(event) {
        log::warn!("SIP backend event receiver dropped while sending {context}: {error}");
    }
}

impl LibreSipBackend {
    pub fn new(config: VoiceGatewayConfig) -> Result<Self> {
        VoiceGatewayConfig::validate(&config).map_err(SipBackendError::Config)?;
        let transport = libre::Transport::try_from(config.sip.transport.as_str())
            .map_err(SipBackendError::Config)?;
        let listen_addr =
            config.sip.listen_addr.parse().map_err(|err| {
                SipBackendError::Config(format!("invalid SIP listen_addr: {err}"))
            })?;
        if let Some(advertise_addr) = config.rtp.advertise_addr.as_deref() {
            advertise_addr.parse::<IpAddr>().map_err(|err| {
                SipBackendError::Config(format!("invalid RTP advertise_addr: {err}"))
            })?;
        }
        let resolved_auth = config
            .sip
            .resolved_auth()
            .map_err(SipBackendError::Config)?;
        let sip_registration = config
            .sip
            .resolved_registration(resolved_auth.as_ref())
            .map_err(SipBackendError::Config)?;
        let sip_auth = resolved_auth.as_ref().map(|auth| libre::SipCredentials {
            username: auth.username.clone(),
            password: auth.password.clone(),
        });
        let sip_registration = sip_registration.map(|registration| libre::SipRegistrationConfig {
            registrar_uri: registration.registrar_uri,
            to_uri: registration.to_uri,
            from_name: registration.from_name,
            from_uri: registration.from_uri,
            contact_user: registration.contact_user,
            expires_secs: registration.expires_secs,
            keepalive_interval_secs: config.sip.keepalive_interval_secs,
            auth: sip_auth.clone(),
        });

        if config.sip.registration.enabled && sip_auth.is_none() {
            log::warn!(
                "SIP registration is enabled without digest auth; most trunk providers require sip.auth"
            );
        }

        let ua_config =
            libre::SipUserAgentConfig::new(listen_addr, transport, config.sip.user_agent.clone());
        preflight_sip_listen_addr(ua_config.listen_addr, ua_config.transport)?;
        let event_loop = libre::EventLoop::spawn()?;
        let local_addr = libre::SocketAddress::from_socket_addr(ua_config.listen_addr)?;
        let trace_handler = if config.logging.sip_trace {
            Some(Arc::new(SipTraceLogger {
                logging: config.logging.clone(),
            }) as Arc<dyn libre::SipTraceHandler>)
        } else {
            None
        };
        let sip_stack = libre::SipStack::new_with_trace(&ua_config.user_agent, trace_handler)?;
        sip_stack
            .add_transport(ua_config.transport, &local_addr)
            .map_err(|source| SipBackendError::Listen {
                listen_addr: ua_config.listen_addr,
                transport: ua_config.transport.as_str().to_string(),
                source,
            })?;
        let (inbound_invite_tx, inbound_invite_rx) = mpsc::unbounded_channel();
        let inbound_dispatcher = Arc::new(InboundDispatcher {
            invite_tx: inbound_invite_tx,
        });
        let (session_socket, inbound_listener) =
            libre::SipSessionSocket::listen_with_inbound_handler(&sip_stack, inbound_dispatcher)
                .map_err(|source| SipBackendError::Listen {
                    listen_addr: ua_config.listen_addr,
                    transport: ua_config.transport.as_str().to_string(),
                    source,
                })?;
        let session_socket = Arc::new(session_socket);
        let registration = if let Some(registration_config) = sip_registration {
            log::info!(
                "starting SIP REGISTER registrar={} from={} contact_user={} expires={} auth={} keepalive_secs={}",
                registration_config.registrar_uri,
                registration_config.from_uri,
                registration_config.contact_user,
                registration_config.expires_secs,
                if registration_config.auth.is_some() {
                    "digest"
                } else {
                    "disabled"
                },
                registration_config.keepalive_interval_secs
            );
            Some(libre::SipRegistration::register(
                &sip_stack,
                registration_config,
                Arc::new(SipRegistrationLogger {
                    logging: config.logging.clone(),
                }),
            )?)
        } else {
            None
        };

        log::info!(
            "initialized libre SIP backend on {} via {} as {} auth={} registration={} sip_trace={}",
            ua_config.listen_addr,
            ua_config.transport.as_str(),
            ua_config.user_agent,
            if sip_auth.is_some() {
                "digest"
            } else {
                "disabled"
            },
            if registration.is_some() {
                "enabled"
            } else {
                "disabled"
            },
            config.logging.sip_trace
        );

        let (gateway_voice_tx, _) = broadcast::channel(config.queues.gateway_voice_frames);
        let rtp_ports = Arc::new(Mutex::new(RtpPortAllocator::new(config.rtp.port_range)));
        let inbound_sessions = Arc::new(Mutex::new(HashMap::new()));
        let inbound_events: Arc<Mutex<Option<SipBackendEventSender>>> = Arc::new(Mutex::new(None));
        let sip_stack = Arc::new(sip_stack);

        let worker_deps = InboundWorkerDeps {
            inbound_sessions: inbound_sessions.clone(),
            rtp_ports: rtp_ports.clone(),
            gateway_voice_tx: gateway_voice_tx.clone(),
            sip_stack: sip_stack.clone(),
            session_socket: session_socket.clone(),
            sdp_address: if let Some(advertise) = config.rtp.advertise_addr.clone() {
                advertise
            } else {
                config.rtp.listen_addr.clone()
            },
            preferred_codecs: config.rtp.preferred_codecs.clone(),
            jitter_buffer_ms: config.jitter_buffer_ms,
            rtp_listen_addr: config.rtp.listen_addr.clone(),
            telephone_event_payload_type: config.rtp.telephone_event_payload_type,
            log_media_frames: config.logging.media_frames,
            log_media_summary: config.logging.media_summary,
            log_sip_sdp: config.logging.sip_sdp,
            inbound_decision_timeout_ms: config.sip.inbound_decision_timeout_ms,
            events: inbound_events.clone(),
        };
        tokio::spawn(run_inbound_worker(inbound_invite_rx, worker_deps));

        Ok(Self {
            rtp_ports,
            sip_auth,
            config,
            sessions: Mutex::new(HashMap::new()),
            inbound_sessions,
            gateway_voice_tx,
            _registration: registration,
            session_socket,
            _sip_stack: sip_stack,
            _event_loop: event_loop,
            inbound_events,
            _inbound_listener: inbound_listener,
        })
    }

    fn request_uri(&self, called_number: &str) -> String {
        self.config
            .sip
            .request_uri_template
            .replace("{called}", called_number)
    }

    fn from_uri(&self, caller_id: &str) -> String {
        format!("sip:{}@{}", caller_id, self.config.sip.from_domain)
    }

    fn sdp_address(&self) -> String {
        if let Some(advertise_addr) = self.config.rtp.advertise_addr.as_deref() {
            return advertise_addr.to_string();
        }

        match self.config.rtp.listen_addr.parse::<IpAddr>() {
            Ok(addr) if !addr.is_unspecified() => addr.to_string(),
            _ => {
                log::warn!(
                    "RTP listen_addr {} is unspecified and rtp.advertise_addr is not set; \
                     SDP will advertise 127.0.0.1 which will prevent media from flowing. \
                     Set rtp.advertise_addr to the public IP of this host.",
                    self.config.rtp.listen_addr
                );
                "127.0.0.1".to_string()
            }
        }
    }

    fn active_session_count(&self) -> usize {
        self.sessions.lock().len()
    }

    async fn bind_media_session(
        &self,
        session_id: &str,
        voice_codec: VoiceCodec,
    ) -> Result<(RtpPortLease, Arc<RtpMediaSession>)> {
        let attempts = self.rtp_ports.lock().capacity();
        let mut last_error = None;

        for _ in 0..attempts {
            let Some(lease) = RtpPortLease::try_acquire(self.rtp_ports.clone()) else {
                break;
            };
            let port = lease.port();
            match RtpMediaSession::bind(
                session_id.to_string(),
                &self.config.rtp.listen_addr,
                port,
                G711Codec::Pcmu,
                voice_codec,
                self.config.jitter_buffer_ms,
                self.config.logging.media_frames,
                self.config.logging.media_summary,
                self.gateway_voice_tx.clone(),
                self.config.rtp.telephone_event_payload_type,
            )
            .await
            {
                Ok(media) => return Ok((lease, media)),
                Err(err) => {
                    log::warn!("failed to bind RTP port {}: {}", port, err);
                    last_error = Some(err);
                }
            }
        }

        Err(SipBackendError::Media(
            last_error.unwrap_or_else(|| "no available RTP ports".to_string()),
        ))
    }

    async fn advertised_rtp_endpoint(
        &self,
        media: &RtpMediaSession,
        rtp_port: u16,
    ) -> Result<AdvertisedRtpEndpoint> {
        match self.config.nat.mode().map_err(SipBackendError::Config)? {
            NatMode::Disabled => Ok(AdvertisedRtpEndpoint {
                addr: self.sdp_address(),
                port: rtp_port,
                source: "static",
            }),
            NatMode::StunLatch => {
                let stun_server = self
                    .config
                    .nat
                    .stun_server()
                    .map_err(SipBackendError::Config)?
                    .expect("stun_server is required for stun_latch");
                let timeout = Duration::from_millis(self.config.nat.stun_timeout_ms);
                let local = media
                    .local_addr()
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string());
                let mapped = media
                    .discover_mapped_addr(stun_server, timeout)
                    .await
                    .map_err(SipBackendError::Media)?;

                log::info!(
                    "RTP NAT STUN mapped endpoint local={} stun_server={} mapped={}",
                    local,
                    stun_server,
                    mapped
                );

                Ok(AdvertisedRtpEndpoint {
                    addr: mapped.ip().to_string(),
                    port: mapped.port(),
                    source: "stun",
                })
            }
        }
    }

    fn build_sdp_offer(&self, endpoint: &AdvertisedRtpEndpoint) -> String {
        let addr = &endpoint.addr;
        let rtp_port = endpoint.port;
        let codecs = offered_g711_codecs(&self.config.rtp.preferred_codecs);
        let telephone_event_pt = self.config.rtp.telephone_event_payload_type;
        let mut payloads: Vec<String> = codecs.iter().map(|c| c.payload.to_string()).collect();
        if let Some(pt) = telephone_event_pt {
            payloads.push(pt.to_string());
        }
        let payloads = payloads.join(" ");
        let mut rtpmap: String = codecs
            .iter()
            .map(|codec| format!("a=rtpmap:{} {}/8000\r\n", codec.payload, codec.name))
            .collect();
        if let Some(pt) = telephone_event_pt {
            rtpmap.push_str(&format!("a=rtpmap:{pt} telephone-event/8000\r\n"));
            rtpmap.push_str(&format!("a=fmtp:{pt} 0-15\r\n"));
        }

        format!(
            "v=0\r\n\
             o=- 0 0 IN IP4 {addr}\r\n\
             s=-\r\n\
             c=IN IP4 {addr}\r\n\
             t=0 0\r\n\
             m=audio {rtp_port} RTP/AVP {payloads}\r\n\
             {rtpmap}\
             a=sendrecv\r\n"
        )
    }

    fn prune_closed_sessions(&self) {
        let mut sessions = self.sessions.lock();

        sessions.retain(|session_id, session| {
            let keep = !session.control.closed.load(Ordering::SeqCst);
            if !keep {
                log::debug!("pruning closed SIP session {}", session_id);
            }
            keep
        });
    }
}

#[async_trait]
impl SipBackend for LibreSipBackend {
    async fn start_outbound_call(
        &self,
        call: OutboundSipCall,
        events: SipBackendEventSender,
    ) -> Result<()> {
        self.prune_closed_sessions();

        if self.active_session_count() >= self.config.calls.max_concurrent_calls {
            return Err(SipBackendError::Unavailable(format!(
                "maximum concurrent SIP calls reached ({})",
                self.config.calls.max_concurrent_calls
            )));
        }

        let request_uri = self.request_uri(&call.called_number);
        let caller_id = self.config.sip.effective_caller_id(&call.caller_number);
        let from_uri = self.from_uri(&caller_id);
        let voice_codec = voice_codec_from_service_option(call.service_option)?;
        let (rtp_port, media) = self
            .bind_media_session(&call.session_id, voice_codec)
            .await?;
        let local_rtp_port = rtp_port.port();
        let advertised_endpoint = self.advertised_rtp_endpoint(&media, local_rtp_port).await?;
        let rtp_task = tokio::spawn(media.clone().receive_loop());
        let sdp_offer = self.build_sdp_offer(&advertised_endpoint);
        let nat_mode = self.config.nat.mode().map_err(SipBackendError::Config)?;
        if self.config.logging.sip_sdp {
            log::debug!(
                "SIP SDP offer session={} local_rtp_port={} advertised_rtp={}:{} source={}:\n{}",
                call.session_id,
                local_rtp_port,
                advertised_endpoint.addr,
                advertised_endpoint.port,
                advertised_endpoint.source,
                sdp_offer.trim_end()
            );
        }
        let control = Arc::new(SessionControl::new());
        let handler = Arc::new(LibreSessionEventHandler {
            events: events.clone(),
            media: media.clone(),
            control: control.clone(),
            negotiated_codec: Mutex::new(None),
            logging: self.config.logging.clone(),
            rtp_latch: RtpLatchConfig::from_nat_mode(nat_mode, &self.config),
            runtime: Handle::current(),
        });

        let session = Arc::new(libre::OutboundSipSession::connect(
            &self.session_socket,
            libre::OutboundSipSessionConfig {
                session_id: call.session_id.clone(),
                to_uri: request_uri.clone(),
                from_name: Some(caller_id.clone()),
                from_uri,
                contact_user: caller_id.clone(),
                call_id: Some(call.session_id.clone()),
                sdp_offer,
                auth: self.sip_auth.clone(),
            },
            handler,
        )?);
        let timeout_tasks = spawn_session_timeout_tasks(
            call.session_id.clone(),
            session.clone(),
            media.clone(),
            events,
            control.clone(),
            self.config.calls.clone(),
        );

        log::info!(
            "started libre outbound SIP call: session={} from={} to={} uri={} service_option={} voice_codec={:?} local_rtp_port={} advertised_rtp={}:{} source={} nat={:?}",
            call.session_id,
            caller_id,
            call.called_number,
            request_uri,
            call.service_option,
            voice_codec,
            local_rtp_port,
            advertised_endpoint.addr,
            advertised_endpoint.port,
            advertised_endpoint.source,
            nat_mode
        );

        self.sessions.lock().insert(
            call.session_id,
            ActiveSession {
                session,
                media,
                rtp_task,
                control,
                timeout_tasks,
                _rtp_port: rtp_port,
            },
        );

        Ok(())
    }

    async fn release_call(&self, session_id: &str, reason: ReleaseReason) -> Result<()> {
        self.prune_closed_sessions();

        if let Some(session) = self.sessions.lock().remove(session_id) {
            log::info!(
                "releasing libre SIP call: session={} reason={:?}",
                session_id,
                reason
            );
            session
                .media
                .log_summary_once(&format!("bsc_release:{reason:?}"));
            session.session.abort();
            return Ok(());
        }

        // Inbound (UAS) leg — drop the session so libre's sipsess deref
        // triggers BYE to the trunk. Mark `answered` first so the close_h
        // callback recognizes that we initiated the teardown (not the trunk)
        // and skips emitting InboundCancel.
        let removed = {
            let mut sessions = self.inbound_sessions.lock();
            if let Some(state) = sessions.get(session_id) {
                state.answered.store(true, Ordering::SeqCst);
            }
            sessions.remove(session_id)
        };
        if let Some(state) = removed {
            log::info!(
                "releasing libre inbound SIP call: session={} reason={:?}",
                session_id,
                reason
            );
            drop(state);
        } else {
            log::debug!(
                "ignoring release for unknown libre SIP session {} reason={:?}",
                session_id,
                reason
            );
        }

        Ok(())
    }

    async fn handle_air_frame(&self, frame: AirVoiceFrame) -> Result<()> {
        self.prune_closed_sessions();

        let media = self
            .sessions
            .lock()
            .get(&frame.session_id)
            .map(|session| session.media.clone())
            .or_else(|| {
                self.inbound_sessions
                    .lock()
                    .get(&frame.session_id)
                    .map(|state| state.media.clone())
            });

        let Some(media) = media else {
            log::trace!(
                "dropping media frame for unknown SIP session {}",
                frame.session_id
            );
            return Ok(());
        };

        media
            .send_air_frame(&frame.bits, frame.rate_bps, frame.service_option)
            .await
            .map_err(SipBackendError::Media)
    }

    async fn send_dtmf(&self, req: crate::sip::SendDtmfRequest) -> Result<()> {
        let media = self
            .sessions
            .lock()
            .get(&req.session_id)
            .map(|session| session.media.clone())
            .or_else(|| {
                self.inbound_sessions
                    .lock()
                    .get(&req.session_id)
                    .map(|state| state.media.clone())
            });
        let Some(media) = media else {
            log::trace!(
                "dropping DTMF event for unknown SIP session {}",
                req.session_id
            );
            return Ok(());
        };
        media
            .send_dtmf_event(
                req.event_code,
                req.volume,
                req.duration_samples,
                req.end,
                req.start_of_event,
            )
            .await
            .map_err(SipBackendError::Media)
    }

    fn subscribe_gateway_voice_frames(&self) -> broadcast::Receiver<GatewayVoiceFrame> {
        self.gateway_voice_tx.subscribe()
    }

    async fn register_inbound_handler(&self, events: SipBackendEventSender) -> Result<()> {
        *self.inbound_events.lock() = Some(events);
        Ok(())
    }

    async fn inbound_progress(&self, session_id: &str) -> Result<()> {
        let sessions = self.inbound_sessions.lock();
        let Some(state) = sessions.get(session_id) else {
            return Err(SipBackendError::Unavailable(format!(
                "inbound_progress: no inbound session {session_id}"
            )));
        };
        state.session.progress(180, "Ringing")?;
        log::info!("inbound SIP session {session_id} 180 Ringing");
        Ok(())
    }

    async fn inbound_answer(&self, session_id: &str, requested_codec: &str) -> Result<()> {
        let sessions = self.inbound_sessions.lock();
        let Some(state) = sessions.get(session_id) else {
            return Err(SipBackendError::Unavailable(format!(
                "inbound_answer: no inbound session {session_id}"
            )));
        };
        // CAS-claim the session before sending 200 so a concurrent watchdog timeout
        // can't fire 408 after we answer (or vice versa).
        if state
            .answered
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Err(SipBackendError::Unavailable(format!(
                "inbound_answer: session {session_id} already finalized"
            )));
        }
        let chosen_codec = G711Codec::from_name(requested_codec)
            .filter(|c| *c == state.chosen_codec)
            .unwrap_or(state.chosen_codec);
        if self.config.logging.sip_sdp {
            log::debug!(
                "SIP SDP answer session={session_id}:\n{}",
                state.sdp_answer.trim_end()
            );
        }
        state.session.answer(200, "OK", &state.sdp_answer)?;
        state.answered_with_2xx.store(true, Ordering::SeqCst);
        log::info!(
            "inbound SIP session {session_id} 200 OK codec={}",
            g711_codec_name(chosen_codec)
        );
        Ok(())
    }

    async fn inbound_reject(&self, session_id: &str, sip_status: u16) -> Result<()> {
        let state = {
            let mut sessions = self.inbound_sessions.lock();
            let Some(state) = sessions.get(session_id) else {
                log::debug!("inbound_reject: no inbound session {session_id} (already torn down)");
                return Ok(());
            };
            if state
                .answered
                .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                .is_err()
            {
                log::debug!(
                    "inbound_reject: session {session_id} already finalized; skipping {sip_status}"
                );
                return Ok(());
            }
            sessions.remove(session_id)
        };
        let Some(state) = state else { return Ok(()) };
        let reason = sip_reason_phrase(sip_status);
        if let Err(error) = state.session.reject(sip_status, reason) {
            log::warn!("inbound SIP session {session_id} reject({sip_status}) failed: {error}");
        } else {
            log::info!("inbound SIP session {session_id} rejected with {sip_status} {reason}");
        }
        Ok(())
    }
}

fn voice_codec_from_service_option(service_option: u16) -> Result<VoiceCodec> {
    let service_option = if service_option == 0 {
        3
    } else {
        service_option
    };
    VoiceCodec::from_service_option(service_option).ok_or_else(|| {
        SipBackendError::Media(format!(
            "unsupported voice gateway service option {service_option}"
        ))
    })
}

fn preflight_sip_listen_addr(listen_addr: SocketAddr, transport: libre::Transport) -> Result<()> {
    match transport {
        libre::Transport::Udp => UdpSocket::bind(listen_addr)
            .map(|_socket| ())
            .map_err(|source| SipBackendError::ListenPreflight {
                listen_addr,
                transport: transport.as_str().to_string(),
                source,
            }),
        libre::Transport::Tcp | libre::Transport::Tls => TcpListener::bind(listen_addr)
            .map(|_listener| ())
            .map_err(|source| SipBackendError::ListenPreflight {
                listen_addr,
                transport: transport.as_str().to_string(),
                source,
            }),
    }
}

struct ActiveSession {
    session: Arc<libre::OutboundSipSession>,
    media: Arc<RtpMediaSession>,
    rtp_task: JoinHandle<()>,
    control: Arc<SessionControl>,
    timeout_tasks: Vec<JoinHandle<()>>,
    _rtp_port: RtpPortLease,
}

impl Drop for ActiveSession {
    fn drop(&mut self) {
        self.media.log_summary_once("session_drop");
        self.rtp_task.abort();
        for task in &self.timeout_tasks {
            task.abort();
        }
    }
}

#[derive(Debug)]
struct RtpPortAllocator {
    min: u16,
    max: u16,
    next: u16,
    allocated: HashSet<u16>,
}

impl RtpPortAllocator {
    fn new(port_range: [u16; 2]) -> Self {
        let [min, max] = port_range;
        Self {
            min,
            max,
            next: min,
            allocated: HashSet::new(),
        }
    }

    fn capacity(&self) -> usize {
        usize::from((self.max - self.min) / 2) + 1
    }

    fn allocate(&mut self) -> Option<u16> {
        for _ in 0..self.capacity() {
            let port = self.next;
            let following = port.saturating_add(2);
            self.next = if following > self.max {
                self.min
            } else {
                following
            };

            if self.allocated.insert(port) {
                return Some(port);
            }
        }

        None
    }

    fn release(&mut self, port: u16) {
        self.allocated.remove(&port);
    }
}

struct RtpPortLease {
    port: u16,
    allocator: Arc<Mutex<RtpPortAllocator>>,
}

impl RtpPortLease {
    fn try_acquire(allocator: Arc<Mutex<RtpPortAllocator>>) -> Option<Self> {
        let port = allocator.lock().allocate()?;
        Some(Self { port, allocator })
    }

    fn port(&self) -> u16 {
        self.port
    }
}

impl Drop for RtpPortLease {
    fn drop(&mut self) {
        self.allocator.lock().release(self.port);
    }
}

#[derive(Debug)]
struct SessionControl {
    phase: AtomicU8,
    closed: AtomicBool,
}

impl SessionControl {
    fn new() -> Self {
        Self {
            phase: AtomicU8::new(SessionPhase::Initiating as u8),
            closed: AtomicBool::new(false),
        }
    }

    fn phase(&self) -> SessionPhase {
        SessionPhase::from_u8(self.phase.load(Ordering::SeqCst))
    }

    fn set_phase(&self, phase: SessionPhase) {
        self.phase.store(phase as u8, Ordering::SeqCst);
    }

    fn close_once(&self) -> bool {
        !self.closed.swap(true, Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum SessionPhase {
    Initiating = 0,
    Ringing = 1,
    Established = 2,
    Closed = 3,
}

impl SessionPhase {
    fn from_u8(value: u8) -> Self {
        match value {
            1 => Self::Ringing,
            2 => Self::Established,
            3 => Self::Closed,
            _ => Self::Initiating,
        }
    }
}

struct AdvertisedRtpEndpoint {
    addr: String,
    port: u16,
    source: &'static str,
}

#[derive(Clone, Copy)]
struct RtpLatchConfig {
    packets: u8,
    interval: Duration,
}

impl RtpLatchConfig {
    fn from_nat_mode(nat_mode: NatMode, config: &VoiceGatewayConfig) -> Option<Self> {
        if nat_mode != NatMode::StunLatch || config.nat.rtp_latch_packets == 0 {
            return None;
        }

        Some(Self {
            packets: config.nat.rtp_latch_packets,
            interval: Duration::from_millis(config.nat.rtp_latch_interval_ms),
        })
    }
}

struct SipTraceLogger {
    logging: LoggingConfig,
}

impl libre::SipTraceHandler for SipTraceLogger {
    fn on_trace(&self, event: libre::SipTraceEvent) {
        let direction = if event.tx { "tx" } else { "rx" };
        let src = event.src.as_deref().unwrap_or("-");
        let dst = event.dst.as_deref().unwrap_or("-");
        let packet = format_sip_trace_packet(&event.packet, self.logging.sip_sdp);

        log::debug!(
            "SIP trace {} transport={} {} -> {} bytes={}:\n{}",
            direction,
            event.transport.as_str(),
            src,
            dst,
            event.packet.len(),
            packet.trim_end()
        );
    }
}

struct SipRegistrationLogger {
    logging: LoggingConfig,
}

impl libre::SipRegistrationHandler for SipRegistrationLogger {
    fn on_event(&self, event: libre::SipRegistrationEvent) {
        if !self.logging.sip_events {
            return;
        }

        match event {
            libre::SipRegistrationEvent::Response {
                error,
                sip_status,
                reason,
            } => {
                let reason = reason.unwrap_or_else(|| "<none>".to_string());
                if error == 0 && (200..300).contains(&sip_status) {
                    log::info!(
                        "SIP REGISTER accepted status={} reason={}",
                        sip_status,
                        reason
                    );
                } else if sip_status != 0 {
                    log::warn!(
                        "SIP REGISTER response err={} status={} reason={}",
                        error,
                        sip_status,
                        reason
                    );
                } else {
                    log::warn!("SIP REGISTER failed err={} reason={}", error, reason);
                }
            }
        }
    }

    fn on_auth_challenge(&self, realm: Option<&str>) {
        if self.logging.sip_events {
            log::debug!(
                "SIP REGISTER digest auth challenge received realm={}",
                realm.unwrap_or("<none>")
            );
        }
    }
}

struct LibreSessionEventHandler {
    events: SipBackendEventSender,
    media: Arc<RtpMediaSession>,
    control: Arc<SessionControl>,
    negotiated_codec: Mutex<Option<String>>,
    logging: LoggingConfig,
    rtp_latch: Option<RtpLatchConfig>,
    runtime: Handle,
}

impl libre::OutboundSipSessionHandler for LibreSessionEventHandler {
    fn on_event(&self, event: libre::OutboundSipSessionEvent) {
        match event {
            libre::OutboundSipSessionEvent::Progress {
                session_id,
                sip_status,
                sdp,
            } => {
                if self.logging.sip_events {
                    log::debug!("SIP session {} progress status={}", session_id, sip_status);
                }
                if self.control.phase() == SessionPhase::Initiating {
                    self.control.set_phase(SessionPhase::Ringing);
                }
                if !sdp.is_empty() {
                    if self.logging.sip_sdp {
                        log::debug!(
                            "SIP SDP early media session={}:\n{}",
                            session_id,
                            String::from_utf8_lossy(&sdp).trim_end()
                        );
                    }
                    self.apply_sdp(&session_id, sip_status, &sdp, true);
                }
                if sip_status != 100 {
                    let codec = self.negotiated_codec.lock().clone().unwrap_or_default();
                    send_backend_event(
                        &self.events,
                        SipBackendEvent::Ringing {
                            session_id,
                            sip_status,
                            codec,
                        },
                        "ringing",
                    );
                }
            }
            libre::OutboundSipSessionEvent::Answer {
                session_id,
                sip_status,
                sdp,
            } => {
                if self.logging.sip_events {
                    log::debug!("SIP session {} answer status={}", session_id, sip_status);
                }
                if self.logging.sip_sdp {
                    log::debug!(
                        "SIP SDP answer session={}:\n{}",
                        session_id,
                        String::from_utf8_lossy(&sdp).trim_end()
                    );
                }
                self.apply_sdp(&session_id, sip_status, &sdp, false);
            }
            libre::OutboundSipSessionEvent::Established {
                session_id,
                sip_status,
            } => {
                if self.logging.sip_events {
                    log::debug!(
                        "SIP session {} established status={}",
                        session_id,
                        sip_status
                    );
                }
                self.media.mark_activity();
                self.control.set_phase(SessionPhase::Established);
                let codec = self
                    .negotiated_codec
                    .lock()
                    .clone()
                    .unwrap_or_else(|| "PCMU".to_string());

                send_backend_event(
                    &self.events,
                    SipBackendEvent::Answered {
                        session_id,
                        sip_status,
                        codec,
                    },
                    "answered",
                );
            }
            libre::OutboundSipSessionEvent::Closed {
                session_id,
                error,
                sip_status,
            } => {
                if self.logging.sip_events {
                    log::debug!(
                        "SIP session {} closed err={} status={} phase={:?}",
                        session_id,
                        error,
                        sip_status,
                        self.control.phase()
                    );
                }
                self.media.log_summary_once(&format!(
                    "sip_closed:err={error}:status={sip_status}:phase={:?}",
                    self.control.phase()
                ));
                let phase = self.control.phase();
                self.control.set_phase(SessionPhase::Closed);
                if !self.control.close_once() {
                    return;
                }
                if phase == SessionPhase::Established {
                    send_backend_event(
                        &self.events,
                        SipBackendEvent::Released {
                            session_id,
                            reason: ReleaseReason::SipReleased,
                        },
                        "released",
                    );
                } else {
                    let status = (sip_status != 0).then_some(sip_status);
                    let reason = if let Some(status) = status {
                        format!("SIP call failed with final status {}", status)
                    } else {
                        format!("SIP call failed with libre error {}", error)
                    };
                    send_backend_event(
                        &self.events,
                        SipBackendEvent::Failed {
                            session_id,
                            sip_status: status,
                            reason,
                        },
                        "failed",
                    );
                }
            }
        }
    }

    fn on_auth_challenge(&self, realm: Option<&str>) {
        if self.logging.sip_events {
            log::debug!(
                "SIP digest auth challenge received realm={}",
                realm.unwrap_or("<none>")
            );
        }
    }
}

impl LibreSessionEventHandler {
    fn apply_sdp(&self, session_id: &str, sip_status: u16, sdp: &[u8], early_media: bool) {
        let Some(codec) = negotiated_codec(sdp) else {
            log::warn!(
                "SIP session {} status={} SDP did not negotiate a supported G.711 codec",
                session_id,
                sip_status
            );
            return;
        };
        let Some(codec_kind) = G711Codec::from_name(&codec) else {
            log::warn!(
                "SIP session {} status={} negotiated unsupported codec {}",
                session_id,
                sip_status,
                codec
            );
            return;
        };

        match self.media.set_remote_from_sdp(sdp, codec_kind) {
            Ok(remote) => {
                log::info!(
                    "SIP session {} negotiated {} RTP remote {} early_media={}",
                    session_id,
                    codec,
                    remote,
                    early_media
                );
                *self.negotiated_codec.lock() = Some(codec);
                if let Some(latch) = self.rtp_latch {
                    let _latch_task = spawn_rtp_latch(
                        &self.runtime,
                        session_id.to_string(),
                        self.media.clone(),
                        latch,
                    );
                }
            }
            Err(err) => {
                log::warn!(
                    "SIP session {} status={} negotiated {} but SDP RTP endpoint was unusable: {}",
                    session_id,
                    sip_status,
                    codec,
                    err
                );
            }
        }
    }
}

fn spawn_session_timeout_tasks(
    session_id: String,
    session: Arc<libre::OutboundSipSession>,
    media: Arc<RtpMediaSession>,
    events: SipBackendEventSender,
    control: Arc<SessionControl>,
    calls: CallConfig,
) -> Vec<JoinHandle<()>> {
    let mut tasks = Vec::new();

    if calls.setup_timeout_ms > 0 {
        tasks.push(spawn_phase_timeout(
            session_id.clone(),
            session.clone(),
            media.clone(),
            events.clone(),
            control.clone(),
            Duration::from_millis(calls.setup_timeout_ms),
            SessionPhase::Initiating,
            "SIP setup timeout",
        ));
    }

    if calls.ringing_timeout_ms > 0 {
        tasks.push(spawn_phase_timeout(
            session_id.clone(),
            session.clone(),
            media.clone(),
            events.clone(),
            control.clone(),
            Duration::from_millis(calls.ringing_timeout_ms),
            SessionPhase::Ringing,
            "SIP ringing timeout",
        ));
    }

    if calls.media_idle_timeout_ms > 0 {
        let idle_timeout = Duration::from_millis(calls.media_idle_timeout_ms);
        tasks.push(tokio::spawn(async move {
            let poll_interval = Duration::from_millis(1_000);
            loop {
                sleep(poll_interval).await;
                if control.closed.load(Ordering::SeqCst) {
                    return;
                }
                if control.phase() != SessionPhase::Established {
                    continue;
                }
                if media.last_activity_elapsed() < idle_timeout {
                    continue;
                }

                if !control.close_once() {
                    return;
                }
                control.set_phase(SessionPhase::Closed);
                log::warn!(
                    "SIP session {} RTP media idle timeout after {} ms",
                    session_id,
                    idle_timeout.as_millis()
                );
                media.log_summary_once("rtp_media_idle_timeout");
                send_backend_event(
                    &events,
                    SipBackendEvent::Failed {
                        session_id: session_id.clone(),
                        sip_status: None,
                        reason: "RTP media idle timeout".to_string(),
                    },
                    "rtp idle timeout",
                );
                session.abort();
                return;
            }
        }));
    }

    tasks
}

fn spawn_phase_timeout(
    session_id: String,
    session: Arc<libre::OutboundSipSession>,
    media: Arc<RtpMediaSession>,
    events: SipBackendEventSender,
    control: Arc<SessionControl>,
    timeout: Duration,
    phase: SessionPhase,
    reason: &'static str,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        sleep(timeout).await;
        if control.closed.load(Ordering::SeqCst) || control.phase() != phase {
            return;
        }
        if !control.close_once() {
            return;
        }
        control.set_phase(SessionPhase::Closed);
        log::warn!(
            "SIP session {} {} after {} ms",
            session_id,
            reason,
            timeout.as_millis()
        );
        media.log_summary_once(reason);
        send_backend_event(
            &events,
            SipBackendEvent::Failed {
                session_id: session_id.clone(),
                sip_status: None,
                reason: reason.to_string(),
            },
            "phase timeout",
        );
        session.abort();
    })
}

fn spawn_rtp_latch(
    runtime: &Handle,
    session_id: String,
    media: Arc<RtpMediaSession>,
    latch: RtpLatchConfig,
) -> JoinHandle<()> {
    runtime.spawn(async move {
        log::debug!(
            "sending RTP NAT latch frames session={} packets={} interval_ms={}",
            session_id,
            latch.packets,
            latch.interval.as_millis()
        );
        if let Err(err) = media.send_latch_frames(latch.packets, latch.interval).await {
            log::warn!("RTP NAT latch failed session={}: {}", session_id, err);
        }
    })
}

fn format_sip_trace_packet(packet: &[u8], include_body: bool) -> String {
    let text = String::from_utf8_lossy(packet).replace("\r\n", "\n");
    let mut formatted = Vec::new();
    let mut in_body = false;
    let mut omitted_body = false;

    for line in text.split('\n') {
        if in_body {
            if include_body {
                formatted.push(line.to_string());
            } else if !line.is_empty() {
                omitted_body = true;
                break;
            }
            continue;
        }

        if line.is_empty() {
            in_body = true;
            if include_body {
                formatted.push(String::new());
            }
            continue;
        }

        formatted.push(redact_sip_trace_header(line));
    }

    if omitted_body {
        formatted.push("<body omitted; enable logging.sip_sdp to include SDP>".to_string());
    }

    formatted.join("\n")
}

fn redact_sip_trace_header(line: &str) -> String {
    let Some((name, _)) = line.split_once(':') else {
        return line.to_string();
    };

    match name.trim().to_ascii_lowercase().as_str() {
        "authorization"
        | "proxy-authorization"
        | "www-authenticate"
        | "proxy-authenticate"
        | "authentication-info" => {
            format!("{}: <redacted>", name.trim())
        }
        _ => line.to_string(),
    }
}

fn negotiated_codec(sdp: &[u8]) -> Option<String> {
    let sdp = std::str::from_utf8(sdp).ok()?;
    let mut audio_payloads = Vec::new();
    let mut rtpmap = HashMap::new();

    for raw_line in sdp.lines() {
        let line = raw_line.trim();
        let upper = line.to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("M=AUDIO ") {
            audio_payloads = rest
                .split_whitespace()
                .skip(2)
                .filter_map(|payload| payload.parse::<u8>().ok())
                .collect();
            continue;
        }
        if let Some(rest) = upper.strip_prefix("A=RTPMAP:") {
            let Some((payload, codec)) = rest.split_once(' ') else {
                continue;
            };
            let Ok(payload) = payload.parse::<u8>() else {
                continue;
            };
            let codec_name = codec.split('/').next().unwrap_or(codec).to_string();
            rtpmap.insert(payload, codec_name);
        }
    }

    for payload in audio_payloads {
        let codec = rtpmap
            .get(&payload)
            .map(String::as_str)
            .or_else(|| match payload {
                0 => Some("PCMU"),
                8 => Some("PCMA"),
                _ => None,
            });
        match codec {
            Some("PCMU") => return Some("PCMU".to_string()),
            Some("PCMA") => return Some("PCMA".to_string()),
            _ => {}
        }
    }

    None
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct StaticG711Codec {
    payload: u8,
    name: &'static str,
}

fn offered_g711_codecs(preferred_codecs: &[String]) -> Vec<StaticG711Codec> {
    let mut codecs: Vec<StaticG711Codec> = Vec::new();

    for codec in preferred_codecs {
        let codec = match codec.trim().to_ascii_uppercase().as_str() {
            "PCMU" | "G711U" | "ULAW" | "MU-LAW" => StaticG711Codec {
                payload: 0,
                name: "PCMU",
            },
            "PCMA" | "G711A" | "ALAW" | "A-LAW" => StaticG711Codec {
                payload: 8,
                name: "PCMA",
            },
            _ => continue,
        };

        if !codecs
            .iter()
            .any(|existing| existing.payload == codec.payload)
        {
            codecs.push(codec);
        }
    }

    if codecs.is_empty() {
        codecs.push(StaticG711Codec {
            payload: 0,
            name: "PCMU",
        });
        codecs.push(StaticG711Codec {
            payload: 8,
            name: "PCMA",
        });
    }

    codecs
}

#[cfg(test)]
mod tests {
    use super::{
        RtpLatchConfig, RtpPortAllocator, RtpPortLease, StaticG711Codec, format_sip_trace_packet,
        negotiated_codec, normalize_did, offered_g711_codecs, parse_offered_g711,
        preflight_sip_listen_addr, spawn_rtp_latch,
    };
    use crate::media::{G711Codec, RtpMediaSession};
    use crate::sip::SipBackendError;
    use cdma_voice::VoiceCodec;
    use parking_lot::Mutex;
    use std::net::UdpSocket;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::runtime::Handle;
    use tokio::sync::broadcast;

    #[test]
    fn extracts_static_g711_codecs_from_sdp() {
        assert_eq!(
            negotiated_codec(b"m=audio 49170 RTP/AVP 0\r\na=rtpmap:0 PCMU/8000\r\n"),
            Some("PCMU".to_string())
        );
        assert_eq!(
            negotiated_codec(b"m=audio 49170 RTP/AVP 8\r\na=rtpmap:8 PCMA/8000\r\n"),
            Some("PCMA".to_string())
        );
        assert_eq!(
            negotiated_codec(b"m=audio 49170 RTP/AVP 8 0\r\n"),
            Some("PCMA".to_string())
        );
    }

    #[test]
    fn returns_none_for_unknown_sdp_codec() {
        assert_eq!(
            negotiated_codec(b"m=audio 49170 RTP/AVP 111\r\na=rtpmap:111 OPUS/48000/2\r\n"),
            None
        );
    }

    #[test]
    fn honors_preferred_g711_offer_order() {
        assert_eq!(
            offered_g711_codecs(&["PCMA".to_string(), "PCMU".to_string()]),
            vec![
                StaticG711Codec {
                    payload: 8,
                    name: "PCMA"
                },
                StaticG711Codec {
                    payload: 0,
                    name: "PCMU"
                },
            ]
        );
    }

    #[test]
    fn falls_back_to_pcmu_pcma_offer_when_preferences_are_unknown() {
        assert_eq!(
            offered_g711_codecs(&["OPUS".to_string()]),
            vec![
                StaticG711Codec {
                    payload: 0,
                    name: "PCMU"
                },
                StaticG711Codec {
                    payload: 8,
                    name: "PCMA"
                },
            ]
        );
    }

    #[test]
    fn rtp_port_allocator_reserves_releases_and_wraps() {
        let allocator = Arc::new(Mutex::new(RtpPortAllocator::new([10_000, 10_004])));
        let first = RtpPortLease::try_acquire(allocator.clone()).unwrap();
        let second = RtpPortLease::try_acquire(allocator.clone()).unwrap();
        let third = RtpPortLease::try_acquire(allocator.clone()).unwrap();

        assert_eq!(first.port(), 10_000);
        assert_eq!(second.port(), 10_002);
        assert_eq!(third.port(), 10_004);
        assert!(RtpPortLease::try_acquire(allocator.clone()).is_none());

        drop(second);
        let reused = RtpPortLease::try_acquire(allocator).unwrap();
        assert_eq!(reused.port(), 10_002);
    }

    #[test]
    fn sip_preflight_rejects_occupied_udp_listen_addr() {
        let occupied = UdpSocket::bind("127.0.0.1:0").unwrap();
        let addr = occupied.local_addr().unwrap();

        let err = preflight_sip_listen_addr(addr, libre::Transport::Udp).unwrap_err();

        assert!(matches!(
            err,
            SipBackendError::ListenPreflight {
                listen_addr,
                ref transport,
                ..
            } if listen_addr == addr && transport == "udp"
        ));
        assert!(err.to_string().contains(&addr.to_string()));
    }

    #[tokio::test]
    async fn rtp_latch_uses_runtime_handle_from_non_tokio_thread() {
        let (gateway_voice_tx, _) = broadcast::channel(1);
        let media = RtpMediaSession::bind(
            "test-session".to_string(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            VoiceCodec::EvrcA,
            60,
            false,
            false,
            gateway_voice_tx,
            None,
        )
        .await
        .unwrap();
        let runtime = Handle::current();
        let latch = RtpLatchConfig {
            packets: 1,
            interval: Duration::from_millis(0),
        };

        let join = std::thread::spawn(move || {
            spawn_rtp_latch(&runtime, "test-session".to_string(), media, latch)
        })
        .join()
        .unwrap();

        join.await.unwrap();
    }

    #[test]
    fn sip_trace_redacts_auth_headers_and_omits_body_by_default() {
        let packet = b"INVITE sip:15551212@example.net SIP/2.0\r\n\
Authorization: Digest username=\"user\", response=\"secret\"\r\n\
Proxy-Authorization: Digest username=\"user\", response=\"proxy-secret\"\r\n\
Content-Type: application/sdp\r\n\
\r\n\
v=0\r\n\
c=IN IP4 203.0.113.10\r\n";

        let trace = format_sip_trace_packet(packet, false);

        assert!(trace.contains("Authorization: <redacted>"));
        assert!(trace.contains("Proxy-Authorization: <redacted>"));
        assert!(!trace.contains("secret"));
        assert!(!trace.contains("v=0"));
        assert!(trace.contains("body omitted"));
    }

    #[test]
    fn normalize_did_strips_leading_plus() {
        assert_eq!(
            normalize_did("+14805551212"),
            Some("14805551212".to_string())
        );
        assert_eq!(normalize_did("4805551212"), Some("4805551212".to_string()));
        assert!(normalize_did("not-a-number").is_none());
        assert!(normalize_did("").is_none());
    }

    #[test]
    fn parse_offered_g711_extracts_pcmu_and_pcma() {
        let sdp =
            b"v=0\r\nm=audio 17118 RTP/AVP 0 8\r\na=rtpmap:0 PCMU/8000\r\na=rtpmap:8 PCMA/8000\r\n";
        assert_eq!(
            parse_offered_g711(sdp),
            vec![G711Codec::Pcmu, G711Codec::Pcma]
        );
    }

    #[test]
    fn parse_offered_g711_falls_back_to_static_payload_types() {
        let sdp = b"m=audio 17118 RTP/AVP 0 8\r\n";
        assert_eq!(
            parse_offered_g711(sdp),
            vec![G711Codec::Pcmu, G711Codec::Pcma]
        );
    }

    #[test]
    fn parse_offered_g711_returns_empty_when_no_g711() {
        let sdp = b"v=0\r\nm=audio 17118 RTP/AVP 111\r\na=rtpmap:111 OPUS/48000/2\r\n";
        assert!(parse_offered_g711(sdp).is_empty());
    }

    #[test]
    fn sip_trace_can_include_sdp_body() {
        let packet = b"SIP/2.0 200 OK\r\n\
WWW-Authenticate: Digest realm=\"example\", nonce=\"secret\"\r\n\
Content-Type: application/sdp\r\n\
\r\n\
v=0\r\n\
m=audio 40000 RTP/AVP 0\r\n";

        let trace = format_sip_trace_packet(packet, true);

        assert!(trace.contains("WWW-Authenticate: <redacted>"));
        assert!(!trace.contains("nonce=\"secret\""));
        assert!(trace.contains("v=0"));
        assert!(trace.contains("m=audio 40000 RTP/AVP 0"));
    }
}

// ---- Inbound SIP support ----

#[derive(Clone)]
struct InboundWorkerDeps {
    inbound_sessions: Arc<Mutex<HashMap<String, InboundSessionState>>>,
    rtp_ports: Arc<Mutex<RtpPortAllocator>>,
    gateway_voice_tx: broadcast::Sender<GatewayVoiceFrame>,
    sip_stack: Arc<libre::SipStack>,
    session_socket: Arc<libre::SipSessionSocket>,
    sdp_address: String,
    preferred_codecs: Vec<String>,
    jitter_buffer_ms: u64,
    rtp_listen_addr: String,
    telephone_event_payload_type: Option<u8>,
    log_media_frames: bool,
    log_media_summary: bool,
    log_sip_sdp: bool,
    inbound_decision_timeout_ms: u64,
    events: Arc<Mutex<Option<SipBackendEventSender>>>,
}

async fn run_inbound_worker(
    mut invite_rx: mpsc::UnboundedReceiver<libre::InboundSipMessage>,
    deps: InboundWorkerDeps,
) {
    while let Some(msg) = invite_rx.recv().await {
        handle_inbound_invite(&deps, msg).await;
    }
}

async fn handle_inbound_invite(deps: &InboundWorkerDeps, msg: libre::InboundSipMessage) {
    // 100 Trying is sent by sipsess_accept below on the validated path; early-reject
    // paths (404/488/503) send their own final response so no provisional is needed.

    let session_id = Uuid::new_v4().to_string();
    let raw_ruri = msg.request_uri_user().unwrap_or_default();
    let from_user = msg.from_user().unwrap_or_default();
    let from_display = msg.from_display().unwrap_or_default();

    let Some(called_number) = normalize_did(&raw_ruri) else {
        log::info!(
            "inbound: rejecting INVITE — Request-URI user {raw_ruri:?} is not dialable, 404"
        );
        let _ = libre::sip_treply(&deps.sip_stack, &msg, 404, "Not Found");
        return;
    };

    let body = msg.body();
    let offered = parse_offered_g711(&body);
    if offered.is_empty() {
        log::info!(
            "inbound: rejecting INVITE for {called_number} — no G.711 codec in SDP offer, 488"
        );
        let _ = libre::sip_treply(&deps.sip_stack, &msg, 488, "Not Acceptable Here");
        return;
    }

    let chosen_codec = pick_codec(&deps.preferred_codecs, &offered);
    let Some(chosen_codec) = chosen_codec else {
        log::info!(
            "inbound: rejecting INVITE for {called_number} — no shared codec with offered={offered:?}, 488"
        );
        let _ = libre::sip_treply(&deps.sip_stack, &msg, 488, "Not Acceptable Here");
        return;
    };

    let (lease, media) = match bind_inbound_media(deps, &session_id).await {
        Ok(pair) => pair,
        Err(error) => {
            log::warn!(
                "inbound: rejecting INVITE for {called_number} — RTP bind failed: {error}, 503"
            );
            let _ = libre::sip_treply(&deps.sip_stack, &msg, 503, "Service Unavailable");
            return;
        }
    };
    let local_port = lease.port();
    let advertised_addr = deps.sdp_address.clone();

    if deps.log_sip_sdp {
        log::debug!(
            "inbound SDP offer session={session_id} called={called_number} body_len={}",
            body.len()
        );
    }

    let sdp_answer = build_sdp_answer(
        &advertised_addr,
        local_port,
        chosen_codec,
        deps.telephone_event_payload_type,
    );
    if let Err(error) = media.set_remote_from_sdp(&body, chosen_codec) {
        log::warn!("inbound: failed to set RTP remote from offer session={session_id}: {error}");
    }

    // Accept-with-183 up front so libre's close_h fires on CANCEL.
    // libre's sipsess_accept rejects scode<101; 183 keeps the trunk from playing
    // ringback before MS is actually alerting (180 is sent later via progress()).
    let answered = Arc::new(AtomicBool::new(false));
    let answered_with_2xx = Arc::new(AtomicBool::new(false));
    let session_handler = Arc::new(InboundSessionHandler {
        session_id: session_id.clone(),
        sessions: deps.inbound_sessions.clone(),
        events: deps.events.clone(),
        answered: answered.clone(),
        answered_with_2xx: answered_with_2xx.clone(),
        runtime: Handle::current(),
    });
    // Send SDP in the 183 so the trunk has an RTP target up front; MSC may
    // push early-media (ringback) before the 200 OK. The same SDP is echoed
    // verbatim by inbound_answer; no renegotiation.
    let session = match libre::InboundSipSession::accept(
        deps.session_socket.as_ref(),
        &msg,
        183,
        "Session Progress",
        &deps.sdp_address,
        Some(&sdp_answer),
        session_handler,
    ) {
        Ok(s) => s,
        Err(error) => {
            log::warn!(
                "inbound: sipsess_accept(183) failed for session={session_id}: {error}; rejecting 503"
            );
            let _ = libre::sip_treply(&deps.sip_stack, &msg, 503, "Service Unavailable");
            return;
        }
    };
    drop(msg);

    deps.inbound_sessions.lock().insert(
        session_id.clone(),
        InboundSessionState {
            session,
            _rtp_lease: lease,
            media: media.clone(),
            sdp_answer,
            chosen_codec,
            answered,
            answered_with_2xx,
        },
    );

    tokio::spawn(media.receive_loop());

    let offered_names: Vec<String> = offered
        .iter()
        .map(|c| g711_codec_name(*c).to_string())
        .collect();
    log::info!(
        "inbound: INVITE accepted session={session_id} called={called_number} from=\"{from_display}\" <{from_user}> codecs={offered_names:?}"
    );
    if deps.inbound_decision_timeout_ms > 0 {
        let timeout_ms = deps.inbound_decision_timeout_ms;
        let sessions = deps.inbound_sessions.clone();
        let events = deps.events.clone();
        let session_id_for_timeout = session_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(timeout_ms)).await;
            // CAS-claim so a concurrent inbound_answer/inbound_reject can't run
            // between the pending-check and the actual 408 send.
            let state = {
                let mut map = sessions.lock();
                let Some(state) = map.get(&session_id_for_timeout) else {
                    return;
                };
                if state
                    .answered
                    .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                    .is_err()
                {
                    return;
                }
                map.remove(&session_id_for_timeout)
            };
            if let Some(state) = state {
                log::warn!(
                    "inbound SIP session {session_id_for_timeout} no MSC decision after {timeout_ms}ms; rejecting 408"
                );
                let _ = state.session.reject(408, "Request Timeout");
                // Also notify MSC so it unwinds the half-set-up MT call (the
                // gateway-initiated 408 has no SIP-side CANCEL that would
                // otherwise drive InboundCancel through libre's close_h).
                if let Some(sender) = events.lock().clone() {
                    send_backend_event(
                        &sender,
                        SipBackendEvent::InboundCancel {
                            session_id: session_id_for_timeout.clone(),
                        },
                        "inbound 408 timeout",
                    );
                }
            }
        });
    }

    let Some(sender) = deps.events.lock().clone() else {
        log::warn!(
            "inbound: dropping InboundInvite session={session_id} — no MSC event sender registered"
        );
        return;
    };
    send_backend_event(
        &sender,
        SipBackendEvent::InboundInvite {
            session_id,
            called_number,
            caller_number: from_user,
            caller_display: from_display,
            offered_codecs: offered_names,
        },
        "inbound_invite",
    );
}

async fn bind_inbound_media(
    deps: &InboundWorkerDeps,
    session_id: &str,
) -> std::result::Result<(RtpPortLease, Arc<RtpMediaSession>), String> {
    let attempts = deps.rtp_ports.lock().capacity();
    let mut last_error = None;
    for _ in 0..attempts {
        let Some(lease) = RtpPortLease::try_acquire(deps.rtp_ports.clone()) else {
            break;
        };
        let port = lease.port();
        match RtpMediaSession::bind(
            session_id.to_string(),
            &deps.rtp_listen_addr,
            port,
            G711Codec::Pcmu,
            VoiceCodec::EvrcA,
            deps.jitter_buffer_ms,
            deps.log_media_frames,
            deps.log_media_summary,
            deps.gateway_voice_tx.clone(),
            deps.telephone_event_payload_type,
        )
        .await
        {
            Ok(media) => return Ok((lease, media)),
            Err(err) => {
                log::warn!("inbound: RTP bind on port {port} failed: {err}");
                last_error = Some(err);
            }
        }
    }
    Err(last_error.unwrap_or_else(|| "no available RTP ports".to_string()))
}

fn normalize_did(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    let stripped = trimmed.strip_prefix('+').unwrap_or(trimmed);
    if stripped.is_empty() || !stripped.chars().all(|c| c.is_ascii_digit()) {
        return None;
    }
    Some(stripped.to_string())
}

fn parse_offered_g711(sdp: &[u8]) -> Vec<G711Codec> {
    let text = match std::str::from_utf8(sdp) {
        Ok(t) => t,
        Err(_) => return Vec::new(),
    };

    let mut rtpmap_to_name: HashMap<&str, &str> = HashMap::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("a=rtpmap:") {
            if let Some((payload, codec)) = rest.split_once(' ') {
                let name = codec.split('/').next().unwrap_or(codec);
                rtpmap_to_name.insert(payload, name);
            }
        }
    }

    let mut codecs = Vec::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("m=audio") {
            for token in rest.split_whitespace().skip(2) {
                if let Some(codec) = rtpmap_to_name
                    .get(token)
                    .and_then(|name| G711Codec::from_name(name))
                {
                    if !codecs.contains(&codec) {
                        codecs.push(codec);
                    }
                    continue;
                }
                if let Ok(pt) = token.parse::<u8>() {
                    if let Some(codec) = match pt {
                        0 => Some(G711Codec::Pcmu),
                        8 => Some(G711Codec::Pcma),
                        _ => None,
                    } {
                        if !codecs.contains(&codec) {
                            codecs.push(codec);
                        }
                    }
                }
            }
        }
    }
    codecs
}

fn pick_codec(preferred: &[String], offered: &[G711Codec]) -> Option<G711Codec> {
    for name in preferred {
        if let Some(codec) = G711Codec::from_name(name)
            && offered.contains(&codec)
        {
            return Some(codec);
        }
    }
    offered.first().copied()
}

fn g711_codec_name(codec: G711Codec) -> &'static str {
    match codec {
        G711Codec::Pcmu => "PCMU",
        G711Codec::Pcma => "PCMA",
    }
}

fn build_sdp_answer(
    addr: &str,
    rtp_port: u16,
    codec: G711Codec,
    telephone_event_pt: Option<u8>,
) -> String {
    let pt = codec.payload_type();
    let name = g711_codec_name(codec);
    let (audio_pts, extra_rtpmap) = match telephone_event_pt {
        Some(te_pt) => (
            format!("{pt} {te_pt}"),
            format!("a=rtpmap:{te_pt} telephone-event/8000\r\na=fmtp:{te_pt} 0-15\r\n"),
        ),
        None => (pt.to_string(), String::new()),
    };
    format!(
        "v=0\r\n\
         o=- 0 0 IN IP4 {addr}\r\n\
         s=-\r\n\
         c=IN IP4 {addr}\r\n\
         t=0 0\r\n\
         m=audio {rtp_port} RTP/AVP {audio_pts}\r\n\
         a=rtpmap:{pt} {name}/8000\r\n\
         {extra_rtpmap}\
         a=sendrecv\r\n"
    )
}

fn sip_reason_phrase(code: u16) -> &'static str {
    match code {
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        408 => "Request Timeout",
        480 => "Temporarily Unavailable",
        486 => "Busy Here",
        487 => "Request Terminated",
        488 => "Not Acceptable Here",
        503 => "Service Unavailable",
        _ if (400..500).contains(&code) => "Client Error",
        _ if (500..600).contains(&code) => "Server Error",
        _ if code >= 600 => "Global Failure",
        _ => "OK",
    }
}

struct InboundSessionHandler {
    session_id: String,
    sessions: Arc<Mutex<HashMap<String, InboundSessionState>>>,
    events: Arc<Mutex<Option<SipBackendEventSender>>>,
    answered: Arc<AtomicBool>,
    answered_with_2xx: Arc<AtomicBool>,
    // on_closed is invoked on libre's re_main thread (non-tokio); we need a
    // tokio handle to defer the InboundSipSession drop off this call stack.
    runtime: Handle,
}

impl libre::InboundSipSessionEventHandler for InboundSessionHandler {
    fn on_established(&self, sip_status: u16) {
        log::info!(
            "inbound SIP session {} established status={}",
            self.session_id,
            sip_status
        );
    }

    fn on_closed(&self, error: i32, sip_status: u16) {
        // CAS-claim so a concurrent inbound_answer/reject can't race us. After this
        // returns true, the sipsess belongs to libre's teardown path; we must not
        // call answer/progress/reject on it.
        let already_finalized = self
            .answered
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err();
        let session_id = self.session_id.clone();
        // Defer the map removal: dropping the InboundSipSession drops `self`'s
        // backing Arc, and mem_deref'ing the sipsess inside libre's own close
        // callback is unsafe. Hand the state to a background task so the drop
        // happens after we return to libre and after this callback frame exits.
        let removed = self.sessions.lock().remove(&session_id);
        if let Some(state) = removed {
            self.runtime.spawn(async move {
                drop(state);
            });
        }
        let was_answered = self.answered_with_2xx.load(Ordering::SeqCst);
        log::info!(
            "inbound SIP session {session_id} closed err={error} status={sip_status} finalized={already_finalized} answered={was_answered}"
        );
        let Some(sender) = self.events.lock().clone() else {
            return;
        };
        if !already_finalized {
            // Pre-answer close (CANCEL or transport-level drop) — we hadn't
            // claimed the session yet, so MSC still needs to unwind setup.
            let _ = sender.send(SipBackendEvent::InboundCancel { session_id });
        } else if was_answered {
            // Post-answer close — trunk-initiated BYE on an established dialog.
            // Tell MSC to release the MS side.
            let _ = sender.send(SipBackendEvent::Released {
                session_id,
                reason: ReleaseReason::SipReleased,
            });
        }
        // else: MSC-initiated teardown (reject / 408 watchdog / release_call) —
        // the caller already drove the unwind on its side; no event needed.
    }
}
