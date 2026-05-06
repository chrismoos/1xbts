# 1xBTS Port Assignments

Default localhost port assignments when running `cdma-nib`. Node transport
addresses default to `127.0.0.1` and can be overridden in config files.

## Inter-node transports

| Port  | Protocol | Direction     | Purpose                          | Config                             |
|-------|----------|---------------|----------------------------------|------------------------------------|
| 5604  | TCP      | BTS <-> BSC   | Abis signaling (spec §4.5.6.4)   | `bts.json: abis.bind_addr`, `bsc.json: abis.remote_addr` |
| 17013 | TCP      | BSC -> MSC    | A1 signaling (IOS call control)  | `msc.json: a1_listen_addr`, `--a1-addr` |
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

## External services

| Port  | Protocol | Component  | Purpose                          | Config                             |
|-------|----------|------------|----------------------------------|------------------------------------|
| 17015 | gRPC     | Voice GW   | SIP/media bridge (cdma-voice-gw) | `msc.json: voice.gateway.endpoint` |
| 17010 | UDP      | PDSN (remote) | FOU tunnel remote endpoint    | `pdsn.json: packet.fou_remote`     |
| 17011 | UDP      | PDSN (local)  | FOU tunnel local port         | `pdsn.json: packet.fou_local_port` |
| 45432 | TCP      | PostgreSQL | HLR + SMSC database (`1xbts`)    | `hlr.json: postgres_dsn`, `smsc.json: postgres_dsn` |
| 3000  | HTTP     | 1xbts-web   | Web dashboard dev server         | `docker-compose.yml: 1xbts-web`, `ONEXBTS_WEB_PORT` |

## Quick reference

```
BSC gRPC mgmt ........ 127.0.0.1:17016
MSC gRPC mgmt ........ 127.0.0.1:17017
HLR gRPC ............. 127.0.0.1:17019
SMSC gRPC ............ 127.0.0.1:17020
Packet gRPC .......... 127.0.0.1:17021
MSC A1 signaling ..... 127.0.0.1:17013
Abis signaling ....... 127.0.0.1:5604
Abis bearer BTS ...... 127.0.0.1:17014
Abis bearer BSC ...... 127.0.0.1:17022
Voice gateway ........ 127.0.0.1:17015
PDSN FOU remote ...... 127.0.0.1:17010
PDSN FOU local ....... 127.0.0.1:17011
FOU-TCP relay ........ 127.0.0.1:17012
PostgreSQL ........... localhost:45432
Web dashboard ........ http://localhost:3000 (or `ONEXBTS_WEB_PORT`)
Tokio console ........ 127.0.0.1:17018
```
