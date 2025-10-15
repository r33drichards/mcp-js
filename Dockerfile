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
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user for security
RUN useradd -m -u 1000 mcpuser

# Create directory for heap storage with proper permissions
RUN mkdir -p /tmp/mcp-v8-heaps && chown mcpuser:mcpuser /tmp/mcp-v8-heaps

# Copy the binary from builder
COPY --from=builder /app/server/target/release/server /usr/local/bin/mcp-v8

# Set ownership
RUN chown mcpuser:mcpuser /usr/local/bin/mcp-v8

# Switch to non-root user
USER mcpuser

# Default to using local filesystem storage
ENV HEAP_STORAGE_PATH=/tmp/mcp-v8-heaps

# Expose HTTP port (default 8080)
EXPOSE 8080

# Default command: run HTTP server on port 8080 with local storage
# Users can override with their own command to use S3 or different settings
CMD ["mcp-v8", "--http-port", "8080", "--directory-path", "/tmp/mcp-v8-heaps"]
