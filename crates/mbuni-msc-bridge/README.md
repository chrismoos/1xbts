# mbuni-msc-bridge

Kannel-compatible `sendsms` HTTP shim that forwards Mbuni's MMS notifications
into `MscManagementService.SendSms`. Routing through the MSC (rather than
queuing directly in the SMSC) makes each notification page the BSC over A1 in
the same request — no SMSC retry-sweep wait.

The MMS pipeline that calls this lives at <https://1xbts.org/docs/guides/mms>.

## What this bridge handles

- Mbuni packs the WAP Push PDU into the `text` query parameter as raw
  percent-encoded bytes (not the Kannel-style hex `data` field) and doesn't
  set `coding=1`. We parse the raw query bytes ourselves so binary NULs
  survive, and treat any UDH-bearing request as binary regardless of the
  declared coding.
- Mbuni's `udh` is raw percent-encoded bytes rather than the hex string Kannel
  emits. We try hex first and fall back to raw bytes when that fails.
- The bridge re-frames the GSM-style WSP UDH for the CDMA WAP teleservice
  (0x1004) per WAP-259 §6.5 — the WSP source/destination ports are emitted
  inline at the head of the User Data subparameter (`MSG_TYPE TOTAL_SEGMENTS
  SEGMENT_NUMBER SOURCE_PORT DESTINATION_PORT DATA`); no GSM UDH on the wire.

## Environment

| Var              | Default                | Purpose                              |
| ---------------- | ---------------------- | ------------------------------------ |
| `BIND_ADDR`      | `127.0.0.1:8081`       | Local bind. Loopback in production.  |
| `MSC_GRPC_ADDR`  | `127.0.0.1:17017`      | MSC management gRPC endpoint.        |
