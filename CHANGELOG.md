# HEAD 

- SIP voice-gateway failures play a busy tone on the FTCH before clearing.
- SIP `INVITE` is sent only after the mobile is on the traffic channel.
- `voice.failure_tone_duration_ms` (default `3000`, `0` disables).
- `voice.sip_ringback_disable` (default `false`): skip MSC ringback for
  SIP-routed calls; let the SIP side provide ringback / early media.

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
