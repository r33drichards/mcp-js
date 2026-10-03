# User-supplied skills over MCP

Publish your own [Agent Skills](https://agentskills.io/specification) from a local
folder with `--skills-dir`, `MCP_V8_SKILLS_DIR`, or `skills_dir` in the TOML/JSON
configuration file:

```bash
mcp-v8 --skills-dir ./examples/skills
# Streamable HTTP:
mcp-v8 --http-port 8080 --skills-dir /srv/mcp-skills
```

```toml
skills_dir = "/srv/mcp-skills"
```

### S3 bucket and key

Use an S3 prefix containing skill folders, or an exact `SKILL.md` key to publish
that skill and its supporting files:

```bash
mcp-v8 --skills-s3-uri s3://my-bucket/skills/
mcp-v8 --skills-s3-uri s3://my-bucket/skills/javascript-workflow/SKILL.md
```

```toml
skills_s3_uri = "s3://my-bucket/skills/"
```

The environment equivalent is `MCP_V8_SKILLS_S3_URI`. Choose either the S3 source
or `skills_dir`; combining them fails startup. Prefixes are matched at folder
boundaries. For a catalog prefix, `skills/javascript-workflow/SKILL.md` becomes
`skill://javascript-workflow/SKILL.md`. An exact entrypoint key retains its
parent skill folder's name and includes every supporting object under that folder.
Folder marker objects are ignored.

The server uses the standard AWS credential and region configuration (for example
an IAM role or `AWS_PROFILE` with `AWS_REGION`). It needs `s3:ListBucket` on the
bucket for the selected prefix and `s3:GetObject` on its objects. Existing
`AWS_ENDPOINT_URL` and `AWS_S3_FORCE_PATH_STYLE=true` settings work for S3-compatible
stores. Listings are paginated and downloads are checked against listed ETags;
access failures or changed objects fail startup. The same validation, 64 MiB limit,
startup snapshot, and restart-to-refresh behavior apply to S3. This is a read-only
source; the server does not upload skill files.

Each skill lives in a named subfolder with a `SKILL.md` entrypoint. Nested folders
are supported, for example:

```text
/srv/mcp-skills/
  javascript-workflow/
    SKILL.md
    references/
      execution.md
```

```markdown
---
name: javascript-workflow
description: Run JavaScript tasks with mcp-v8 and inspect their results.
---
Read references/execution.md before using the execution tools.
```

The `name` must match the skill folder, use lowercase letters/digits/single
hyphens, and contain at most 64 characters. A nonempty `description` is required
(maximum 1024 characters). YAML metadata and multiline descriptions are preserved.
Paths must use ASCII letters, digits, hyphens, underscores, and dots. Symlinks and
special files are rejected, and the catalog is limited to 64 MiB.

On stdio and Streamable HTTP, a nonempty catalog advertises
`io.modelcontextprotocol/skills`. Clients discover entries through `skills/list`
(paginated) and retrieve one through `skills/get`. Each entry includes a complete
file manifest with SHA-256 digests and byte sizes, including `SKILL.md` and nested
supporting files. Use `resources/read` to retrieve a file, for example
`skill://javascript-workflow/references/execution.md`. Text files return text;
binary files return base64 resource blobs. `resources/list` also exposes skill
entrypoints to clients that do not implement the extension. Directory listing is
not advertised. Files outside skill folders are not published.

Files are snapshotted at startup; restart to publish edits. The same catalog is
served to all clients connected to this process. Only point the server at skills
you intend those clients to read. Serving a skill does not execute its scripts,
change server policies, or grant client permissions. Legacy SSE does not support
this extension and rejects either skills source option.

This implementation uses the wire types from the upstream Rust SDK
[Skills extension PR #1286](https://github.com/modelcontextprotocol/rust-sdk/pull/1286),
pinned to commit `ff211dd82c488ff80669a8e13c3f82a64ced5309` from
`branben/rust-sdk` branch `feat/sep-2640-skills`. A small adapter uses the stable
SDK's custom request hook, preserving this server's existing transport and task
APIs. The server continues to negotiate its existing MCP protocol version; a
full migration to the SDK's newer base protocol is separate work.
