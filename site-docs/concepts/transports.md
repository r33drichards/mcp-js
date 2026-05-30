# Transports

The transport layer changes how clients talk to `mcp-v8`, but not what the
engine does once code reaches execution.

- stdio is process-local and subprocess-oriented
- Streamable HTTP is network-friendly and pairs naturally with the REST API
- SSE is an older HTTP-based event-stream transport

Cluster mode is only supported for HTTP and SSE. Stdio is explicitly excluded
from cluster deployments.
