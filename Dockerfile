# syntax=docker/dockerfile:1

# The image installs a prebuilt mcp-v8 server binary from the project's GitHub
# Releases instead of compiling the source in this checkout. A release build
# takes a few seconds to download; a source build of V8 takes tens of minutes.
#
#   docker build -t mcp-v8 .                                    # latest release
#   docker build --build-arg MCP_V8_VERSION=v0.20.1 -t mcp-v8 . # pinned release
#   docker build --build-arg MCP_V8_BUILD=source -t mcp-v8 .    # compile checkout
#
# MCP_V8_VERSION accepts a release tag with or without the leading "v", or
# "latest" (the default). Docker caches the download layer by its inputs, so a
# rebuild with the default "latest" reuses a previously downloaded binary even
# after a newer release ships; pass an explicit tag or `--no-cache` to refresh.
#
# MCP_V8_BUILD selects where the binary comes from: "release" (the default)
# downloads it, "source" compiles the working tree. The integration-test
# workflows use "source" so they exercise the code under test; only the stage
# that is selected gets built.
ARG MCP_V8_VERSION=latest
ARG MCP_V8_REPO=r33drichards/mcp-js
ARG MCP_V8_BUILD=release

# ── Release binary: download from GitHub Releases ────────────────────────────
FROM debian:trixie-slim AS binary-release

ARG MCP_V8_VERSION
ARG MCP_V8_REPO
# Populated by BuildKit from --platform (amd64 / arm64); defaults to the host.
ARG TARGETARCH
ARG BUILDPLATFORM
ARG TARGETPLATFORM

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Release assets are named per release.yml: mcp-v8-linux.gz (x86_64) and
# mcp-v8-linux-arm64.gz (aarch64). Both are glibc-floored (2.27) dynamic
# binaries linked against rustls, so they need no OpenSSL at runtime.
RUN set -eu; \
    case "${TARGETARCH:-$(dpkg --print-architecture)}" in \
      amd64) asset="mcp-v8-linux.gz" ;; \
      arm64) asset="mcp-v8-linux-arm64.gz" ;; \
      *) echo "mcp-v8 has no release binary for architecture '${TARGETARCH}'" >&2; exit 1 ;; \
    esac; \
    case "$MCP_V8_VERSION" in \
      latest) url="https://github.com/${MCP_V8_REPO}/releases/latest/download/${asset}" ;; \
      v*)     url="https://github.com/${MCP_V8_REPO}/releases/download/${MCP_V8_VERSION}/${asset}" ;; \
      *)      url="https://github.com/${MCP_V8_REPO}/releases/download/v${MCP_V8_VERSION}/${asset}" ;; \
    esac; \
    echo "Downloading ${url}"; \
    curl --fail --silent --show-error --location --retry 5 --retry-delay 2 \
      -o /tmp/mcp-v8.gz "$url"; \
    gunzip -c /tmp/mcp-v8.gz > /mcp-v8; \
    chmod 0755 /mcp-v8; \
    rm /tmp/mcp-v8.gz; \
    # Smoke-test the binary when it can run natively. Under emulation
    # (a cross-platform buildx build) the check is skipped rather than run
    # through QEMU.
    if [ -z "${TARGETPLATFORM:-}" ] || [ "${BUILDPLATFORM:-}" = "${TARGETPLATFORM:-}" ]; then \
      /mcp-v8 --version; \
    fi

# ── Source binary: compile this checkout ─────────────────────────────────────
FROM rust:latest AS binary-source

# Install required dependencies for V8 build
RUN apt-get update && apt-get install -y \
    python3 \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app

# Copy the entire project
COPY . .

# Install nightly toolchain as required by rust-toolchain file
RUN rustup default nightly

# Build the release binary
RUN cargo build --release -p server \
    && cp target/release/server /mcp-v8 \
    && /mcp-v8 --version

# ── Binary selection ─────────────────────────────────────────────────────────
# Resolves to binary-release or binary-source; BuildKit builds only that one.
FROM binary-${MCP_V8_BUILD} AS binary

# ── Runtime stage ────────────────────────────────────────────────────────────
FROM debian:trixie-slim

LABEL io.modelcontextprotocol.server.name="io.github.r33drichards/mcp-js"

# ca-certificates lets the server's outbound HTTPS (fetch(), JWKS, S3) verify
# peers; TLS itself is rustls, statically linked into the binary.
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 mcpuser

# Install the selected binary
COPY --from=binary --chown=mcpuser:mcpuser /mcp-v8 /usr/local/bin/mcp-v8

# Create default data directory for stateful mode (heaps, sessions, etc.)
RUN mkdir -p /data && chown mcpuser:mcpuser /data

# Switch to non-root user
USER mcpuser

# Expose the MCP HTTP port (default 8080)
EXPOSE 8080

# Default the port the server listens on. mcp-v8 folds $PORT into --http-port,
# so the container serves the Streamable HTTP MCP endpoint at POST /mcp — which
# is what MCP clients and hosted-deployment health checks probe. The legacy SSE
# transport (--sse-port) only serves /sse + /message and 404s on /mcp.
#
# Hosted platforms (Railway, Render, Heroku, Fly, Cloud Run, ...) inject their
# own $PORT, which overrides this default and needs no argument changes. An
# explicit --http-port/--sse-port argument still wins over $PORT, and
# `-e PORT=` clears it to select the stdio transport.
ENV PORT=8080

# Accept any Host header. The Streamable HTTP transport otherwise allows only
# loopback hosts, as DNS-rebinding protection for servers a browser on the same
# machine can reach, and would 403 every request routed by a platform domain or
# reverse proxy. Publishing a container that listens on a port is already the
# decision to serve a network, so the opt-out belongs here rather than in the
# binary's default.
#
# Narrow it back down with -e MCP_V8_ALLOWED_HOSTS=mcp.example.com (or
# --allowed-hosts) when the hostnames clients use are known — worth doing if the
# port is published to a developer machine rather than a deployment.
ENV MCP_V8_ALLOWED_HOSTS=*

# Use ENTRYPOINT for the binary so arguments can be appended directly.
# This allows Docker MCP Registry and other orchestrators to override
# just the arguments without repeating the binary name, e.g.:
#   docker run <image> --http-port 8080 --fs-store dir --fs-dir /data/fs
ENTRYPOINT ["mcp-v8"]

# Clear the base image's inherited CMD ("bash"), which would otherwise be
# appended to the ENTRYPOINT and rejected as an unexpected argument. The
# transport comes from $PORT above, so no default arguments are needed.
CMD []
