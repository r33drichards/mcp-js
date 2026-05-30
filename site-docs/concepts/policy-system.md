# Policy System

Optional capabilities such as `fetch()`, the `fs` module, and external module
auditing are gated by policy configuration. The server builds policy chains
from `--policies-json` and evaluates requests against those chains before the
operation is allowed to proceed.

This design keeps powerful capabilities available when needed without making
them part of the default runtime surface.
