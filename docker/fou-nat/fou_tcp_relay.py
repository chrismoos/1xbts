#!/usr/bin/env python3
import argparse
import selectors
import socket
import struct
import sys


class Relay:
    def __init__(self, tcp_port: int, udp_bind_port: int, udp_target_host: str, udp_target_port: int):
        self.sel = selectors.DefaultSelector()
        self.listener = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.listener.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self.listener.bind(("0.0.0.0", tcp_port))
        self.listener.listen(1)
        self.listener.setblocking(False)
        self.sel.register(self.listener, selectors.EVENT_READ, self._accept)

        self.udp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
        self.udp.bind(("127.0.0.1", udp_bind_port))
        self.udp.setblocking(False)
        self.sel.register(self.udp, selectors.EVENT_READ, self._udp_readable)

        self.udp_target = (udp_target_host, udp_target_port)
        self.conn = None
        self.tcp_rx = bytearray()
        self.tcp_tx = bytearray()

        print(
            f"fou-tcp-relay: tcp=0.0.0.0:{tcp_port} udp_bind=127.0.0.1:{udp_bind_port} "
            f"udp_target={udp_target_host}:{udp_target_port}",
            flush=True,
        )

    def _accept(self, _sock):
        conn, addr = self.listener.accept()
        conn.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
        conn.setblocking(False)
        if self.conn is not None:
            print(f"fou-tcp-relay: replacing existing TCP client with {addr[0]}:{addr[1]}", flush=True)
            self._close_conn()
        else:
            print(f"fou-tcp-relay: accepted TCP client {addr[0]}:{addr[1]}", flush=True)
        self.conn = conn
        self.tcp_rx.clear()
        self.tcp_tx.clear()
        self.sel.register(conn, selectors.EVENT_READ, self._tcp_ready)

    def _close_conn(self):
        if self.conn is None:
            return
        try:
            self.sel.unregister(self.conn)
        except Exception:
            pass
        try:
            self.conn.close()
        except Exception:
            pass
        self.conn = None
        self.tcp_rx.clear()
        self.tcp_tx.clear()

    def _tcp_ready(self, conn):
        if self.conn is None:
            return
        try:
            if self.tcp_tx:
                sent = conn.send(self.tcp_tx)
                del self.tcp_tx[:sent]
        except (BlockingIOError, InterruptedError):
            pass
        except OSError as err:
            print(f"fou-tcp-relay: TCP write failed: {err}", flush=True)
            self._close_conn()
            return

        try:
            data = conn.recv(4096)
        except (BlockingIOError, InterruptedError):
            data = None
        except OSError as err:
            print(f"fou-tcp-relay: TCP read failed: {err}", flush=True)
            self._close_conn()
            return

        if data == b"":
            print("fou-tcp-relay: TCP client disconnected", flush=True)
            self._close_conn()
            return
        if data:
            self.tcp_rx.extend(data)
            self._drain_tcp_frames()

        events = selectors.EVENT_READ
        if self.tcp_tx:
            events |= selectors.EVENT_WRITE
        try:
            self.sel.modify(conn, events, self._tcp_ready)
        except Exception:
            self._close_conn()

    def _drain_tcp_frames(self):
        while len(self.tcp_rx) >= 2:
            frame_len = struct.unpack("!H", self.tcp_rx[:2])[0]
            if len(self.tcp_rx) < 2 + frame_len:
                return
            frame = normalize_ipv4_checksums(bytes(self.tcp_rx[2 : 2 + frame_len]))
            del self.tcp_rx[: 2 + frame_len]
            try:
                self.udp.sendto(frame, self.udp_target)
            except OSError as err:
                print(f"fou-tcp-relay: UDP send failed: {err}", flush=True)

    def _udp_readable(self, udp_sock):
        try:
            data, _addr = udp_sock.recvfrom(65535)
        except OSError as err:
            print(f"fou-tcp-relay: UDP recv failed: {err}", flush=True)
            return
        if self.conn is None:
            return
        if len(data) > 0xFFFF:
            print(f"fou-tcp-relay: dropping oversize UDP payload ({len(data)} bytes)", flush=True)
            return
        data = normalize_ipv4_checksums(data)
        self.tcp_tx.extend(struct.pack("!H", len(data)))
        self.tcp_tx.extend(data)
        try:
            self.sel.modify(self.conn, selectors.EVENT_READ | selectors.EVENT_WRITE, self._tcp_ready)
        except Exception:
            self._close_conn()

    def serve(self):
        while True:
            for key, _mask in self.sel.select():
                callback = key.data
                callback(key.fileobj)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--tcp-port", type=int, required=True)
    parser.add_argument("--udp-bind-port", type=int, required=True)
    parser.add_argument("--udp-target-host", required=True)
    parser.add_argument("--udp-target-port", type=int, required=True)
    args = parser.parse_args()

    relay = Relay(args.tcp_port, args.udp_bind_port, args.udp_target_host, args.udp_target_port)
    relay.serve()


def checksum16(parts):
    total = 0
    for part in parts:
        if len(part) & 1:
            part += b"\x00"
        for idx in range(0, len(part), 2):
            total += (part[idx] << 8) | part[idx + 1]
            total = (total & 0xFFFF) + (total >> 16)
    return (~total) & 0xFFFF


def normalize_ipv4_checksums(packet: bytes) -> bytes:
    if len(packet) < 20:
        return packet
    version_ihl = packet[0]
    version = version_ihl >> 4
    ihl = (version_ihl & 0x0F) * 4
    if version != 4 or ihl < 20 or len(packet) < ihl:
        return packet

    total_len = struct.unpack("!H", packet[2:4])[0]
    if total_len < ihl:
        return packet
    packet_len = min(len(packet), total_len)
    buf = bytearray(packet[:packet_len])

    buf[10:12] = b"\x00\x00"
    ip_csum = checksum16([bytes(buf[:ihl])])
    buf[10:12] = struct.pack("!H", ip_csum)

    proto = buf[9]
    payload = memoryview(buf)[ihl:packet_len]
    if proto == socket.IPPROTO_TCP and len(payload) >= 20:
        _normalize_l4_checksum(buf, ihl, packet_len, proto, 16)
    elif proto == socket.IPPROTO_UDP and len(payload) >= 8:
        _normalize_l4_checksum(buf, ihl, packet_len, proto, 6)
    elif proto == socket.IPPROTO_ICMP and len(payload) >= 4:
        payload = bytearray(payload)
        payload[2:4] = b"\x00\x00"
        icmp_csum = checksum16([bytes(payload)])
        buf[ihl + 2 : ihl + 4] = struct.pack("!H", icmp_csum)

    return bytes(buf)


def _normalize_l4_checksum(buf: bytearray, ihl: int, packet_len: int, proto: int, checksum_offset: int):
    segment = bytearray(buf[ihl:packet_len])
    if len(segment) < checksum_offset + 2:
        return
    segment[checksum_offset : checksum_offset + 2] = b"\x00\x00"
    pseudo = struct.pack(
        "!4s4sBBH",
        bytes(buf[12:16]),
        bytes(buf[16:20]),
        0,
        proto,
        len(segment),
    )
    csum = checksum16([pseudo, bytes(segment)])
    buf[ihl + checksum_offset : ihl + checksum_offset + 2] = struct.pack("!H", csum)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        sys.exit(0)
