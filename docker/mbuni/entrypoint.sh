#!/bin/sh
# Mbuni + mbuni-msc-bridge container entrypoint.
#
# Templates /opt/mbuni/etc/mbuni.conf from env vars, starts mbuni-msc-bridge
# in the background on its loopback port, waits for it to become healthy,
# then exec's mmsc in the foreground so signals propagate to Mbuni.

set -eu

CONF=/opt/mbuni/etc/mbuni.conf
TEMPLATE=/opt/mbuni/etc/mbuni.conf.template

: "${MMSC_HOSTNAME:=mmsc.local.1xbts.org}"
: "${MMSC_PORT:=80}"
: "${BRIDGE_BIND_ADDR:=127.0.0.1:8081}"
: "${MSC_GRPC_ADDR:=host.docker.internal:17017}"

export MMSC_HOSTNAME MMSC_PORT BRIDGE_BIND_ADDR

mkdir -p /var/spool/mbuni
envsubst < "${TEMPLATE}" > "${CONF}"

# Mbuni fetches every handset's UAProf XML synchronously before answering
# M-Retrieve.conf, even with content-adaptation disabled. The well-known
# carrier UAProf hosts (e.g. uaprof.vtext.com) are out on the public
# internet and frequently take 60-90s to fail, which makes MMS retrieval
# look hung. Blackhole the common ones to 127.0.0.1 so connect() fails
# immediately and Mbuni falls back to building a profile from request
# headers. Override with UAPROF_BLACKHOLE_HOSTS=host1,host2 (empty to
# disable).
UAPROF_BLACKHOLE_HOSTS="${UAPROF_BLACKHOLE_HOSTS:-uaprof.vtext.com,uaprof.mobile.att.net,uaprof.sprintpcs.com,uaprof.tmobile.com,uaprof.uscc.net,device.sprintpcs.com}"
if [ -n "${UAPROF_BLACKHOLE_HOSTS}" ]; then
    for h in $(printf '%s\n' "${UAPROF_BLACKHOLE_HOSTS}" | tr ',' ' '); do
        [ -z "$h" ] && continue
        grep -qE "[[:space:]]${h}([[:space:]]|\$)" /etc/hosts || \
            echo "127.0.0.1 ${h}" >>/etc/hosts
    done
fi

echo "[entrypoint] starting mbuni-msc-bridge on ${BRIDGE_BIND_ADDR} -> MSC ${MSC_GRPC_ADDR}"
MSC_GRPC_ADDR="${MSC_GRPC_ADDR}" \
    BIND_ADDR="${BRIDGE_BIND_ADDR}" \
    /usr/local/bin/mbuni-msc-bridge &
BRIDGE_PID=$!

# Forward SIGTERM/SIGINT to both processes.
trap 'echo "[entrypoint] shutting down"; kill -TERM "${BRIDGE_PID}" 2>/dev/null || true; exit 0' TERM INT

# Wait for mbuni-msc-bridge to answer /healthz before launching mbuni.
i=0
while [ "$i" -lt 50 ]; do
    if curl -fsS "http://${BRIDGE_BIND_ADDR}/healthz" >/dev/null 2>&1; then
        break
    fi
    i=$((i + 1))
    sleep 0.1
done
if ! kill -0 "${BRIDGE_PID}" 2>/dev/null; then
    echo "[entrypoint] mbuni-msc-bridge exited prematurely" >&2
    exit 1
fi

echo "[entrypoint] starting Mbuni MMSC on ${MMSC_HOSTNAME}:${MMSC_PORT}"
exec /opt/mbuni/bin/mmsc "${CONF}"
