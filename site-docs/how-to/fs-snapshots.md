# Filesystem snapshots

Recipes for enabling the content-addressed filesystem, mounting a snapshot in `run_js`, and driving every label operation across the three surfaces: the MCP `fs_*` tools, the `/api/fs/...` HTTP endpoints, and the `mcp-v8-cli fs ...` verbs. For the model behind these operations — the content/pointer planes, the overlay mount, and the merge design — see [Concepts: Filesystem snapshots](../concepts/fs-snapshots.md).

The HTTP and CLI examples assume a server reachable at `http://localhost:3000`; the CLI honours `--url` (or `MCP_V8_URL`), defaulting to that address.

## Enable the feature

Filesystem snapshots are off by default. Turn them on with `--enable-fs-snapshots`:

```bash
mcp-v8 --http-port=3000 --enable-fs-snapshots
```

Two stores are created. Both default under `--session-db-path` and can be overridden:

| Store | Flag | Default |
|---|---|---|
| Blob store (chunks + manifests) | `--fs-store-dir DIR` | `<session-db-path>/fs-blobs` |
| Label / reflog database (sled) | `--fs-labels-db PATH` | `<session-db-path>/fs-labels` |

```bash
mcp-v8 --http-port=3000 --enable-fs-snapshots \
       --fs-store-dir=/var/lib/mcp-v8/fs-blobs \
       --fs-labels-db=/var/lib/mcp-v8/fs-labels
```

### Cluster storage requirement

The node-local blob store is **single-node only**. In a cluster, labels replicate cluster-wide but node-local blobs do not, so a label advanced on one node would resolve on another to a manifest that node is missing. The server therefore **refuses to start** with `--enable-fs-snapshots` in cluster mode unless shared blob storage is configured. Use `--s3-bucket` (optionally with `--cache-dir` for a write-through cache):

```bash
mcp-v8 --http-port=3000 --cluster-port=4000 --node-id=node1 \
       --peers=node2@node2:4000,node3@node3:4000 \
       --enable-fs-snapshots \
       --s3-bucket=my-mcp-fs \
       --cache-dir=/var/lib/mcp-v8/fs-cache
```

With `--s3-bucket` set, blobs and tree nodes live in S3 (so every node resolves them identically), and `--cache-dir` is a **size-bounded** local write-through cache in front of it — it never tries to mirror the whole bucket, so a multi-terabyte snapshot store stays usable on a node with a modest local disk.

## Mount a snapshot in run_js

Pass the `fs` parameter to `run_js`. The handle is either a **label name** (mounted at the label's current head) or a **64-hex CA id** (mounted detached/pinned). It is independent of the `heap` parameter.

MCP tool call:

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "await fs.writeFile('/data/out.txt', 'hello'); 'done'",
    "fs": "main"
  }
}
```

The same over the REST execution endpoint, then reading back the resulting snapshot id from the execution status:

```bash
# Start the execution; the body carries an `fs` handle.
EXEC=$(curl -s -X POST http://localhost:3000/api/exec \
  -H "Content-Type: application/json" \
  -d '{"code":"await fs.writeFile(\"/data/out.txt\",\"hello\"); \"done\"","fs":"main"}' \
  | jq -r .execution_id)

