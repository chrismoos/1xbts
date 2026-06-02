# HEAD 

- Added OTASP (`*228`) over-the-air provisioning. Dial `*228` to
  program the handset's CDMA/Analog NAM (IMSI, MDN, home system
  banner), MMS URI, and Preferred Roaming List (classic and
  extended). PRL can be managed in the web interface. Writes are
  off by default — set
  `otasp.writes.{cdma_analog_nam,mdn,cdma_nam,home_system_tag,mms_uri,prl}`
  in `config/msc.json` to enable.
- Fixed a LimeSDR crash on BTS shutdown. Fixes #19.
- Added initial support for Mobile IP via `packet.mobile_ip.enabled` for
  devices that use it instead of Simple IP.
- MT SMS to an unprovisioned phone number or with a payload that won't
  fit on A1 ADDS Page is marked Failed instead of looping in the retry
  sweep forever.
- Concurrent SMS to the same subscriber are queued instead of rejected,
  so MSC retry sweeps no longer drop submissions when an earlier SMS is
  still being delivered.
- Optional MMS support via an Mbuni MMSC. Enable with
  `docker compose --profile mms up` and configure handsets with MMSC URL
  `http://mmsc.local.1xbts.org/`. Migration: set `mgmt_grpc_addr` in
  `config/msc.json` to `0.0.0.0:17017`.
- Captive DNS redirects well-known carrier MMSC hostnames to the local
  MMSC, so handsets with a baked-in carrier MMSC reach the cell MMSC
  without reprovisioning. Override with `MMSC_HIJACK_HOSTS`. HTTP only.
- MMS messages are attributed to the real subscriber phone number
  rather than a NAT'd IP. Migration: set `grpc_listen_addr` in
  `config/management.json` to `0.0.0.0:17016`.
- Oversized SMS escalates from the paging channel to an SO6 traffic
  channel and delivers on F-DSCH instead of failing.
- MS-to-MS SMS now delivers to the destination subscriber.
- MT SMS for an offline subscriber is queued and delivered when the
  subscriber next registers.
- MT SMS delivered on an active traffic channel is now acknowledged
  back to the MSC, so submissions no longer get stuck in retry.
- MSC recovers SMS submissions left mid-page across a restart.
- Message log decodes WAP Push bursts and no longer renders binary
  bearer-data bytes as if they were SMS text.
- SMSC list shows MMS rows with inline decode (Transaction-ID,
  Content-Location, Size, Expiry, From). Click a row to expand.
- `/mobiles/[id]` and `/subscribers/[id]` show a Recent Messages card.
- `MscManagementService.SendSms` accepts an optional teleservice ID
  and binary user data, for WAP Push and other non-text teleservices.
- PPP packet data sessions now support peer-requested Van Jacobson TCP/IP header
  compression. PDSN-originated VJ requests are opt-in with
  `pdsn.packet.enable_vj_compression_default` (default `false`).
- Traffic-channel SMS and packet-data originations now keep A1 call context, so
  MO SMS can resolve the originating subscriber on the MSC.
- DTMF pressed during a call is now delivered to the SIP peer.
  Migration: `dtmf_mode` in `config/voice-gw.json` now defaults to
  `"rfc2833"`. Set it to `"disabled"` to disable DTMF forwarding.
- Registrations now require a complete mobile identity (IMSI+ESN or
  IMSI+MEID) for HLR resolution and welcome-SMS gating. Subscriber
  create/update in the web UI requires the same.
- P_REV 11 ESPM support (EXT_PREF_MSID_TYPE, MEID_REQD) and the SPM
  P_REV 6/7/8 tail fields are implemented but not enabled in the
  shipped `config/bts.json`. To enable, set the overhead and ESPM
  `p_rev` to 11 and fill in `ext_pref_msid_type` and `meid_reqd`.
  Not fully supported yet: some mobiles do not register cleanly at
  P_REV 11, leave it at 6 for production.
- Reverse access-channel SMS Data Burst messages are now handled by the BSC.
- Access-channel probe defaults use higher initial and nominal power for SDRs
  with lower RX sensitivity.
- Legacy IS-95 voice calls complete setup by sending a single Service Option
  Response and labeling it correctly in traffic events.
- Inbound SIP: the voice gateway accepts trunk `INVITE`s and routes them
  to subscribers by matching the Request-URI user against
  `subscriber.phone_number`. Unknown → 404, not registered → 480, busy →
  486, caller cancels → 487, MSC timeout → 408
  (`sip.inbound_decision_timeout_ms`, default `30000`).
- MSC plays ringback toward the SIP caller (subscriber's HLR ringtone or
  NANP fallback) with early-media SDP in `183 Session Progress`.
  `voice.inbound_sip_msc_ringback` (default `true`) controls it.
- Hang-up clears immediately in both directions: mobile on-hook sends
  `BYE` on the trunk; trunk `BYE` releases the mobile leg without
  waiting for BSC inactivity.
- MS-MS calls now display the calling party number on the callee.
- MT page hunt: MSC keeps re-paging on BSC page-response timeout
  (and when the target IMSI isn't in BSC's volatile registry) until the
  MS answers or the hunt window expires. SIP callers get 480 on
  giveup. `voice.page_retry_cooldown_ms` (default `1000`),
  `voice.page_retry_max_duration_ms` (default `60000`, must be `> 0`).
- New `voice.generate_ringback` (default `true`): MSC plays bearer-side
  ringback audio toward the caller MS.
- New `voice.send_tones_alert` (default `false`): MSC sends the air-side
  Ringback Signal IE so the MS plays its own ringback. Independent of
  `generate_ringback` — either, both, or neither are valid.
- `voice.sip_ringback_disable` (default `false`): when `true`, disables
  MSC-generated ringback early media for SIP-routed calls so the SIP side
  can provide ringback.
- SIP voice-gateway failures play a busy tone on the F-TCH before
  clearing. `voice.failure_tone_duration_ms` (default `3000`,
  `0` disables).

- Select your CDMA carrier with a single `channel` block (band class,
  subclass, channel number) — TX/RX frequencies and overhead fields
  are derived automatically. All 23 C.S0057-F band classes supported.

  Migration:
  - Add to `config/bts.json`:
    ```json
    "channel": { "band_class": "bc0", "band_subclass": 0, "cdma_channel": 384 }
    ```
  - In `config/bts.json`, remove `runtime.tx_center_frequency_hz`.
  - In each `config/radio_*.json`, remove `rx_freq_hz` (or rename to
    `rx_freq_hz_override` if you actually need a non-derived RX tune).
  - In `config/bsc.json`, drop `overhead.cdma_freq` and `ext_cdma_freq`
    if they were `0` (or set them to `null`).
  - In `config/bsc.json`, you can also drop
    `paging.message_defaults.cdma_channel_list.channels` — when empty,
    the operating channel is broadcast automatically.
