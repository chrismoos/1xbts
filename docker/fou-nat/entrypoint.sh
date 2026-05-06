#!/bin/sh
set -eu

FOU_LISTEN_PORT="${FOU_LISTEN_PORT:-5555}"
FOU_REMOTE_PORT="${FOU_REMOTE_PORT:-5556}"
FOU_TCP_PORT="${FOU_TCP_PORT:-5557}"
MOBILE_CIDR="${MOBILE_CIDR:-10.55.0.0/24}"
TUNNEL_ADDR="${TUNNEL_ADDR:-10.55.0.1/24}"
FOU_LOCAL_IP="${FOU_LOCAL_IP:-127.0.0.1}"

sysctl -w net.ipv4.ip_forward=1 >/dev/null
sysctl -w net.ipv4.conf.all.rp_filter=0 >/dev/null
sysctl -w net.ipv4.conf.default.rp_filter=0 >/dev/null

ip link del fou0 2>/dev/null || true
ip fou del port "$FOU_LISTEN_PORT" 2>/dev/null || true

ip fou add port "$FOU_LISTEN_PORT" ipproto 4
ip link add fou0 type ipip \
  local "$FOU_LOCAL_IP" \
  remote "$FOU_LOCAL_IP" \
  encap fou \
  encap-sport "$FOU_LISTEN_PORT" \
  encap-dport "$FOU_REMOTE_PORT"
ip addr add "$TUNNEL_ADDR" dev fou0
ip link set fou0 up

iptables -t nat -C POSTROUTING -s "$MOBILE_CIDR" -o eth0 -j MASQUERADE 2>/dev/null \
  || iptables -t nat -A POSTROUTING -s "$MOBILE_CIDR" -o eth0 -j MASQUERADE
iptables -C FORWARD -i fou0 -o eth0 -s "$MOBILE_CIDR" -j ACCEPT 2>/dev/null \
  || iptables -A FORWARD -i fou0 -o eth0 -s "$MOBILE_CIDR" -j ACCEPT
iptables -C FORWARD -i eth0 -o fou0 -d "$MOBILE_CIDR" -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT 2>/dev/null \
  || iptables -A FORWARD -i eth0 -o fou0 -d "$MOBILE_CIDR" -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
echo "FOU NAT ready: fou_udp=127.0.0.1:${FOU_LISTEN_PORT} relay_tcp=0.0.0.0:${FOU_TCP_PORT} relay_udp_bind=127.0.0.1:${FOU_REMOTE_PORT} tunnel=fou0 ${TUNNEL_ADDR} nat=${MOBILE_CIDR}"

/usr/local/bin/fou-tcp-relay \
  --tcp-port "$FOU_TCP_PORT" \
  --udp-bind-port "$FOU_REMOTE_PORT" \
  --udp-target-host 127.0.0.1 \
  --udp-target-port "$FOU_LISTEN_PORT" &
relay_pid=$!

trap 'kill "$relay_pid" 2>/dev/null || true; wait "$relay_pid" 2>/dev/null || true' INT TERM EXIT
wait "$relay_pid"
