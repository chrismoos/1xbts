#!/usr/bin/env python3
import argparse
import ipaddress
import os
import random
import socket
import struct
import sys
import time


TCP_FIN = 0x01
TCP_SYN = 0x02
TCP_RST = 0x04
TCP_PSH = 0x08
TCP_ACK = 0x10


def checksum16(*parts: bytes) -> int:
    total = 0
    for part in parts:
        if len(part) % 2:
            part += b"\x00"
        for i in range(0, len(part), 2):
            total += (part[i] << 8) | part[i + 1]
            total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def ipv4_header(src_ip: str, dst_ip: str, proto: int, payload: bytes, ident: int, df: bool = True) -> bytes:
    version_ihl = 0x45
    total_len = 20 + len(payload)
    flags_fragment = 0x4000 if df else 0
    header = struct.pack(
        "!BBHHHBBH4s4s",
        version_ihl,
        0,
        total_len,
        ident & 0xFFFF,
        flags_fragment,
        64,
        proto,
        0,
        socket.inet_aton(src_ip),
        socket.inet_aton(dst_ip),
    )
    csum = checksum16(header)
    return header[:10] + struct.pack("!H", csum) + header[12:]


def tcp_checksum(src_ip: str, dst_ip: str, segment: bytes) -> int:
    pseudo = struct.pack(
        "!4s4sBBH",
        socket.inet_aton(src_ip),
        socket.inet_aton(dst_ip),
        0,
        socket.IPPROTO_TCP,
        len(segment),
    )
    return checksum16(pseudo, segment)


def build_tcp_packet(
    src_ip: str,
    dst_ip: str,
    src_port: int,
    dst_port: int,
    seq: int,
    ack: int,
    flags: int,
    payload: bytes = b"",
    window: int = 65535,
    options: bytes = b"",
    ident: int = 0,
) -> bytes:
    if len(options) % 4:
        raise ValueError("TCP options must be 32-bit aligned")
    data_offset = 5 + len(options) // 4
    tcp_header = struct.pack(
        "!HHLLBBHHH",
        src_port,
        dst_port,
        seq & 0xFFFFFFFF,
        ack & 0xFFFFFFFF,
        (data_offset << 4),
        flags,
        window,
        0,
        0,
    )
    tcp_header += options
    csum = tcp_checksum(src_ip, dst_ip, tcp_header + payload)
    tcp_header = tcp_header[:16] + struct.pack("!H", csum) + tcp_header[18:]
    return ipv4_header(src_ip, dst_ip, socket.IPPROTO_TCP, tcp_header + payload, ident) + tcp_header + payload


def parse_ipv4(packet: bytes) -> dict:
    if len(packet) < 20:
        raise ValueError(f"short IPv4 packet: {len(packet)}")
    version_ihl, tos, total_len, ident, frag, ttl, proto, hdr_csum, src, dst = struct.unpack(
        "!BBHHHBBH4s4s", packet[:20]
    )
    version = version_ihl >> 4
    ihl = (version_ihl & 0x0F) * 4
    if version != 4 or len(packet) < ihl:
        raise ValueError(f"bad IPv4 packet: version={version} ihl={ihl} len={len(packet)}")
    payload = packet[ihl:total_len]
    hdr = bytearray(packet[:ihl])
    hdr[10:12] = b"\x00\x00"
    ip_ok = checksum16(bytes(hdr)) == hdr_csum
    return {
        "ihl": ihl,
        "total_len": total_len,
        "id": ident,
        "proto": proto,
        "src_ip": socket.inet_ntoa(src),
        "dst_ip": socket.inet_ntoa(dst),
        "payload": payload,
        "ip_checksum_rx": hdr_csum,
        "ip_checksum_ok": ip_ok,
    }


def parse_tcp(packet: bytes) -> dict:
    ip = parse_ipv4(packet)
    if ip["proto"] != socket.IPPROTO_TCP:
        raise ValueError(f"not TCP proto={ip['proto']}")
    payload = ip["payload"]
    if len(payload) < 20:
        raise ValueError(f"short TCP segment: {len(payload)}")
    src_port, dst_port, seq, ack, off_flags_hi, flags, window, tcp_csum, urg = struct.unpack(
        "!HHLLBBHHH", payload[:20]
    )
    offset = (off_flags_hi >> 4) * 4
    if len(payload) < offset:
        raise ValueError(f"bad TCP offset={offset} len={len(payload)}")
    tcp_header = bytearray(payload[:offset])
    tcp_header[16:18] = b"\x00\x00"
    calc = tcp_checksum(ip["src_ip"], ip["dst_ip"], bytes(tcp_header) + payload[offset:])
    return {
        **ip,
        "src_port": src_port,
        "dst_port": dst_port,
        "seq": seq,
        "ack": ack,
        "flags": flags,
        "window": window,
        "tcp_checksum_rx": tcp_csum,
        "tcp_checksum_calc": calc,
        "tcp_checksum_ok": calc == tcp_csum,
        "tcp_payload": payload[offset:],
    }


