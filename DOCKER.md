# Docker Guide for mcp-v8

This guide explains how to build and run the mcp-v8 HTTP server using Docker.

## Building the Docker Image

Build the image from the project root:

```bash
docker build -t mcp-v8:latest .
```

The build process:
1. Uses Rust nightly toolchain (as required by the project)
2. Installs dependencies needed for V8 compilation
3. Builds the release binary
4. Creates a minimal runtime image with only necessary dependencies
5. Runs as non-root user for security

## Running the Container

### Basic HTTP Server (Local Storage)

```bash
docker run -p 8080:8080 mcp-v8:latest
```

This starts the HTTP server on port 8080 with local filesystem storage.

### Custom Port

```bash
docker run -p 3000:3000 mcp-v8:latest mcp-v8 --http-port 3000 --directory-path /tmp/mcp-v8-heaps
```

### With Persistent Storage (Volume)

```bash
docker run -p 8080:8080 -v mcp-v8-data:/tmp/mcp-v8-heaps mcp-v8:latest
```

This creates a named volume to persist heap snapshots between container restarts.

### With S3 Storage

```bash
docker run -p 8080:8080 \
  -e AWS_ACCESS_KEY_ID=your_access_key \
  -e AWS_SECRET_ACCESS_KEY=your_secret_key \
  -e AWS_REGION=us-east-1 \
  mcp-v8:latest \
  mcp-v8 --http-port 8080 --s3-bucket your-bucket-name
```

## Docker Compose

Create a `docker-compose.yml` file:

```yaml
version: '3.8'

services:
  mcp-v8:
    build: .
    ports:
      - "8080:8080"
    volumes:
      - mcp-v8-data:/tmp/mcp-v8-heaps
    environment:
      - RUST_LOG=info
    restart: unless-stopped

volumes:
  mcp-v8-data:
```

Run with:

```bash
docker-compose up -d
```

## Testing the HTTP Server

Once running, test the connection:

```bash
# Test basic connectivity
curl http://localhost:8080

# Use MCP Inspector
npx @modelcontextprotocol/inspector http://localhost:8080
```

## Environment Variables

- `AWS_ACCESS_KEY_ID` - AWS access key for S3 storage
- `AWS_SECRET_ACCESS_KEY` - AWS secret key for S3 storage
- `AWS_REGION` - AWS region for S3 bucket
- `RUST_LOG` - Logging level (debug, info, warn, error)

## Security Notes

- The container runs as non-root user (mcpuser, UID 1000)
- Only essential runtime dependencies are included
- The HTTP server binds to 0.0.0.0 to accept connections from outside the container
- Consider using secrets management for AWS credentials in production
- In production, use a reverse proxy (nginx, traefik) for TLS termination

## Troubleshooting

### Container Exits Immediately

Check logs:
```bash
docker logs <container-id>
```

### Port Already in Use

Change the host port:
```bash
docker run -p 9090:8080 mcp-v8:latest
```

### S3 Access Issues

Verify AWS credentials and permissions:
```bash
docker run -it mcp-v8:latest bash -c "env | grep AWS"
```

## Building for Different Architectures

Build for ARM64 (Apple Silicon, ARM servers):
```bash
docker buildx build --platform linux/arm64 -t mcp-v8:arm64 .
```

Build for AMD64 (most servers):
```bash
docker buildx build --platform linux/amd64 -t mcp-v8:amd64 .
```

Multi-platform build:
```bash
docker buildx build --platform linux/amd64,linux/arm64 -t mcp-v8:latest .
```
