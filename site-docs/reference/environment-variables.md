# Environment Variables Reference

## Server Environment Variables

| Variable | Used By | Description |
|----------|---------|-------------|
| `JWKS_URL` | `--jwks-url` | JWKS endpoint URL for JWT verification. Alternative to the CLI flag. |

## CLI Client Environment Variables

| Variable | Used By | Default | Description |
|----------|---------|---------|-------------|
| `MCP_V8_URL` | `mcp-v8-cli --url` | `http://localhost:3000` | Base URL of the mcp-v8 server |

## AWS Credentials (for S3 Storage)

Used when `--s3-bucket` is set. Credentials are loaded via `aws_config::load_from_env()`, which supports all standard AWS credential sources.

| Variable | Description |
|----------|-------------|
| `AWS_ACCESS_KEY_ID` | AWS access key ID |
| `AWS_SECRET_ACCESS_KEY` | AWS secret access key |
| `AWS_DEFAULT_REGION` | Default AWS region |
| `AWS_REGION` | AWS region (alternative to `AWS_DEFAULT_REGION`) |
| `AWS_SESSION_TOKEN` | Session token for temporary credentials |
| `AWS_PROFILE` | AWS CLI profile name (for credentials file lookup) |
| `AWS_SHARED_CREDENTIALS_FILE` | Path to shared credentials file (default: `~/.aws/credentials`) |
| `AWS_CONFIG_FILE` | Path to AWS config file (default: `~/.aws/config`) |
| `AWS_ROLE_ARN` | IAM role ARN for assume-role |
| `AWS_WEB_IDENTITY_TOKEN_FILE` | Path to web identity token file (for EKS pod identity) |

## Build-Time Environment Variables

Used during compilation to embed CLI binaries in the server:

| Variable | Description |
|----------|-------------|
| `MCP_V8_CLI_LINUX_X86_64` | Path to Linux x86_64 CLI binary |
| `MCP_V8_CLI_LINUX_AARCH64` | Path to Linux ARM64 CLI binary |
| `MCP_V8_CLI_MACOS_AARCH64` | Path to macOS ARM64 CLI binary |

When set, the corresponding CLI binary is embedded in the server and served via `GET /api/cli/{platform}`.
