FROM debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        iproute2 \
        iptables \
        iputils-ping \
        nginx \
        python3 \
        procps \
        tcpdump \
        unbound \
    && rm -rf /var/lib/apt/lists/*

COPY entrypoint.sh /usr/local/bin/fou-nat-entrypoint
COPY fou_tcp_relay.py /usr/local/bin/fou-tcp-relay
RUN chmod +x /usr/local/bin/fou-nat-entrypoint
RUN chmod +x /usr/local/bin/fou-tcp-relay

ENTRYPOINT ["/usr/local/bin/fou-nat-entrypoint"]