def flags_text(flags: int) -> str:
    out = []
    if flags & TCP_SYN:
        out.append("S")
    if flags & TCP_ACK:
        out.append("A")
    if flags & TCP_FIN:
        out.append("F")
    if flags & TCP_RST:
        out.append("R")
    if flags & TCP_PSH:
        out.append("P")
    return "".join(out) or "-"


class FouTcpClient:
    def __init__(self, host: str, port: int, timeout: float):
        self.sock = socket.create_connection((host, port), timeout=timeout)
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        self.sock.settimeout(timeout)

    def close(self):
        self.sock.close()

    def send_packet(self, packet: bytes):
        self.sock.sendall(struct.pack("!H", len(packet)) + packet)

    def recv_packet(self) -> bytes:
        hdr = self._recv_exact(2)
        (length,) = struct.unpack("!H", hdr)
        return self._recv_exact(length)

    def _recv_exact(self, size: int) -> bytes:
        buf = bytearray()
        while len(buf) < size:
            chunk = self.sock.recv(size - len(buf))
            if not chunk:
                raise EOFError("fou_tcp closed")
            buf.extend(chunk)
        return bytes(buf)


def describe_tcp(info: dict) -> str:
    return (
        f"{info['src_ip']}:{info['src_port']} -> {info['dst_ip']}:{info['dst_port']} "
        f"flags={flags_text(info['flags'])} seq={info['seq']} ack={info['ack']} "
        f"payload={len(info['tcp_payload'])} ip_csum={'ok' if info['ip_checksum_ok'] else 'bad'} "
        f"tcp_csum={'ok' if info['tcp_checksum_ok'] else f'bad(rx=0x{info['tcp_checksum_rx']:04x} calc=0x{info['tcp_checksum_calc']:04x})'}"
    )


def resolve_target(host: str) -> str:
    return socket.gethostbyname(host)


