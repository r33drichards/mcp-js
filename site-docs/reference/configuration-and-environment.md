# Configuration and Environment

Important configuration inputs include:

- command-line flags in `server/src/main.rs`
- optional `JWKS_URL` for JWT verification
- AWS credentials and region resolution used by the AWS SDK when S3 storage is
  enabled
- JSON configuration files for fetch headers, WASM modules, MCP servers, and
  policy configuration

The runtime itself does not expose environment variables to user JavaScript.
Environment variables affect server configuration, not guest-code capabilities.
