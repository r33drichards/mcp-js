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
RUN cd server && cargo build --release

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
COPY --from=builder /app/server/target/release/server /usr/local/bin/mcp-v8

# Set ownership
RUN chown mcpuser:mcpuser /usr/local/bin/mcp-v8

# Create default data directory for stateful mode (heaps, sessions, etc.)
RUN mkdir -p /data && chown mcpuser:mcpuser /data

# Switch to non-root user
USER mcpuser

# Expose SSE port (default 8080)
EXPOSE 8080

# Default: run SSE server on port 8080 in stateless mode.
# Override with: docker run <image> mcp-v8 --http-port 8080 --stateless, etc.
# Note: We use CMD (not ENTRYPOINT) so that platforms like Railway that override
# the start command can specify: mcp-v8 --sse-port 8080 --stateless
CMD ["mcp-v8", "--sse-port", "8080", "--stateless"]

