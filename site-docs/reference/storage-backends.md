# Storage Backends Reference

## CLI Flags

| Flag | Type | Default | Conflicts | Description |
|------|------|---------|-----------|-------------|
| `--directory-path` | `string` | `/tmp/mcp-v8-heaps` | `--s3-bucket`, `--stateless` | Directory for filesystem heap storage |
| `--s3-bucket` | `string` | (none) | `--directory-path`, `--stateless` | S3 bucket name for heap storage |
| `--cache-dir` | `string` | (none) | -- | Local FS cache directory (requires `--s3-bucket`) |
| `--stateless` | `bool` | `false` | `--s3-bucket`, `--directory-path` | No heap snapshots saved or loaded |

## Backend Selection Logic

```
if --stateless:
    No heap storage (stateless mode)
elif --s3-bucket:
    if --cache-dir:
        S3 + filesystem write-through cache
    else:
        S3 only
elif --directory-path:
    Filesystem at specified path
else:
    Filesystem at /tmp/mcp-v8-heaps
```

## Filesystem Backend

- Stores heap snapshots as files in the specified directory
- File name: 64-character hex SHA-256 hash
- Directory is created automatically if it does not exist
- Default path: `/tmp/mcp-v8-heaps`

## S3 Backend

- Stores heap snapshots as S3 objects
- Object key: 64-character hex SHA-256 hash
- Bucket must already exist

### AWS Credentials

S3 credentials are loaded from the standard AWS environment:

| Variable | Description |
|----------|-------------|
| `AWS_ACCESS_KEY_ID` | AWS access key |
| `AWS_SECRET_ACCESS_KEY` | AWS secret key |
| `AWS_DEFAULT_REGION` | AWS region |
| `AWS_REGION` | AWS region (alternative) |
| `AWS_SESSION_TOKEN` | Session token (for temporary credentials) |
| `AWS_PROFILE` | AWS CLI profile name |

Credentials can also come from the AWS credentials file (`~/.aws/credentials`), EC2 instance profiles, or ECS task roles. The `aws_config::load_from_env()` function handles all standard credential sources.

## Write-Through Cache

When `--s3-bucket` and `--cache-dir` are both specified, a write-through cache is used:

| Operation | Behavior |
|-----------|----------|
| `put` | Write to local FS cache first, then to S3 |
| `get` | Check local FS cache first. On miss, fetch from S3 and populate cache. |

Cache population failures on `get` are logged as warnings but do not fail the operation.

## Storage Interface

All backends implement the `HeapStorage` trait:

```
put(name: &str, data: &[u8]) -> Result<(), String>
get(name: &str) -> Result<Vec<u8>, String>
```

- `name`: the 64-character hex content hash
- `data`: the wrapped snapshot envelope (magic + checksum + payload)
