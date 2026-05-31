# Use Local Storage

Use a local directory for heap snapshots:

```bash
mcp-v8 --directory-path /var/lib/mcp-v8/heaps
```

If you omit all storage flags, the server defaults to a local directory under
`/tmp/mcp-v8-heaps`.
