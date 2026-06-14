# Filesystem snapshots

Enable the content-addressed, snapshottable filesystem:

```bash
server --stateless --http-port 3000 --enable-fs-snapshots
```

By default the blob store lives at `<session-db-path>/fs-blobs` and the
label/reflog database at `<session-db-path>/fs-labels`; override with
`--fs-store-dir` and `--fs-labels-db`. In cluster mode label writes
automatically route through the Raft leader, and each move replicates the head
pointer and its reflog entry as a single atomic entry (so the head never
advances without the reflog recording it; see `specs/FsLabelAtomicWrite`).

### Storage in a cluster

Labels and reflogs replicate cluster-wide, but the **blob store does not** — the
default node-local `--fs-store-dir` blobs are **single-node only**. In a cluster
a label advanced on one node would otherwise resolve on another to a manifest
that node is missing. So enabling fs snapshots in a cluster requires shared blob
storage via `--s3-bucket` (optionally with `--cache-dir` for a write-through
cache); the server refuses to start with node-local blobs in cluster mode.

## Mounting in `run_js`

Pass `fs` — a **label name** or a **64-hex CA id** — alongside (and independent
of) `heap`. Inside the script, the normal `fs.*` API operates on the virtual
overlay:

```bash
curl -s localhost:3000/api/exec -H 'content-type: application/json' -d '{
  "code": "await fs.writeFile(\"/data/note.txt\", \"hello\");",
  "fs": "main"
}'
```

When the execution completes, its resulting snapshot CA id is reported as the
`fs` field of the execution status (`GET /api/executions/{id}`). Pushing a label
is a separate, explicit step.

## Tools, endpoints, and CLI verbs

| Operation | MCP tool | HTTP | CLI |
|-----------|----------|------|-----|
| List labels | `fs_ls` | `GET /api/fs/labels` | `mcp-v8-cli fs ls` |
| Resolve a label | `fs_pull` | `GET /api/fs/labels/{label}` | `mcp-v8-cli fs pull <label>` |
| Create / repoint a label | `fs_label` | `POST /api/fs/labels` | `mcp-v8-cli fs label <name> <ca>` |
| Show the reflog | `fs_log` | `GET /api/fs/labels/{label}/log` | `mcp-v8-cli fs log <label>` |
| Advance a label | `fs_push` | `POST /api/fs/push` | `mcp-v8-cli fs push <ca> --label <l>` |
| Reset to an earlier id | `fs_reset` | `POST /api/fs/reset` | `mcp-v8-cli fs reset <label> <ca>` |
| Merge two snapshots | `fs_merge` | `POST /api/fs/merge` | `mcp-v8-cli fs merge <ours> <theirs>` |

### Merge (three-way)

`fs_merge` combines two snapshots into a new one. Pass `base` — the snapshot
both sides diverged from (e.g. the label head you mounted before two runs) — so
only paths **both** sides changed conflict; omit it for a 2-way merge. A clean
merge returns the merged snapshot's `ca_id` (push it to a label separately); a
conflict returns `status: "conflict"` with the conflicting paths and each side's
content id (`null` = the file is absent on that side):

```bash
curl -s localhost:3000/api/fs/merge -H 'content-type: application/json' -d '{
  "ours": "<ca-A>", "theirs": "<ca-B>", "base": "<common-ancestor-ca>"
}'
```

**Type-aware content merge.** When the same path changed on both sides, a
type-specific merger gets a shot before reporting a conflict. **Text** files are
merged at line level (diff3): edits to *different lines* of the same file merge
cleanly and never reach you. A genuine same-line clash is reported with
`kind: "text"`, the diff3 conflict `markers` (`<<<<<<< ======= >>>>>>>`), and
unified `diff_ours` / `diff_theirs`, so you can resolve at line level — edit the
marker text, write it back with `run_js`, and push. Binary files (and SQLite
databases, detected but not yet auto-merged) report a whole-file conflict with
their `kind`; richer per-type drivers plug in behind the same extension point.

Set `prefer` to `ours` or `theirs` to auto-resolve any remaining conflicts to
that side. The structural merge itself is a flat per-path 3-way; renames and a
recursive virtual base for criss-cross histories remain out of scope.

### Push (reject-and-rebase)

`fs_push` advances a label to a CA id — typically the `fs` value from a
completed execution. Pass `expected` (the head you pulled) so the push is
rejected (HTTP `409`) if the label moved since:

```bash
curl -s localhost:3000/api/fs/push -H 'content-type: application/json' -d '{
  "label": "main", "ca_id": "<ca>", "expected": "<head-you-pulled>"
}'
```

`force: true` overrides the conflict; `detach: true` returns the CA id without
touching any label.

Pass an optional `message` — a commit-style note — on `push`, `reset`, or
`label` to annotate the move. It is stored on the reflog entry and shown by
`fs_log` (the `message` field). Messages are capped at 4096 bytes. `fs_push`,
`fs_label`, and the `POST /api/fs/{push,labels,reset}` bodies all accept it:

```bash
curl -s localhost:3000/api/fs/push -H 'content-type: application/json' -d '{
  "label": "main", "ca_id": "<ca>", "expected": "<head-you-pulled>",
  "message": "apply migration 0007"
}'
```

### Reset (rollback)

`fs_reset` moves a label to an earlier CA id from its reflog (see `fs_log`). The
target must appear in the reflog unless `allow_unlogged` is set.

### Reading the reflog

`fs_log` returns the full move history oldest-first. The reflog is append-only,
so a long-lived label accumulates one entry per move; pass `limit` (MCP/CLI) or
`?limit=N` (HTTP) to read only the most recent N entries and bound the scan:

```bash
curl -s 'localhost:3000/api/fs/labels/main/log?limit=20'
mcp-v8-cli fs log main --limit 20
```

## Policy gating

Pointer moves can be gated by an `fs_snapshot` policy namespace. The policy
input is `{ "op": "pull"|"push"|"reset"|"label", "label": "...", "ca_id": "..." }`,
evaluated through the usual local-Rego / remote-OPA chain. See
[Security policies](policies.md).
