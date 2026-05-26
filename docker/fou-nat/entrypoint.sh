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

# Carrier MMSC hostnames redirected to our local Mbuni MMSC.
# Architecture: https://1xbts.org/docs/guides/mms
MMSC_HIJACK_HOSTS="${MMSC_HIJACK_HOSTS:-mms.vtext.com,mmsc.vtext.com,mms.myvzw.com,mmsc.sprintpcs.com,mms.sprintpcs.com,mmsc.mobile.att.net,mmsc.cingular.com,mms.msg.eng.t-mobile.com,mms.t-mobile.com,mmsc.uscc.net,mmsc.aiowireless.net}"

EDGE_RESOLVER_BIND="${EDGE_RESOLVER_BIND:-127.0.0.1:8088}"
MGMT_GRPC_ADDR="${MGMT_GRPC_ADDR:-host.docker.internal:17016}"
# Use docker's embedded DNS directly (not our captive Unbound, which
# rewrites mmsc.<zone> to the tunnel gateway and would loop back here).
MMSC_UPSTREAM="${MMSC_UPSTREAM:-http://mbuni}"

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

# Forward traffic from the MS subnet to any peer container on the compose
# bridge (e.g. the optional mbuni MMSC). Matching by source CIDR rather than
# pinning egress interface keeps this OS- and bridge-name-agnostic.
iptables -C FORWARD -i fou0 -s "$MOBILE_CIDR" -j ACCEPT 2>/dev/null \
  || iptables -A FORWARD -i fou0 -s "$MOBILE_CIDR" -j ACCEPT
iptables -C FORWARD -o fou0 -d "$MOBILE_CIDR" -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT 2>/dev/null \
  || iptables -A FORWARD -o fou0 -d "$MOBILE_CIDR" -m conntrack --ctstate RELATED,ESTABLISHED -j ACCEPT
# NAT replies from any other compose container back into the MS subnet so
# return traffic reaches the mobile via the same source IP it dialed.
iptables -t nat -C POSTROUTING -s "$MOBILE_CIDR" -d "$MOBILE_CIDR" -j RETURN 2>/dev/null \
  || iptables -t nat -I POSTROUTING -s "$MOBILE_CIDR" -d "$MOBILE_CIDR" -j RETURN

mkdir -p /run/nginx /var/log/nginx /etc/nginx/conf.d /etc/unbound
rm -f /etc/nginx/sites-enabled/default /etc/nginx/conf.d/default.conf 2>/dev/null || true

# MMSC reverse-proxy: nginx fetches the authoritative MSISDN for
# $remote_addr from the edge-resolver via auth_request, then sets
# X-1xBTS-MSISDN on the outbound request before handing off to
# Mbuni. Mbuni reads `mms-client-msisdn-header = X-1xBTS-MSISDN`
# and writes the value into the MO MMS envelope's F field.
#
# The internal `/_msisdn` location is marked `internal;` so a phone
# cannot call it directly even if it learns the URI.
#
# Adding more services that want the same header injection: copy
# this server block, swap the server_name and proxy_pass, keep the
# auth_request + proxy_set_header pair as-is.
if [ -n "$MMSC_HIJACK_HOSTS" ]; then
    MMSC_PROXY_SERVER_NAMES="mmsc.${LOCAL_DNS_ZONE}"
    for host in $(printf '%s\n' "$MMSC_HIJACK_HOSTS" | tr ',' ' '); do
        [ -n "$host" ] || continue
        MMSC_PROXY_SERVER_NAMES="${MMSC_PROXY_SERVER_NAMES} ${host}"
    done
else
    MMSC_PROXY_SERVER_NAMES="mmsc.${LOCAL_DNS_ZONE}"
fi

cat >/etc/nginx/conf.d/mmsc-proxy.conf <<EOF
server {
    listen 80;
    server_name ${MMSC_PROXY_SERVER_NAMES};

    # Use docker's embedded DNS directly for upstream resolution so the
    # Mbuni service name resolves to its container IP at request time
    # (not at nginx startup, which happens before mbuni in compose). We
    # bypass our captive Unbound because it rewrites mmsc.<zone> to the
    # gateway IP for the phone-facing hijack, which would loop back here.
    resolver 127.0.0.11 valid=30s ipv6=off;

    # Authoritative MSISDN attribution. nginx calls the resolver,
    # which queries PDSN for the session at \$remote_addr and returns
    # an X-1xBTS-MSISDN response header (empty on miss).
    location = /_msisdn {
        internal;
        proxy_pass_request_body off;
        proxy_set_header Content-Length "";
        proxy_set_header Host \$host;
        proxy_pass http://${EDGE_RESOLVER_BIND}/_msisdn?ip=\$remote_addr;
    }

    location / {
        auth_request /_msisdn;
        auth_request_set \$msisdn \$upstream_http_x_1xbts_msisdn;
        proxy_set_header X-1xBTS-MSISDN \$msisdn;
        proxy_set_header Host \$host;
        proxy_set_header X-Real-IP \$remote_addr;
        proxy_set_header X-Forwarded-For \$remote_addr;
        # Variable in proxy_pass forces per-request DNS lookup via the
        # resolver above. Without this, nginx caches the IP at startup
        # and a mbuni-container restart strands the cached value.
        set \$mmsc_upstream ${MMSC_UPSTREAM};
        proxy_pass \$mmsc_upstream\$request_uri;
    }
}
EOF

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
EOF

