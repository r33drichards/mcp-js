# How to Configure Storage Backends

Choose where heap snapshots are stored: local filesystem, S3, or not at all.

## Local filesystem (default)

Store heap snapshots in a directory:

```bash
mcp-v8 --directory-path /var/lib/mcp-v8/heaps
```

## Amazon S3

Store heap snapshots in an S3 bucket:

```bash
mcp-v8 --s3-bucket my-heap-bucket
```

AWS credentials are read from the standard environment variables (`AWS_ACCESS_KEY_ID`, `AWS_SECRET_ACCESS_KEY`, `AWS_REGION`) or instance profile.

### S3 with local cache

Add a local write-through cache for faster reads:

```bash
mcp-v8 --s3-bucket my-heap-bucket --cache-dir /tmp/heap-cache
```

`--cache-dir` requires `--s3-bucket`.

## Stateless mode

Skip heap persistence entirely. Each execution runs in a fresh isolate:

```bash
mcp-v8 --stateless
```

## Storage flag constraints

The three storage modes are mutually exclusive:

- `--directory-path` conflicts with `--s3-bucket` and `--stateless`
- `--s3-bucket` conflicts with `--directory-path` and `--stateless`
- `--stateless` conflicts with `--directory-path` and `--s3-bucket`

If none are specified, the server runs in stateful mode with a default local storage path.

## Session database

The session log database is separate from heap storage. Configure its path with:

```bash
mcp-v8 --session-db-path /var/lib/mcp-v8/sessions
```

Default: `/tmp/mcp-v8-sessions`
