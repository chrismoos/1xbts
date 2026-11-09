# 1xBTS Port Assignments

Default localhost port assignments when running `cdma-nib`. Node transport
addresses default to `127.0.0.1` and can be overridden in config files.

## Inter-node transports

Out of the box, every node runs on `127.0.0.1` on ports that need no root, so
one machine works with no extra setup. When nodes run on separate machines, or
connect to another vendor's equipment, use the standard port from the Spec
column.

| Interface | Protocol | Single-host default | Spec port | Config |
|-----------|----------|---------------------|-----------|--------|
| Abis signaling | TCP | BTS `127.0.0.1:5604` | 5604 (A.S0003-A §4.5.6.4) | `bts.json: abis.bind_addr`, `bsc.json: bts_peers[].abis_addr` |
| Abis traffic | UDP | BSC `127.0.0.1:17022`, BTS one ephemeral port per traffic connection | none. A.S0003-A §4.5.6.2 leaves the ports to the operator. The BTS reports each port as the Traffic Circuit Identifier in Abis-Connect | `bsc.json: bts_peers[].bearer_bind_addr`, `bts.json: bearer.remote_addr`. The ports in `bts.json: bearer.bind_addr` and `bsc.json: bts_peers[].bearer_remote_addr` are ignored |
| A1p | SCTP | BSC `127.0.0.1:14001` | 14001 (A.S0012-D §3.2.2) | `bsc.json: a1_bind_addr`, `bsc.json: a1p`, `msc.json: base_stations[].a1p`, `femto.json: fcs.a1_listen` |
| A1p fallback | UDP | BSC 9899, MSC 9900 | 9899 (RFC 6951) on both ends | `a1p.sctp.fallback_udp_encapsulation`, or `a1p.sctp.udp_encapsulation` to always use it |
| A2p | RTP over UDP | MSC and BSC: one ephemeral port per call on `voice_bearer_bind_ip` (Linux default range 32768-60999). Femto FCS: 18000-18199 | none. Each end sends its address and port in the A2p Bearer Session-Level Parameters of Assignment Request and Assignment Complete (A.S0014-D §4.2.89) | `msc.json: voice.voice_bearer_bind_ip`, `bsc.json: voice_bearer_bind_ip`, `femto.json: fcs.rtp_msc_bind_ip`, `fcs.rtp_port_start`, `fcs.rtp_port_end` |
| Femto RTP | RTP over UDP | FCS 18000-18199, shared with its A2p ports | none. Negotiated in SIP SDP with the FAP | `femto.json: fcs.rtp_bind_ip`, `fcs.rtp_port_start`, `fcs.rtp_port_end` |
| A9 | UDP or TCP | PCF `127.0.0.1:5603` | 5603 (A.S0012-D §3.4) | `pcf.json: a9_bind_addr`, `pcf.json: a9_transport`, `bts.json: evdo.a9.pcf_addr`, `bts.json: evdo.a9.a9_transport`, `bsc.json: packet_a9.pcf_addr` |
| A11 | UDP | PCF `127.0.0.1:17044`, PDSN `127.0.0.1:17045` | 699 on both ends (A.S0012-D §3.5) | `pcf.json: a11`, `pdsn.json: a11` |
| A8 | GRE-in-UDP | EV-DO AN `127.0.0.1:17040`, BSC `127.0.0.1:17048`, PCF `127.0.0.1:17041` | native GRE, IP protocol 47 (A.S0012-D §2.6). GRE-in-UDP uses 4754 (RFC 8086) | `pcf.json: a8_bearer`, `bts.json: evdo.a9`, `bsc.json: packet_a9` |
| A10 | GRE-in-UDP | PCF `127.0.0.1:17042`, PDSN `127.0.0.1:17043` | native GRE, IP protocol 47 (A.S0012-D §2.6). GRE-in-UDP uses 4754 (RFC 8086) | `pcf.json: a10_bearer`, `pdsn.json: a10_bearer` |
| A21 | UDP | BSC `127.0.0.1:4597`, EV-DO AN `127.0.0.1:17049` | 4597 on both ends (A.S0008-C §2.8.1) | `bsc.json: a21`, `bts.json: evdo.a21` |
| A12 | UDP | PDSN `1812` | 1812 | `pdsn.json: a12.listen` |
| SMPP | TCP | 2775 | 2775 | `smsc.json: smpp.bind_addr`, `smsc.json: smpp.hub_addr` |

The BSC's 1x packet data uses A9 to the PCF and A8 from `127.0.0.1:17048`
(`bsc.json: packet_a9`). Its `a8_bind_addr` must differ from the EV-DO AN's A8
address.

