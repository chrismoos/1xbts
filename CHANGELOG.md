# HEAD 

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
