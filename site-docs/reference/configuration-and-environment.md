# Configuration and Environment

Important configuration inputs include:

- command-line flags in `server/src/main.rs`
- `--jwks-url` for JWT verification, with `JWKS_URL` as an environment-variable
  fallback
- AWS credentials and region resolution used by the AWS SDK when S3 storage is
  enabled
- JSON configuration files for fetch headers, WASM modules, MCP servers, and
  policy configuration
- inline JSON or file-path input passed to `--policies-json`

The runtime itself does not expose environment variables to user JavaScript.
Environment variables affect server configuration, not guest-code capabilities.