### Standard ports between hosts

Port 699 needs `CAP_NET_BIND_SERVICE` on Linux, and native GRE needs
`CAP_NET_RAW` on Linux or root on macOS:

```sh
sudo setcap cap_net_bind_service,cap_net_raw+ep target/release/cdma-nib
```

Set `mode: "raw_gre"` on `pcf.json` `a8_bearer` and `a10_bearer`, on
`pdsn.json` `a10_bearer`, and in `bts.json` `evdo.a9.a8_transport`. With native
GRE only the IP addresses of the bearer settings are used. To run nodes on
standard ports on one host, give each node its own address, for example PCF
`127.0.0.2`, PDSN `127.0.0.3` and EV-DO AN `127.0.0.4`. Linux answers on all of
`127.0.0.0/8`. macOS needs `sudo ifconfig lo0 alias 127.0.0.N up` for each one.

### SCTP

A1p uses the Linux SCTP module when it is loaded (`sudo modprobe sctp`).
Otherwise it uses a built-in SCTP stack over UDP (macOS always uses this).
The MSC and the BSC must both use regular SCTP or both use SCTP over UDP. The
two cannot be mixed.

## Management gRPC

| Port  | Protocol | Component | Purpose                           | Config / CLI                       |
|-------|----------|-----------|-----------------------------------|------------------------------------|
| 17016 | gRPC     | BSC       | BSC management (web UI, CLI ops)  | `management.json: grpc_listen_addr` |
| 17017 | gRPC     | MSC       | MSC management (initiate_call, list_calls) | `msc.json: mgmt_grpc_addr`, `--msc-mgmt-addr` |
| 17019 | gRPC     | HLR       | HLR subscriber and registration service | `hlr.json: grpc_listen_addr` |
| 17020 | gRPC     | SMSC      | SMS submission and delivery service | `smsc.json: grpc_listen_addr` |
| 17021 | gRPC     | PDSN        | Packet session status for management | `pdsn.json: packet_grpc_listen_addr`, `msc.json: packet_endpoint` |
| 17023 | gRPC     | Event bus | Aggregated network-event bus     | `events.json: grpc_listen_addr` |
| 17030 | gRPC     | AN        | HRPD AN session/UATI service, hosted by the BTS | `bts.json: evdo.an_grpc_bind_addr` |
| 17047 | gRPC     | PCF       | PCF management: A10 connections | `pcf.json: management_bind_addr`, `msc.json: pcf_endpoint` |

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
PCF management gRPC .. 127.0.0.1:17047
BSC A1p signaling .... 127.0.0.1:14001 (SCTP, or UDP 9899/9900 fallback)
Abis signaling ....... 127.0.0.1:5604
Abis bearer BTS ...... 127.0.0.1, one ephemeral port per traffic connection
Abis bearer BSC ...... 127.0.0.1:17022
MSC/BSC A2p voice .... 127.0.0.1, one ephemeral port per call
Femto FCS RTP/A2p .... 18000-18199
PCF A9 signaling ..... 127.0.0.1:5603
AN A8 bearer ......... 127.0.0.1:17040 (spec: native GRE)
PCF A8 bearer ........ 127.0.0.1:17041 (spec: native GRE)
BSC A8 bearer ........ 127.0.0.1:17048 (spec: native GRE)
PCF A10 bearer ....... 127.0.0.1:17042 (spec: native GRE)
PDSN A10 bearer ...... 127.0.0.1:17043 (spec: native GRE)
PCF A11 signaling .... 127.0.0.1:17044 (spec: 699)
PDSN A11 signaling ... 127.0.0.1:17045 (spec: 699)
BSC A21 (IWS) ........ 127.0.0.1:4597
AN A21 ............... 127.0.0.1:17049 (spec: 4597)
Voice gateway ........ 127.0.0.1:17015
SMPP to hub .......... 2775 (interconnect only)
PDSN FOU remote ...... 127.0.0.1:17010
PDSN FOU local ....... 127.0.0.1:17011
FOU-TCP relay ........ 127.0.0.1:17012
PostgreSQL ........... localhost:45432
Web dashboard ........ http://localhost:3000 (or `ONEXBTS_WEB_PORT`)
Speed test ........... http://localhost:5656 (or `SPEEDTEST_PORT`)
Mobile speed URL ..... http://speed/ or http://speed.local.1xbts.org/ (DNS via packet gateway 10.55.0.1)
Tokio console ........ 127.0.0.1:17018
```