# Poll status; the resulting snapshot appears as the `fs` field.
curl -s http://localhost:3000/api/executions/$EXEC | jq '{status, fs}'
# => { "status": "completed", "fs": "9f2c...<64 hex>" }
```

The `fs` field of a completed execution is the durable CA id of the snapshot the run produced. It is **not** automatically attached to any label — pushing it to a label is a separate step (`fs_push`).

### Writing a large file without buffering it

`fs.writeFile(path, value)` hands the whole `value` across in one call. For a file too big to hold in memory, use a streaming write handle — feed it in pieces (`Uint8Array` or string), then `close()`; the bytes are chunked on the fly and the file appears only after `close()`:

```js
const w = await fs.createWriteStream("/big/dump.bin");
for await (const chunk of source) {   // chunks of any size
  await w.write(chunk);
}
await w.close();
```

Copying a large file is also cheap: `fs.copyFile(src, dest)` clones the source's content-addressed entry by reference — no bytes are read or re-chunked.

## Operations table

Every operation routes through the same engine logic regardless of surface, so behaviour is identical across the three.

| Operation | MCP tool | HTTP | CLI verb |
|---|---|---|---|
| List labels | `fs_ls` | `GET /api/fs/labels` | `fs ls` |
| Resolve a label to its head | `fs_pull` | `GET /api/fs/labels/{label}` | `fs pull <label>` |
| Create / repoint a label | `fs_label` | `POST /api/fs/labels` | `fs label <name> <ca_id>` |
| Show a label's reflog | `fs_log` | `GET /api/fs/labels/{label}/log?limit=N` | `fs log <label> [--limit N]` |
| Advance a label (push) | `fs_push` | `POST /api/fs/push` | `fs push <ca_id> --label <l>` |
| Reset a label (rollback) | `fs_reset` | `POST /api/fs/reset` | `fs reset <label> <ca_id>` |
| Merge two snapshots | `fs_merge` | `POST /api/fs/merge` | `fs merge <ours> <theirs>` |

## List labels

```json
{ "tool": "fs_ls", "arguments": {} }
```

```bash
curl -s http://localhost:3000/api/fs/labels | jq
```

```bash
mcp-v8-cli fs ls
```

Returns each label name and its current head CA id.

## Resolve a label (pull)

Resolve a label to the CA id it currently points at. Use this value as the `fs` argument to `run_js` to mount the label's current head, or as the `expected` value when pushing.

```json
{ "tool": "fs_pull", "arguments": { "label": "main" } }
```

```bash
curl -s http://localhost:3000/api/fs/labels/main | jq
```

```bash
mcp-v8-cli fs pull main
```

A label that does not exist returns an error (`404` over HTTP).

## Create or repoint a label

Create a new label, or move an existing one, to a CA id. Accepts an optional `message`.

```json
{
  "tool": "fs_label",
  "arguments": { "name": "main", "ca_id": "9f2c...", "message": "seed snapshot" }
}
```

```bash
curl -s -X POST http://localhost:3000/api/fs/labels \
  -H "Content-Type: application/json" \
  -d '{"name":"main","ca_id":"9f2c...","message":"seed snapshot"}' | jq
```

```bash
mcp-v8-cli fs label main 9f2c... -m "seed snapshot"
```

## Push: advance a label

`fs_push` advances a label to a CA id — typically the `fs` value from a completed execution. The default is **reject-and-rebase**: pass `expected` (the head you pulled before the run) and the push fails if the label moved since.

```json
{
  "tool": "fs_push",
  "arguments": {
    "ca_id": "9f2c...",
    "label": "main",
    "expected": "1a4b...",
    "message": "add out.txt"
  }
}
```

```bash
curl -s -X POST http://localhost:3000/api/fs/push \
  -H "Content-Type: application/json" \
  -d '{"ca_id":"9f2c...","label":"main","expected":"1a4b..."}' | jq
```

```bash
mcp-v8-cli fs push 9f2c... --label main --expected 1a4b...
```

If the label moved since you pulled, the push is **rejected** — over HTTP this is status **`409`** with a body reporting the label's `current` head. Re-pull, rebase or merge your snapshot onto the new head, and push again.

Two explicit opt-outs of the safe default:

- **`force`** moves the label unconditionally (still recording a reflog entry):

  ```bash
  curl -s -X POST http://localhost:3000/api/fs/push \
    -H "Content-Type: application/json" \
    -d '{"ca_id":"9f2c...","label":"main","force":true}' | jq

  mcp-v8-cli fs push 9f2c... --label main --force
  ```

- **`detach`** touches no label and just echoes the CA id back (no `label` required):

  ```bash
  curl -s -X POST http://localhost:3000/api/fs/push \
    -H "Content-Type: application/json" \
    -d '{"ca_id":"9f2c...","detach":true}' | jq
  # => { "status": "detached", "ca_id": "9f2c..." }

  mcp-v8-cli fs push 9f2c... --detach
  ```

## Reset: roll a label back

`fs_reset` moves a label back to an earlier CA id from its reflog. By default the target must already appear in the label's reflog (see `fs_log`); pass `allow_unlogged` to reset to any CA id anyway.

```json
{
  "tool": "fs_reset",
  "arguments": { "label": "main", "ca_id": "1a4b...", "message": "roll back bad run" }
}
```

```bash
curl -s -X POST http://localhost:3000/api/fs/reset \
  -H "Content-Type: application/json" \
  -d '{"label":"main","ca_id":"1a4b...","message":"roll back bad run"}' | jq
