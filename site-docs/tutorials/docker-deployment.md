# Tutorial: Deploying with Docker

In this tutorial you will deploy mcp-v8 using Docker. You will build the image, run a single node, and then set up a 3-node cluster with docker-compose.

## Prerequisites

- Docker installed
- docker-compose installed (for the cluster section)
- Basic Docker familiarity

## Step 1: Understand the Docker setup

The mcp-v8 Dockerfile:
- Builds the binary from Rust source
- Creates a non-root user (`mcpuser`) for security
- Exposes SSE on port 8080 by default
- Runs the `mcp-v8` binary as the entrypoint

## Step 2: Build the Docker image

Clone the repository and build:

```bash
git clone https://github.com/r33drichards/mcp-js.git
cd mcp-js
docker build -t mcp-v8 .
```

The build compiles the Rust project inside the container, so it may take several minutes the first time.

## Step 3: Run a single node

Start a single mcp-v8 container with SSE transport:

```bash
docker run -d \
  --name mcp-v8 \
  -p 8080:8080 \
  mcp-v8 --sse-port 8080
```

## Step 4: Test the container

Verify the server is running:

```bash
curl -s -I http://localhost:8080/sse
```

You should see a `200` response.

Execute code (using SSE or add HTTP port):

```bash
docker rm -f mcp-v8

docker run -d \
  --name mcp-v8 \
  -p 8080:8080 \
  -p 3000:3000 \
  mcp-v8 --sse-port 8080 --http-port 3000
```

```bash
curl -s -X POST http://localhost:3000/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"Hello from Docker!\""}' | jq .
```

Expected output:

```json
{
  "result": "Hello from Docker!"
}
```

## Step 5: Mount a volume for persistent storage

To persist heap snapshots across container restarts:

```bash
docker run -d \
  --name mcp-v8 \
  -p 8080:8080 \
  -p 3000:3000 \
  -v mcp-v8-heaps:/tmp/mcp-v8-heaps \
  mcp-v8 --sse-port 8080 --http-port 3000
```

The `-v mcp-v8-heaps:/tmp/mcp-v8-heaps` mounts a Docker volume at the default storage path.

## Step 6: Run with custom configuration

Pass configuration via command arguments:

```bash
docker run -d \
  --name mcp-v8 \
  -p 8080:8080 \
  -p 3000:3000 \
  -v mcp-v8-heaps:/data/heaps \
  mcp-v8 \
    --sse-port 8080 \
    --http-port 3000 \
    --directory-path /data/heaps \
    --timeout 60
```

## Step 7: Set up a 3-node cluster with docker-compose

Create a `docker-compose.yml` file:

```yaml
version: "3.8"

services:
  node1:
    image: mcp-v8
    container_name: mcp-v8-node1
    ports:
      - "3001:3000"
      - "8081:8080"
    command: >
      --http-port 3000
      --sse-port 8080
      --node-id 1
      --cluster-port 5000
      --peers "2=node2:5000,3=node3:5000"
      --directory-path /data/heaps
    volumes:
      - node1-heaps:/data/heaps
    networks:
      - mcp-cluster

  node2:
    image: mcp-v8
    container_name: mcp-v8-node2
    ports:
      - "3002:3000"
      - "8082:8080"
    command: >
      --http-port 3000
      --sse-port 8080
      --node-id 2
      --cluster-port 5000
      --peers "1=node1:5000,3=node3:5000"
      --directory-path /data/heaps
    volumes:
      - node2-heaps:/data/heaps
    networks:
      - mcp-cluster

  node3:
    image: mcp-v8
    container_name: mcp-v8-node3
    ports:
      - "3003:3000"
      - "8083:8080"
    command: >
      --http-port 3000
      --sse-port 8080
      --node-id 3
      --cluster-port 5000
      --peers "1=node1:5000,2=node2:5000"
      --directory-path /data/heaps
    volumes:
      - node3-heaps:/data/heaps
    networks:
      - mcp-cluster

volumes:
  node1-heaps:
  node2-heaps:
  node3-heaps:

networks:
  mcp-cluster:
    driver: bridge
```

## Step 8: Start the cluster

```bash
docker-compose up -d
```

This starts all three nodes. They discover each other via Docker DNS (the service names `node1`, `node2`, `node3` resolve within the `mcp-cluster` network).

## Step 9: Verify the cluster

Check that all containers are running:

```bash
docker-compose ps
```

Test each node:

```bash
# Node 1
curl -s -X POST http://localhost:3001/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"Node 1 responding\""}' | jq .

# Node 2
curl -s -X POST http://localhost:3002/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"Node 2 responding\""}' | jq .

# Node 3
curl -s -X POST http://localhost:3003/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"Node 3 responding\""}' | jq .
```

## Step 10: Test cluster failover

Stop one node:

```bash
docker-compose stop node1
```

The remaining nodes continue serving requests:

```bash
curl -s -X POST http://localhost:3002/execute \
  -H "Content-Type: application/json" \
  -d '{"code": "\"Still running with 2 nodes\""}' | jq .
```

Bring the node back:

```bash
docker-compose start node1
```

## Step 11: Add policies via mounted files

Mount a policies file into the container:

```bash
docker run -d \
  --name mcp-v8 \
  -p 3000:3000 \
  -v $(pwd)/policies.json:/etc/mcp-v8/policies.json:ro \
  mcp-v8 \
    --http-port 3000 \
    --policies-json /etc/mcp-v8/policies.json
```

## Step 12: Clean up

Stop and remove everything:

```bash
docker-compose down -v
docker rm -f mcp-v8
```

## What you learned

- How to build the mcp-v8 Docker image from the repository
- How to run a single node with Docker, exposing HTTP and SSE ports
- How to persist heap snapshots with Docker volumes
- How to set up a 3-node Raft cluster with docker-compose
- How Docker networking enables service discovery between cluster nodes
- How to test failover in a containerized cluster
- How to mount policy files into containers

This concludes the tutorials. Return to the [Tutorials Overview](overview.md) for links to all tutorials.
