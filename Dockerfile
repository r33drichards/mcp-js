# Build stage
FROM rust:latest AS builder

# Install required dependencies for V8 build
RUN apt-get update && apt-get install -y \
    python3 \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Set working directory
WORKDIR /app

# Copy the entire project
COPY . .

# Install nightly toolchain as required by rust-toolchain file
RUN rustup default nightly

# Build the release binary
RUN cargo build --release -p server

# Runtime stage
FROM debian:trixie-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 mcpuser

# Copy the binary from builder
COPY --from=builder /app/target/release/server /usr/local/bin/mcp-v8

# Set ownership
RUN chown mcpuser:mcpuser /usr/local/bin/mcp-v8

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

