# Tutorial: Configuring Heap Storage

In this tutorial you will configure mcp-v8 with different storage backends for heap snapshots: local filesystem, Amazon S3, write-through cache, and stateless mode. You will understand the tradeoffs of each approach.

## Prerequisites

- mcp-v8 installed ([Installation](installation.md))
- mcp-v8-cli installed
- AWS credentials configured (for the S3 section)

## Step 1: Understand storage backends

mcp-v8 needs to store heap snapshots somewhere. The storage backend determines where and how these snapshots are persisted:

| Backend | Flag | Use case |
|---|---|---|
| Local filesystem (default) | `--directory-path` | Development, single node |
| Amazon S3 | `--s3-bucket` | Production, shared storage |
| Write-through cache | Both flags together | Performance + durability |
| Stateless | `--stateless` | No storage needed |

The default storage directory is `/tmp/mcp-v8-heaps`.

## Step 2: Use default local storage

Start the server with no storage flags:

```bash
mcp-v8 --http-port 3000
```

Heap snapshots are stored in `/tmp/mcp-v8-heaps`. Run some code:

```bash
mcp-v8-cli --http-port 3000 exec --code '
var message = "stored locally";
message;
'
```

Check the storage directory:

```bash
ls /tmp/mcp-v8-heaps/
```

You will see files corresponding to heap snapshots.

## Step 3: Use a custom local directory

Specify a custom path for local storage:

```bash
mcp-v8 --http-port 3000 --directory-path /tmp/mcp-v8-custom-storage
```

Run code to generate a snapshot:

```bash
mcp-v8-cli --http-port 3000 exec --code '
var storageTest = "custom directory";
storageTest;
'
```

Verify the snapshot is in your custom directory:

```bash
ls /tmp/mcp-v8-custom-storage/
```

This is useful for organizing snapshots by project or environment, and for persisting them across system reboots (use a non-tmp path).

## Step 4: Configure S3 storage

For production deployments, store heap snapshots in S3. This enables:
- Persistence across server restarts
- Shared access from multiple nodes
- Unlimited storage capacity

Start the server with S3:

```bash
mcp-v8 --http-port 3000 --s3-bucket my-mcp-v8-heaps
```

Make sure your AWS credentials are configured (via environment variables, AWS config file, or IAM role).

## Step 5: Test S3 storage

```bash
mcp-v8-cli --http-port 3000 exec --code '
var s3Test = "stored in S3";
s3Test;
'
```

The heap snapshot is uploaded to the `my-mcp-v8-heaps` S3 bucket. You can verify with:

```bash
aws s3 ls s3://my-mcp-v8-heaps/
```

## Step 6: Resume from S3 snapshots

Since snapshots are in S3, you can stop the server, restart it, and resume from any previous snapshot:

```bash
# Stop the server (Ctrl+C), then restart
mcp-v8 --http-port 3000 --s3-bucket my-mcp-v8-heaps

# Resume from the previous heap
mcp-v8-cli --http-port 3000 exec --code '
s3Test;
' --heap-id <HEAP_HASH>
```

The snapshot is fetched from S3 and restored.

## Step 7: Configure write-through cache

For the best of both worlds, use write-through cache. Specify both a local directory and an S3 bucket:

```bash
mcp-v8 --http-port 3000 \
  --directory-path /tmp/mcp-v8-cache \
  --s3-bucket my-mcp-v8-heaps
```

With write-through cache:
- **Writes** go to both local disk and S3
- **Reads** check local disk first, then fall back to S3
- Local disk acts as a fast cache
- S3 provides durable, shared storage

## Step 8: Test write-through behavior

```bash
mcp-v8-cli --http-port 3000 exec --code '
var cacheTest = "write-through cached";
cacheTest;
'
```

The snapshot is now stored in both locations:

```bash
# Check local cache
ls /tmp/mcp-v8-cache/

# Check S3
aws s3 ls s3://my-mcp-v8-heaps/
```

Subsequent reads for this snapshot will be served from local disk (fast), while the S3 copy ensures durability.

## Step 9: Use stateless mode

If you do not need to persist state at all, use stateless mode:

```bash
mcp-v8 --http-port 3000 --stateless
```

In stateless mode:
- No heap snapshots are created or stored
- Each execution starts with a fresh V8 isolate
- Console output is returned directly in the response
- Only the `run_js` tool is available

```bash
mcp-v8-cli --http-port 3000 exec --code '
"No state stored anywhere!";
'
```

## Step 10: Choose the right backend

| Scenario | Recommended backend |
|---|---|
| Local development | Default (`/tmp/mcp-v8-heaps`) |
| Production, single node | `--directory-path /var/lib/mcp-v8/heaps` |
| Production, multi-node | `--s3-bucket` |
| Production, performance-sensitive | Write-through (both flags) |
| Ephemeral/serverless | `--stateless` |
| CI/CD pipelines | `--stateless` or default |

## What you learned

- The default storage location is `/tmp/mcp-v8-heaps`
- `--directory-path` sets a custom local filesystem path
- `--s3-bucket` stores snapshots in Amazon S3 for durability and sharing
- Combining both flags enables write-through cache (local reads, S3 durability)
- `--stateless` disables all storage for ephemeral use cases
- The choice of backend depends on your persistence, performance, and sharing requirements

Next, learn about the REST API in [Using the REST API Directly](http-api.md).
