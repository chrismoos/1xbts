# syntax=docker/dockerfile:1.7
#
# Build context is the repo root so the multi-stage build can reach the
# Rust workspace for edge-resolver. See docker-compose.yml for the
# matching `dockerfile:` directive.

# ---------------------------------------------------------------------------
# Stage 1: build the edge-resolver and hdtp-gw Rust binaries from the workspace.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS resolver-build

RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler \
        libprotobuf-dev \
        cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --release -p edge-resolver -p hdtp-gw

# ---------------------------------------------------------------------------
# Stage 2: runtime image. Single container, edge-resolver supervised
# alongside nginx / unbound / fou-tcp-relay by entrypoint.sh.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

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

COPY --from=resolver-build /src/target/release/edge-resolver /usr/local/bin/edge-resolver
COPY --from=resolver-build /src/target/release/hdtp-gw /usr/local/bin/hdtp-gw
COPY docker/fou-nat/entrypoint.sh /usr/local/bin/fou-nat-entrypoint
COPY docker/fou-nat/fou_tcp_relay.py /usr/local/bin/fou-tcp-relay
RUN chmod +x /usr/local/bin/fou-nat-entrypoint
RUN chmod +x /usr/local/bin/fou-tcp-relay

ENTRYPOINT ["/usr/local/bin/fou-nat-entrypoint"]