```

```bash
mcp-v8-cli fs reset main 1a4b... -m "roll back bad run"
```

A CA id that is not in the reflog (and without `allow_unlogged`) returns an error (`400` over HTTP):

```bash
mcp-v8-cli fs reset main deadbeef... --allow-unlogged
```

Resetting past a snapshot does not lose it: the snapshot stays reachable through its reflog entry (so you can roll forward) until that entry ages out of the GC retention window.

## Read the reflog

`fs_log` returns a label's move history oldest-first. Each entry has `at`, `from`, `to` (CA ids), `op` (`create` / `push` / `reset` / `force`), and an optional `message`. Use a `to` value as the `ca_id` for a reset. Bound long histories with `limit` / `?limit=N`, which returns only the most recent N entries.

```json
{ "tool": "fs_log", "arguments": { "label": "main", "limit": 20 } }
```

```bash
curl -s "http://localhost:3000/api/fs/labels/main/log?limit=20" | jq
```

```bash
mcp-v8-cli fs log main --limit 20
```

## Merge two snapshots

`fs_merge` combines two snapshots (CA ids) into a new one. Pass `base` — the snapshot both sides diverged from (e.g. the label head you mounted before two runs) — so that only paths **both** sides changed conflict; omit it for a 2-way merge where any path both sides changed conflicts.

```json
{
  "tool": "fs_merge",
  "arguments": { "ours": "9f2c...", "theirs": "7b3d...", "base": "1a4b..." }
}
```

```bash
curl -s -X POST http://localhost:3000/api/fs/merge \
  -H "Content-Type: application/json" \
  -d '{"ours":"9f2c...","theirs":"7b3d...","base":"1a4b..."}' | jq
```

```bash
mcp-v8-cli fs merge 9f2c... 7b3d... --base 1a4b...
```

Text files are merged at line level: edits to different lines of the same file auto-merge cleanly. The response is one of two shapes:

- **Clean** — `{ "status": "merged", "ca_id": "..." }`. The merge produced a new snapshot; push it to a label separately (it does not move any label on its own).
- **Conflict** — `{ "status": "conflict", "conflicts": [...] }`. Each conflict carries the `path`, each side's content id (`base` / `ours` / `theirs`, with `null` meaning the file is absent on that side), a `kind` (e.g. `text`, `modify/delete`), and for text the diff3 conflict `markers` plus unified `diff_ours` / `diff_theirs`. Resolve text conflicts by editing the markers, writing the file back, and pushing.

Auto-resolve every remaining conflict to one side with `prefer`:

```bash
mcp-v8-cli fs merge 9f2c... 7b3d... --base 1a4b... --prefer ours
```

```bash
curl -s -X POST http://localhost:3000/api/fs/merge \
  -H "Content-Type: application/json" \
  -d '{"ours":"9f2c...","theirs":"7b3d...","base":"1a4b...","prefer":"theirs"}' | jq
```

Valid `prefer` values are `ours`, `theirs`, or omitted/`none` (report conflicts).

## A full round-trip

1. `fs pull main` → note the current head `H` (use as `expected`).
2. `run_js` with `fs: "main"`, then read the execution's `fs` field → snapshot `S`.
3. `fs push <S> --label main --expected <H>` → advances `main`, or returns `409` if someone else moved it first.
4. On rejection: `fs pull main` again for the new head, `fs merge <S> <new-head> --base <H>`, then push the merged id.

## Optional reflog messages

`fs_push`, `fs_reset`, and `fs_label` each accept an optional `message` — a commit-style note recorded on the reflog entry and shown in `fs_log`. Messages are capped at **4096 bytes**; a longer message is rejected. On the CLI the flag is `-m` / `--message`.

## Policy gating

When an `fs_snapshot` policy chain is configured, label-moving operations are gated through it. The policy input is:

```json
{ "op": "pull|push|reset|label", "label": "<name or null>", "ca_id": "<hex or null>" }
```

A denied operation returns an error before any pointer move happens. See [Policies](../concepts/policies.md) for how to configure a policy chain.

## See also

- [Concepts: Filesystem snapshots](../concepts/fs-snapshots.md)
- [How-to: Stateful sessions & heap snapshots](sessions-and-heaps.md)
- [How-to: Heap storage backends](storage-backends.md)
- [How-to: Clustering & replication (Raft)](clustering.md)
- [Concepts: Policies](../concepts/policies.md)
