# syntax=docker/dockerfile:1.7
#
# Builds an optional Mbuni MMSC image bundled with the workspace's
# mbuni-msc-bridge process. Triggered by `docker compose --profile mms ...`.
#
# Sources are fetched from upstream Mbuni / Kannel and verified against
# pinned SHA256 hashes. Mbuni 1.6.0 (December 2013) is the last released
# version of the project; no newer upstream exists.

ARG MBUNI_URL=https://mbuni.org/downloads/1.6.0/mbuni-1.6.0.tar.gz
ARG MBUNI_SHA256=a0d7b5028eb88871f4f468d1da297d91ba316d02361a3d85007882bcdbb28bf4
ARG KANNEL_URL=https://www.kannel.org/download/1.4.5/gateway-1.4.5.tar.gz
ARG KANNEL_SHA256=f279606ea8317fd2c8fa6b0f591c084fdfa954612f1e7c9cb667a5f6b77387f3

# ---------------------------------------------------------------------------
# Stage 1: build Kannel + Mbuni from source.
# ---------------------------------------------------------------------------
FROM debian:bookworm AS mbuni-build
ARG MBUNI_URL
ARG MBUNI_SHA256
ARG KANNEL_URL
ARG KANNEL_SHA256

RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential \
        ca-certificates \
        curl \
        bison \
        flex \
        autoconf \
        automake \
        libtool \
        pkg-config \
        libxml2-dev \
        libssl-dev \
        zlib1g-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build

# -k: kannel.org serves a broken TLS chain (wrong intermediate). Authenticity
# is enforced by the SHA256 pin verified immediately after download.
RUN curl -fsSLk -o kannel.tar.gz "${KANNEL_URL}" \
    && echo "${KANNEL_SHA256}  kannel.tar.gz" | sha256sum -c - \
    && tar xzf kannel.tar.gz \
    && rm kannel.tar.gz

RUN curl -fsSL -o mbuni.tar.gz "${MBUNI_URL}" \
    && echo "${MBUNI_SHA256}  mbuni.tar.gz" | sha256sum -c - \
    && tar xzf mbuni.tar.gz \
    && rm mbuni.tar.gz

# Build only the Kannel libraries Mbuni links against: libgw.a and
# libgwlib.a. Kannel 1.4.5's WMLScript grammar predates bison 3.x and fails
# to compile under modern bison; we don't need it (Mbuni runs its own
# WSP/HTTP) so we name the library targets explicitly and skip the rest.
# Set --prefix to where we'll stage the install. Kannel's `make` produces a
# `gw-config` shell script whose -I/-L paths are hard-coded from --prefix;
# Mbuni's configure invokes that script (`gw-config --cflags`/`--libs`) to
# learn where the headers and archives live.
RUN cd gateway-1.4.5 \
    && ./configure --prefix=/build/kannel-install \
                   --disable-docs --disable-wap-compile \
                   CFLAGS="-fPIC -g -O2" \
    && make libgw.a libgwlib.a libwap.a gw-config

# Mbuni's configure expects an *installed* Kannel layout
# ($DIR/include/kannel/{gw-config.h,gwlib,gw,...} and $DIR/lib/kannel/*.a).
# Kannel's `make install` target depends on `all`, which would try to build
# wmlscript — so we replicate the relevant pieces by hand, mirroring the
# `install:` recipe in Kannel's Makefile.in.
RUN K=/build/gateway-1.4.5 \
 && I=/build/kannel-install \
 && mkdir -p "$I/bin" \
            "$I/include/kannel" \
            "$I/include/kannel/gwlib" \
            "$I/include/kannel/gw" \
            "$I/include/kannel/gw/smsc" \
            "$I/include/kannel/wap" \
            "$I/lib/kannel" \
 && cp "$K"/gw-config.h "$I/include/kannel/" \
 && cp "$K"/gwlib/*.h "$K"/gwlib/*.def "$I/include/kannel/gwlib/" \
 && cp "$K"/gw/*.h    "$K"/gw/*.def    "$I/include/kannel/gw/" \
 && cp "$K"/gw/smsc/*.h "$I/include/kannel/gw/smsc/" \
 && cp "$K"/wap/*.h "$K"/wap/*.def "$I/include/kannel/wap/" \
 && cp "$K"/libgw.a "$K"/libgwlib.a "$K"/libwap.a "$I/lib/kannel/" \
 && cp "$K"/gw-config "$I/bin/" \
 && chmod +x "$I/bin/gw-config"

# Build Mbuni against the staged Kannel install. --build is set explicitly
# because Mbuni's vendored config.guess (timestamp 2006-07-02) predates
# aarch64/arm64 and bails out on Apple Silicon hosts.
#
# Patches to make Mbuni 1.6.0 compile and link against modern toolchains:
#   * mmlib/mms_cfg.c uses `bool` as a parameter name; C11 makes that
#     reserved (pulled in via libxml2 → ICU → stdbool). Rename to `out`
#     — not `val` because there's already a local of that name in the
#     function.
#   * mmlib/mms_util.c calls the Kannel SSL helpers by their old
#     un-prefixed names; Kannel 1.4.5 renamed them to `conn_*`.
RUN cd mbuni-1.6.0 \
    && sed -i 's/int \*bool)/int *out)/; s/\*bool = /\*out = /g' mmlib/mms_cfg.c \
    && sed -i 's/\b\(use_global_\(client_certkey_file\|server_certkey_file\|trusted_ca_file\)\)/conn_\1/g' \
        mmlib/mms_util.c \
    && ./configure \
        --with-kannel-dir=/build/kannel-install \
        --prefix=/opt/mbuni \
        --build="$(uname -m)-unknown-linux-gnu" \
    && make \
    && make install

# ---------------------------------------------------------------------------
# Stage 2: build the mbuni-msc-bridge Rust binary out of the workspace source.
# ---------------------------------------------------------------------------
FROM rust:1-bookworm AS bridge-build

RUN apt-get update && apt-get install -y --no-install-recommends \
        protobuf-compiler \
        libprotobuf-dev \
        cmake \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
COPY . .
RUN cargo build --release -p mbuni-msc-bridge

# ---------------------------------------------------------------------------
# Stage 3: slim runtime image. Both processes are supervised by entrypoint.sh.
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
        ca-certificates \
        libxml2 \
        libssl3 \
        zlib1g \
        gettext-base \
        curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=mbuni-build /opt/mbuni /opt/mbuni
COPY --from=bridge-build /src/target/release/mbuni-msc-bridge /usr/local/bin/mbuni-msc-bridge
COPY docker/mbuni/mbuni.conf.template /opt/mbuni/etc/mbuni.conf.template
COPY docker/mbuni/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

ENV PATH=/opt/mbuni/bin:/opt/mbuni/sbin:${PATH}
ENV MMSC_HOSTNAME=mmsc.local.1xbts.org \
    MMSC_PORT=80 \
    MSC_GRPC_ADDR=host.docker.internal:17017 \
    BRIDGE_BIND_ADDR=127.0.0.1:8081

EXPOSE 80

VOLUME ["/var/spool/mbuni"]

ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
