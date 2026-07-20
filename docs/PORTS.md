# 1xBTS Port Assignments

Default localhost port assignments when running `cdma-nib`. Node transport
addresses default to `127.0.0.1` and can be overridden in config files.

## Inter-node transports

| Port  | Protocol | Direction     | Purpose                          | Config                             |
|-------|----------|---------------|----------------------------------|------------------------------------|
| 5604  | TCP      | BTS <-> BSC   | Abis signaling (spec §4.5.6.4)   | `bts.json: abis.bind_addr`, `bsc.json: abis.remote_addr` |
| 17013 | TCP      | BSC -> MSC    | A1 signaling (IOS call control)  | `msc.json: a1_listen_addr`, `--a1-addr` |
| 17031 | TCP      | BSC <-> AN    | A21 hybrid-AT coord (cross-paging, IMSI↔UATI) | `bsc.json: an_a21_addr`, `cdma-an --a21-listen` |
| 17040 | UDP/GRE  | AN A8         | HRPD A8 bearer endpoint toward PCF | `cdma-an: a8_bearer`, `pcf.json: a8_bearer.udp_peer_addr` |
| 17041 | UDP/GRE  | PCF A8        | HRPD A8 bearer endpoint toward AN | `pcf.json: a8_bearer.udp_bind_addr` |
| 17042 | UDP/GRE  | PCF A10       | HRPD A10 bearer endpoint toward PDSN | `pcf.json: a10_bearer.udp_bind_addr` |
| 17043 | UDP/GRE  | PDSN A10      | HRPD A10 bearer endpoint toward PCF | `pdsn.json: a10_bearer.udp_bind_addr` |
| 17044 | UDP      | PCF A11       | HRPD A11 registration endpoint toward PDSN | `pcf.json: a11.bind_addr` |
| 17045 | UDP      | PDSN A11      | HRPD A11 registration endpoint toward PCF | `pdsn.json: a11.bind_addr` |
| 17046 | UDP      | PCF A9        | HRPD A9 signaling endpoint toward AN/BSC | `pcf.json: a9_bind_addr` |
| 17014 | UDP      | BTS -> BSC    | Abis bearer BTS-side (fwd frames from BSC) | `bts.json: bearer.bind_addr`, `bsc.json: bearer.remote_addr` |
| 17022 | UDP      | BSC -> BTS    | Abis bearer BSC-side (rev frames from BTS) | `bsc.json: bearer.bind_addr`, `bts.json: bearer.remote_addr` |

The default NIB mode uses localhost Abis TCP and UDP bearer transports. Split
BTS/BSC operation uses the same config fields pointed at the remote peer.

## Management gRPC

| Port  | Protocol | Component | Purpose                           | Config / CLI                       |
|-------|----------|-----------|-----------------------------------|------------------------------------|
| 17016 | gRPC     | BSC       | BSC management (web UI, CLI ops)  | `management.json: grpc_listen_addr` |
| 17017 | gRPC     | MSC       | MSC management (initiate_call, list_calls) | `msc.json: mgmt_grpc_addr`, `--msc-mgmt-addr` |
| 17019 | gRPC     | HLR       | HLR subscriber and registration service | `hlr.json: grpc_listen_addr` |
| 17020 | gRPC     | SMSC      | SMS submission and delivery service | `smsc.json: grpc_listen_addr` |
| 17021 | gRPC     | PDSN/Packet | Packet session service          | `pdsn.json: packet_grpc_listen_addr`, `pcf.json: packet_grpc_endpoint` |
| 17023 | gRPC     | Event bus | Aggregated network-event bus     | `events.json: grpc_listen_addr` |
| 17030 | gRPC     | AN        | HRPD AN session/air service used by `cdma-nib` | derived from `bts.json: evdo.overhead` |

## External services

| Port  | Protocol | Component  | Purpose                          | Config                             |
|-------|----------|------------|----------------------------------|------------------------------------|
| 17015 | gRPC     | Voice GW   | SIP/media bridge (cdma-voice-gw) | `msc.json: voice.gateway.endpoint` |
| 17010 | UDP      | PDSN (remote) | FOU tunnel remote endpoint    | `pdsn.json: packet.fou_remote`     |
| 17011 | UDP      | PDSN (local)  | FOU tunnel local port         | `pdsn.json: packet.fou_local_port` |
| 45432 | TCP      | PostgreSQL | HLR + SMSC database (`1xbts`)    | `hlr.json: postgres_dsn`, `smsc.json: postgres_dsn` |
| 3000  | HTTP     | 1xbts-web   | Web dashboard dev server         | `docker-compose.yml: 1xbts-web`, `ONEXBTS_WEB_PORT` |
| 5656  | HTTP     | Speed test  | Host access to FOU gateway proxy | `docker-compose.yml: fou-nat`, `SPEEDTEST_PORT` |
| 80    | HTTP     | FOU gateway | Mobile access to speed test (`http://speed/`, `http://speed.local.1xbts.org/`) | `docker/fou-nat`, nginx proxy |
| 53    | DNS      | FOU gateway | Mobile DNS resolver for packet data | `docker/fou-nat`, Unbound; `pdsn.json: packet.primary_dns` |

## Quick reference

```
BSC gRPC mgmt ........ 127.0.0.1:17016
MSC gRPC mgmt ........ 127.0.0.1:17017
HLR gRPC ............. 127.0.0.1:17019
SMSC gRPC ............ 127.0.0.1:17020
Packet gRPC .......... 127.0.0.1:17021
Event bus gRPC ....... 127.0.0.1:17023
AN gRPC .............. 127.0.0.1:17030
MSC A1 signaling ..... 127.0.0.1:17013
BSC <-> AN A21 ....... 127.0.0.1:17031
AN A8 bearer ......... 127.0.0.1:17040
PCF A8 bearer ........ 127.0.0.1:17041
PCF A10 bearer ....... 127.0.0.1:17042
PDSN A10 bearer ...... 127.0.0.1:17043
PCF A11 signaling .... 127.0.0.1:17044
PDSN A11 signaling ... 127.0.0.1:17045
PCF A9 signaling ..... 127.0.0.1:17046
Abis signaling ....... 127.0.0.1:5604
Abis bearer BTS ...... 127.0.0.1:17014
Abis bearer BSC ...... 127.0.0.1:17022
Voice gateway ........ 127.0.0.1:17015
PDSN FOU remote ...... 127.0.0.1:17010
PDSN FOU local ....... 127.0.0.1:17011
FOU-TCP relay ........ 127.0.0.1:17012
PostgreSQL ........... localhost:45432
Web dashboard ........ http://localhost:3000 (or `ONEXBTS_WEB_PORT`)
Speed test ........... http://localhost:5656 (or `SPEEDTEST_PORT`)
Mobile speed URL ..... http://speed/ or http://speed.local.1xbts.org/ (DNS via packet gateway 10.55.0.1)
Tokio console ........ 127.0.0.1:17018
```
