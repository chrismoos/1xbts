#!/bin/sh
set -eu

FOU_LISTEN_PORT="${FOU_LISTEN_PORT:-5555}"
FOU_REMOTE_PORT="${FOU_REMOTE_PORT:-5556}"
FOU_TCP_PORT="${FOU_TCP_PORT:-5557}"
MOBILE_CIDR="${MOBILE_CIDR:-10.55.0.0/24}"
TUNNEL_ADDR="${TUNNEL_ADDR:-10.55.0.1/24}"
FOU_LOCAL_IP="${FOU_LOCAL_IP:-127.0.0.1}"
SPEEDTEST_UPSTREAM="${SPEEDTEST_UPSTREAM:-http://speedtest:5656}"
LOCAL_DNS_ZONE="${LOCAL_DNS_ZONE:-local.1xbts.org}"
UNBOUND_FORWARD_ADDRS="${UNBOUND_FORWARD_ADDRS:-${DNS_UPSTREAMS:-8.8.8.8}}"
GATEWAY_IP="${TUNNEL_ADDR%%/*}"

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

mkdir -p /run/nginx /var/log/nginx /etc/nginx/conf.d /etc/unbound
rm -f /etc/nginx/sites-enabled/default /etc/nginx/conf.d/default.conf 2>/dev/null || true

cat >/etc/nginx/conf.d/speedtest.conf <<EOF
server {
    listen 80 default_server;
    server_name speed speed.${LOCAL_DNS_ZONE};

    location / {
        proxy_pass ${SPEEDTEST_UPSTREAM};
        proxy_http_version 1.1;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$proxy_add_x_forwarded_for;
        proxy_set_header X-Forwarded-Proto http;
        proxy_buffering off;
        proxy_request_buffering off;
    }
}
EOF

cat >/etc/unbound/unbound.conf <<EOF
server:
    verbosity: 1
    interface: 0.0.0.0
    port: 53
    do-ip4: yes
    do-ip6: no
    do-udp: yes
    do-tcp: yes
    access-control: 127.0.0.0/8 allow
    access-control: ${MOBILE_CIDR} allow
    access-control: 0.0.0.0/0 refuse
    local-zone: "speed." static
    local-data: "speed. 60 IN A ${GATEWAY_IP}"
    local-zone: "${LOCAL_DNS_ZONE}." transparent
    local-data: "speed.${LOCAL_DNS_ZONE}. 60 IN A ${GATEWAY_IP}"

forward-zone:
    name: "."
EOF

for upstream in $(printf '%s\n' "$UNBOUND_FORWARD_ADDRS" | tr ',' ' '); do
  [ -n "$upstream" ] || continue
  printf '    forward-addr: %s\n' "$upstream" >>/etc/unbound/unbound.conf
done

echo "FOU NAT ready: fou_udp=127.0.0.1:${FOU_LISTEN_PORT} relay_tcp=0.0.0.0:${FOU_TCP_PORT} relay_udp_bind=127.0.0.1:${FOU_REMOTE_PORT} tunnel=fou0 ${TUNNEL_ADDR} nat=${MOBILE_CIDR} dns=${GATEWAY_IP}:53 http=${GATEWAY_IP}:80 speedtest=${SPEEDTEST_UPSTREAM}"

unbound -d -c /etc/unbound/unbound.conf &
unbound_pid=$!

nginx -g 'daemon off;' &
nginx_pid=$!

/usr/local/bin/fou-tcp-relay \
  --tcp-port "$FOU_TCP_PORT" \
  --udp-bind-port "$FOU_REMOTE_PORT" \
  --udp-target-host 127.0.0.1 \
  --udp-target-port "$FOU_LISTEN_PORT" &
relay_pid=$!

trap 'kill "$relay_pid" "$nginx_pid" "$unbound_pid" 2>/dev/null || true; wait "$relay_pid" "$nginx_pid" "$unbound_pid" 2>/dev/null || true' INT TERM EXIT

while :; do
  for pid in "$relay_pid" "$nginx_pid" "$unbound_pid"; do
    if ! kill -0 "$pid" 2>/dev/null; then
      exit 1
    fi
  done
  sleep 2
done
