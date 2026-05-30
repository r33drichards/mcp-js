# Use S3 Storage

Start the server with S3-backed heap storage:

```bash
./target/release/server --s3-bucket my-mcp-v8-heaps
```

To add a local write-through cache in front of S3:

```bash
./target/release/server \
  --s3-bucket my-mcp-v8-heaps \
  --cache-dir /var/cache/mcp-v8-heaps
```