# Hijack the carrier MMSC hostnames and our own mmsc.<zone> to the
# gateway address. That puts all MMSC traffic through nginx (in this
# container), which calls the edge-resolver for an authoritative
# MSISDN header before forwarding to the actual Mbuni upstream.
#
# Resolving to ${GATEWAY_IP} (10.55.0.1) means the phone connects to
# fou-nat itself — same address it already uses for DNS and the
# tunnel — rather than to the mbuni container directly. Without this
# step traffic would skip nginx and arrive at Mbuni with no header.
echo "MMSC hijack -> ${GATEWAY_IP}, rewriting:"
# Always hijack mmsc.<zone> so M-Retrieve.conf URLs Mbuni embeds for
# MT delivery also land on nginx.
printf '    local-data: "mmsc.%s. 60 IN A %s"\n' "$LOCAL_DNS_ZONE" "$GATEWAY_IP" \
    >>/etc/unbound/unbound.conf
echo "  mmsc.${LOCAL_DNS_ZONE}"
if [ -n "$MMSC_HIJACK_HOSTS" ]; then
  for host in $(printf '%s\n' "$MMSC_HIJACK_HOSTS" | tr ',' ' '); do
    [ -n "$host" ] || continue
    printf '    local-zone: "%s." redirect\n' "$host" >>/etc/unbound/unbound.conf
    printf '    local-data: "%s. 60 IN A %s"\n' "$host" "$GATEWAY_IP" >>/etc/unbound/unbound.conf
    echo "  $host"
  done
fi

cat >>/etc/unbound/unbound.conf <<EOF

# Forward all other lookups inside the captive zone to docker's embedded DNS.
# This lets the auto-created compose network resolve service names (e.g.
# the optional 'mbuni' service exposed as mmsc.local.1xbts.org via a network
# alias) without recreating the network or pinning container IPs.
forward-zone:
    name: "${LOCAL_DNS_ZONE}."
    forward-addr: 127.0.0.11

forward-zone:
    name: "."
EOF

for upstream in $(printf '%s\n' "$UNBOUND_FORWARD_ADDRS" | tr ',' ' '); do
  [ -n "$upstream" ] || continue
  printf '    forward-addr: %s\n' "$upstream" >>/etc/unbound/unbound.conf
done

echo "FOU NAT ready: fou_udp=127.0.0.1:${FOU_LISTEN_PORT} relay_tcp=0.0.0.0:${FOU_TCP_PORT} relay_udp_bind=127.0.0.1:${FOU_REMOTE_PORT} tunnel=fou0 ${TUNNEL_ADDR} nat=${MOBILE_CIDR} dns=${GATEWAY_IP}:53 http=${GATEWAY_IP}:80 speedtest=${SPEEDTEST_UPSTREAM} edge-resolver=${EDGE_RESOLVER_BIND} mgmt=${MGMT_GRPC_ADDR}"

unbound -d -c /etc/unbound/unbound.conf &
unbound_pid=$!

# Edge MSISDN resolver. Loopback-only so phones cannot reach it
# directly; only nginx (in the same container) calls /_msisdn.
BIND_ADDR="${EDGE_RESOLVER_BIND}" \
    MGMT_GRPC_ADDR="${MGMT_GRPC_ADDR}" \
    /usr/local/bin/edge-resolver &
resolver_pid=$!

nginx -g 'daemon off;' &
nginx_pid=$!

/usr/local/bin/fou-tcp-relay \
  --tcp-port "$FOU_TCP_PORT" \
  --udp-bind-port "$FOU_REMOTE_PORT" \
  --udp-target-host 127.0.0.1 \
  --udp-target-port "$FOU_LISTEN_PORT" &
relay_pid=$!

trap 'kill "$relay_pid" "$nginx_pid" "$resolver_pid" "$unbound_pid" 2>/dev/null || true; wait "$relay_pid" "$nginx_pid" "$resolver_pid" "$unbound_pid" 2>/dev/null || true' INT TERM EXIT

while :; do
  for pid in "$relay_pid" "$nginx_pid" "$resolver_pid" "$unbound_pid"; do
    if ! kill -0 "$pid" 2>/dev/null; then
      exit 1
    fi
  done
  sleep 2
done