def run_http_session(
    client: FouTcpClient,
    src_ip: str,
    host: str,
    port: int,
    timeout: float,
    ignore_bad_checksum: bool,
) -> dict:
    dst_ip = resolve_target(host)
    src_port = random.randint(40000, 60000)
    client_seq = random.randint(0, 0xFFFFFFFF)
    ident = random.randint(0, 0xFFFF)
    syn_opts = (
        b"\x02\x04\x05\x6c"  # MSS 1388
        b"\x01"
        b"\x03\x03\x08"      # WS=8
        b"\x01\x01"
        b"\x04\x02"          # SACK permitted
    )
    print(f"\n== {host}:{port} via {dst_ip} src={src_ip}:{src_port} ==", flush=True)
    syn = build_tcp_packet(
        src_ip, dst_ip, src_port, port, client_seq, 0, TCP_SYN, options=syn_opts, ident=ident
    )
    client.send_packet(syn)
    print("TX", f"{src_ip}:{src_port} -> {dst_ip}:{port} flags=S seq={client_seq}", flush=True)

    deadline = time.time() + timeout
    server_seq = None
    http_data = bytearray()
    sent_get = False
    client_next = (client_seq + 1) & 0xFFFFFFFF
    server_next = None
    saw_synack = False

    while time.time() < deadline:
        packet = client.recv_packet()
        try:
            tcp = parse_tcp(packet)
        except Exception as err:
            print("RX non-TCP/parse-error", err, flush=True)
            continue
        if tcp["src_ip"] != dst_ip or tcp["dst_ip"] != src_ip:
            print("RX other", describe_tcp(tcp), flush=True)
            continue
        if tcp["src_port"] != port or tcp["dst_port"] != src_port:
            print("RX other", describe_tcp(tcp), flush=True)
            continue

        print("RX", describe_tcp(tcp), flush=True)
        if tcp["flags"] & TCP_RST:
            return {"ok": False, "reason": "rst", "host": host, "dst_ip": dst_ip}
        if (not tcp["ip_checksum_ok"] or not tcp["tcp_checksum_ok"]) and not ignore_bad_checksum:
            return {"ok": False, "reason": "bad_checksum", "host": host, "dst_ip": dst_ip}

        if (tcp["flags"] & (TCP_SYN | TCP_ACK)) == (TCP_SYN | TCP_ACK) and server_seq is None:
            saw_synack = True
            server_seq = tcp["seq"]
            server_next = (server_seq + 1) & 0xFFFFFFFF
            ack = build_tcp_packet(
                src_ip, dst_ip, src_port, port, client_next, server_next, TCP_ACK, ident=ident + 1
            )
            client.send_packet(ack)
            print("TX", f"{src_ip}:{src_port} -> {dst_ip}:{port} flags=A seq={client_next} ack={server_next}", flush=True)

            req = (
                f"GET / HTTP/1.1\r\nHost: {host}\r\nConnection: close\r\nUser-Agent: fou-tcp-test/1\r\n\r\n"
            ).encode()
            get_pkt = build_tcp_packet(
                src_ip,
                dst_ip,
                src_port,
                port,
                client_next,
                server_next,
                TCP_ACK | TCP_PSH,
                payload=req,
                ident=ident + 2,
            )
            client.send_packet(get_pkt)
            print(
                "TX",
                f"{src_ip}:{src_port} -> {dst_ip}:{port} flags=AP seq={client_next} ack={server_next} payload={len(req)}",
                flush=True,
            )
            client_next = (client_next + len(req)) & 0xFFFFFFFF
            sent_get = True
            continue

        if server_next is None:
            continue

        seg_len = len(tcp["tcp_payload"])
        if tcp["flags"] & TCP_SYN:
            seg_len += 1
        if tcp["flags"] & TCP_FIN:
            seg_len += 1
        if seg_len:
            if tcp["seq"] == server_next:
                server_next = (server_next + seg_len) & 0xFFFFFFFF
                http_data.extend(tcp["tcp_payload"])
            ack_pkt = build_tcp_packet(
                src_ip, dst_ip, src_port, port, client_next, server_next, TCP_ACK, ident=ident + 3
            )
            client.send_packet(ack_pkt)
            print(
                "TX",
                f"{src_ip}:{src_port} -> {dst_ip}:{port} flags=A seq={client_next} ack={server_next}",
                flush=True,
            )

        if sent_get and http_data.startswith(b"HTTP/1."):
            header_end = http_data.find(b"\r\n\r\n")
            if header_end != -1:
                status_line = http_data.split(b"\r\n", 1)[0].decode("latin1", errors="replace")
                return {
                    "ok": True,
                    "host": host,
                    "dst_ip": dst_ip,
                    "status_line": status_line,
                    "bytes": len(http_data),
                    "saw_synack": saw_synack,
                }

    return {
        "ok": False,
        "host": host,
        "dst_ip": dst_ip,
        "reason": "timeout",
        "saw_synack": saw_synack,
        "bytes": len(http_data),
    }


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--host", dest="hosts", action="append", help="HTTP host to test", default=[])
    parser.add_argument("--relay-host", default="127.0.0.1")
    parser.add_argument("--relay-port", type=int, default=5557)
    parser.add_argument("--src-ip", default="10.55.0.250")
    parser.add_argument("--timeout", type=float, default=10.0)
    parser.add_argument("--ignore-bad-checksum", action="store_true")
    args = parser.parse_args()

    if not args.hosts:
        args.hosts = ["neverssl.com", "example.com", "detectportal.firefox.com"]
    ipaddress.IPv4Address(args.src_ip)

    client = FouTcpClient(args.relay_host, args.relay_port, args.timeout)
    try:
        results = []
        for host in args.hosts:
            try:
                results.append(
                    run_http_session(
                        client,
                        args.src_ip,
                        host,
                        80,
                        args.timeout,
                        args.ignore_bad_checksum,
                    )
                )
            except Exception as err:
                results.append({"ok": False, "host": host, "reason": str(err)})

        print("\n== summary ==", flush=True)
        ok = True
        for result in results:
            if result.get("ok"):
                print(
                    f"OK   host={result['host']} ip={result['dst_ip']} status={result['status_line']} bytes={result['bytes']}",
                    flush=True,
                )
            else:
                ok = False
                detail = " ".join(
                    f"{key}={value}" for key, value in result.items() if key not in {"ok", "host"}
                )
                print(f"FAIL host={result['host']} {detail}".strip(), flush=True)
        return 0 if ok else 1
    finally:
        client.close()


if __name__ == "__main__":
    sys.exit(main())
