
FROM rust:latest AS builder


RUN apt-get update && apt-get install -y \
    python3 \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*


WORKDIR /app


COPY . .


RUN rustup default nightly


RUN cargo build --release -p server


FROM debian:trixie-slim


RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    && rm -rf /var/lib/apt/lists/*


RUN useradd -m -u 1000 mcpuser


COPY --from=builder /app/target/release/server /usr/local/bin/mcp-v8


RUN chown mcpuser:mcpuser /usr/local/bin/mcp-v8


RUN mkdir -p /data && chown mcpuser:mcpuser /data


USER mcpuser


EXPOSE 8080




ENTRYPOINT ["mcp-v8"]



CMD ["--sse-port", "8080"]
