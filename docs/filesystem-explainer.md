# The mcp-v8 Filesystem, Explained

*A comprehensive deep-dive into how filesystem access works in mcp-v8 — from the policy-gated `fs` API that sandboxed JavaScript sees, down to the git-like content-addressed snapshot engine underneath.*

---

## Part 0: Context — what is mcp-v8?

**mcp-v8** is a [Model Context Protocol](https://modelcontextprotocol.io) server, written in Rust, that gives an AI agent a single powerful tool: `run_js`. Instead of wiring up dozens of narrow tools, the agent writes JavaScript or TypeScript and runs it in a **sandboxed V8 isolate** — looping, branching, transforming data, calling other tools.

Two design principles shape everything in this document:

1. **Secure by default.** Host capabilities — network (`fetch`), filesystem, subprocess, WebAssembly, module imports — are all **off by default**. Each is unlocked only by an explicit [OPA/Rego](https://www.openpolicyagent.org/) policy.
2. **Durable state.** In stateful mode, the V8 heap is saved as a content-addressed snapshot, so an agent can build up variables and objects across many calls. The filesystem gets the *same treatment*: an entire directory tree can be snapshotted, identified by a hash, restored, branched, and merged.

The filesystem story therefore has **two layers stacked together**:

- **Layer 1 — the capability:** a Node.js-compatible `fs` API exposed to sandboxed JS, where *every single call* is checked against a security policy before any I/O happens.
- **Layer 2 — the backends:** what that `fs` API actually reads and writes. Either the **real host disk**, or a **virtual, snapshottable, content-addressed filesystem** — a git-like object store with branches ("labels"), a reflog, three-way merges, and garbage collection.

We'll build up from the JS API, through the policy engine, and then go deep on the snapshot system — which is the most architecturally interesting part of the project.

---

## Part 1: The `fs` global is an opt-in capability

### No policy, no filesystem

If the server starts **without** a `filesystem` entry in its `--policies-json` configuration, the `fs` global is *never injected* into the V8 isolate. Any JavaScript that references `fs` throws:

```
ReferenceError: fs is not defined
```

This is the project's capability model in action: the sandbox has no ambient authority. Code cannot even *ask* about the filesystem unless the operator has explicitly configured a policy that governs it.

When a `filesystem` policy chain **is** configured, the server calls `inject_fs()` during runtime setup, which installs `globalThis.fs` as a collection of async methods. The JS layer is deliberately thin — it validates argument types and dispatches to native Rust ops built on `deno_core`. All real I/O and all policy evaluation happen on the Rust side (in `server/src/engine/fs.rs`). Binary data crosses the JS/Rust boundary directly as `Uint8Array` through deno_core's native buffer support — no base64 encoding overhead.

### The API surface: Node.js compatibility

All methods are **async** and must be `await`ed. The API deliberately mirrors Node's `fs.promises`:

| Method | What it does | Notes |
|---|---|---|
| `fs.readFile(path, encoding?)` | Read a whole file | Returns `Uint8Array` by default; a string when a text encoding like `"utf8"` is given — exactly Node's semantics |
| `fs.writeFile(path, data)` | Create/truncate and write | `data` may be a string or `Uint8Array` (raw bytes); anything else is coerced with `String(data)` |
| `fs.appendFile(path, data)` | Append text; creates the file if missing | |
| `fs.readdir(path)` | List directory entries | Returns entry *names*, not full paths |
| `fs.stat(path)` | File/directory metadata | Follows a final symlink; returns a Node `fs.Stats`-like object |
| `fs.lstat(path)` | Like `stat` but does **not** follow a final symlink | `isSymbolicLink()` is `true` for a link itself |
| `fs.mkdir(path, {recursive})` | Create a directory | `recursive: true` behaves like `mkdir -p` |
| `fs.rm(path, {recursive})` | Remove a file or directory | Non-recursive removal of a non-empty directory fails (`ENOTEMPTY`) |
| `fs.unlink(path)` | Remove a file | Alias for `fs.rm` without recursion |
| `fs.rmdir(path, {recursive})` | Remove a directory | Provided for Node compatibility; same backend op as `rm` |
| `fs.rename(oldPath, newPath)` | Rename/move a file or directory | |
| `fs.copyFile(src, dest)` | Copy a file, overwriting the destination | On the snapshot backend this is a *by-reference* clone — no bytes are re-read |
| `fs.symlink(target, path)` | Create a symlink | Node argument order: **target first** |
| `fs.readlink(path)` | Read a symlink's target | `EINVAL` if the path is not a symlink |
| `fs.exists(path)` | Boolean existence check | |
| `fs.createWriteStream(path)` | Streaming write handle | `write()` pieces in, `close()` finalises — for files too big to buffer |

The `Stats` object returned by `stat`/`lstat` matches Node's `fs.Stats`: `isFile()`, `isDirectory()`, `isSymbolicLink()` as **methods**; `size`, `mode`, `ino`, `dev`, `nlink`, `uid`, `gid`; timestamps as both milliseconds (`mtimeMs`, `atimeMs`, `ctimeMs`, `birthtimeMs`) and `Date` objects; plus a `readonly` boolean. POSIX fields that a virtual snapshot can't represent are reported as `0`/`1`.

### `fs.promises` and error codes — why real libraries just work

Two compatibility details make the sandbox `fs` a drop-in for libraries that expect Node:

1. **`fs.promises`** exists as an enumerable own property exposing the same promise-returning methods. Libraries detect a Node-style `fs` by reading `fs.promises` and binding its methods — that detection succeeds here.
2. **Node-style error codes.** Rejections carry a `code` property where a POSIX code is known: `ENOENT`, `EEXIST`, `EACCES`, `ENOTDIR`, `EISDIR`, `ENOTEMPTY`, `EROFS`, `ENOSYS`, `EINVAL`. Callers can branch on `err.code === "ENOENT"` exactly as they would in Node.

The flagship demonstration: **isomorphic-git runs unmodified against the sandbox filesystem.**

```js
import git from "npm:isomorphic-git";
await git.init({ fs, dir: "/repo" });          // pass the sandbox `fs` directly
await fs.promises.writeFile("/repo/README.md", "# hi");
await git.add({ fs, dir: "/repo", filepath: "README.md" });
```

A full git implementation, running inside a V8 sandbox, on top of a virtual filesystem — because the `fs` interface is faithful enough.

---

## Part 2: Every call is policy-checked

### Per-operation evaluation

Every `fs.*` call — not just the first, every one — is individually evaluated by the policy chain **before any I/O is performed**. The Rust op builds a JSON input object and asks the chain to allow or deny:

| Field | Type | Present for | Description |
|---|---|---|---|
| `operation` | string | all calls | The method name: `readFile`, `writeFile`, `appendFile`, `readdir`, `stat`, `lstat`, `mkdir`, `rm`, `rename`, `copyFile`, `symlink`, `readlink`, `exists` |
| `path` | string | all calls | The primary path argument (for `symlink`, the link being created) |
| `destination` | string | `rename`, `copyFile`, `symlink` | The second path (`newPath` / `dest` / the link `target`) |
| `recursive` | boolean | `mkdir`, `rm` | Reflects `options.recursive` |
| `encoding` | string | `readFile` | `"utf8"` (text mode) or `"buffer"` (binary mode) |
| `mcp_headers` | object | when configured | MCP session headers forwarded by the server |

The Rego entrypoint is `data.mcp.filesystem.allow`. Denial throws a JavaScript error — `fs.readFile denied by policy: <path> is not allowed` — and the syscall is **never invoked**.

Example policy input for a text read:

```json
{
  "operation": "readFile",
  "path": "/var/mcp-workspace/config.json",
  "encoding": "utf8"
}
```

### The policy chain

The `filesystem` key in `--policies-json` accepts a chain of one or more evaluators:

- **`mode`**: `"all"` (default — every evaluator must allow) or `"any"` (one allow suffices).
- **`policies`**: each source is either a local Rego file or directory (`file:///path/to/policy.rego`, evaluated in-process via the `regorus` engine) or a **remote OPA server** (`http://…` / `https://…`, with a configurable `policy_path` defaulting to `mcp/filesystem`).

So an operator can mix a local baseline policy with a centrally managed OPA instance, and require both to agree.

### The call flow, end to end

For a gated `fs.readFile("/data/x.txt", "utf8")`:

1. **JS (V8):** the thin `fs` wrapper validates that `path` is a string and calls the native op `op_fs_read_file_text`.
2. **Rust ops layer:** builds the policy input `{operation:"readFile", path:"/data/x.txt", encoding:"utf8"}`.
3. **Policy chain (Rego/OPA):** evaluates. If *deny* → the op returns an error, JS sees a rejected promise, no I/O ever happened.
4. If *allow* → the op performs the read against whichever backend is active (real disk via `tokio::fs`, or the overlay mount — see Part 3), validates UTF-8 in text mode, and resolves the promise.

One engineering wrinkle worth knowing: mount-backed operations must run on deno's current-thread runtime (the overlay uses `deno_unsync`, which asserts a current-thread executor), while real-filesystem operations are offloaded to the multi-threaded tokio pool via `tokio::spawn`. The code paths are explicitly split for this.

### Security model — and its sharp edges

The documentation is refreshingly honest about the boundaries:

- **The policy is the only sandbox.** There is no chroot, no mount namespace, no OS-level isolation. The process can reach whatever the host user can; the Rego policy is the *sole* control point. A careless `default allow = true` grants everything.
- **Paths are not normalized before evaluation.** The policy sees the exact string JS provided. A policy checking `startswith(input.path, "/safe/dir/")` is bypassed by `"/safe/dir/../../etc/passwd"` unless the Rego also rejects `..` components. Production policies must validate this.
- **Two-path operations need two checks.** `rename` and `copyFile` supply both `path` and `destination`; a policy checking only `input.path` leaves the destination unconstrained. The shipped example (`policies/filesystem.rego`) demonstrates the correct `check_destination` pattern.
- **`unlink` is `rm` to the policy.** Internally `fs.unlink` delegates to the same op as `fs.rm` with `recursive=false`, so policies see `operation = "rm"` — never `"unlink"`.
- **Session-scoped confinement.** When MCP session headers are forwarded as `mcp_headers`, a policy can tie filesystem access to the authenticated identity — e.g. confining each session to its own directory subtree.

---

## Part 3: Two backends — real disk, or the overlay mount

Every filesystem op begins by checking whether the current session has a **mount handle** attached (`FsMountHandle` in the deno_core op state):

- **No mount** → the op targets the **real host filesystem** via `tokio::fs`, with whatever OS permissions the server process holds. Policy is still checked first.
- **Mount attached** → the op resolves against an **in-process overlay** of a content-addressed snapshot (Part 4). No kernel overlayfs, no FUSE — pure userspace.

There is also a **passthrough** option (off by default): with a mount attached and passthrough on, a read that *misses* the overlay falls through to the real filesystem as a **read-only lower layer** (still policy-gated). This is how a deployment can serve bundled read-only assets (say, `/opt/languages`) from the real disk while keeping `/work` as the per-session snapshot overlay. With passthrough off, the overlay is the entire filesystem view, and a miss is simply `ENOENT`.

---

## Part 4: The snapshot filesystem — a git for directory trees

This is the heart of the design. Enable it with:

```bash
mcp-v8 --http-port=3000 --enable-fs-snapshots
```

Two stores are created (both default under `--session-db-path`, both overridable):

| Store | Flag | Holds |
|---|---|---|
| Blob store | `--fs-store-dir` | File chunks and directory tree nodes |
| Label/reflog database (sled) | `--fs-labels-db` | Label heads and their move history |

A **snapshot** is an immutable directory tree identified by a hash. A **label** is a movable, human-readable name that points at one. If you know git, the mapping is: snapshot ≈ commit tree, label ≈ branch, reflog ≈ reflog, push with compare-and-set ≈ `git push --ff-only`.

### The two-plane model

The entire design rests on separating **what the bytes are** from **which snapshot is current**:

| Plane | Holds | Keyed by | Mutability | Coordination needed |
|---|---|---|---|---|
| **Content plane** | File chunks + directory tree nodes | blake3 hash of the content | Immutable | **None** — any node can write, idempotently |
| **Pointer plane** | Labels + per-label reflog | Human-readable name | Mutable | **The only coordinated write in the system** |

Because a content-addressed id always names the same bytes, writing a chunk or tree node is idempotent — two nodes writing "the same" object cannot conflict. All lineage ("this snapshot came after that one") lives exclusively in the pointer plane. This split is what later makes clustering tractable (Part 7).

### The content plane: chunks, trees, and structural sharing

**How a file is stored.** Each file is one `Entry` (mode, size, content, optional symlink target) held in its directory's tree node. Content is stored one of two ways by size:

- **Small files (≤ 64 KiB)** are *inlined* directly into the tree-node entry — no separate blob, no round-trip.
- **Large files** are split into **content-defined chunks** using FastCDC. Each chunk is hashed by its *plaintext* bytes (blake3) and stored — optionally zstd-compressed — as a blob keyed by that hash. The file's entry holds the ordered list of chunk hashes.

Why *content-defined* chunking rather than fixed-size blocks? Because chunk boundaries are determined by the data itself. Insert ten bytes in the middle of a 2 GB file and only the chunks around the edit change; everything downstream keeps its boundaries and therefore its hashes. Successive snapshots of a slowly-changing large file share almost all of their storage.

Chunking is **streamed and push-based**: bytes are fed in arbitrary pieces and emitted as complete chunks on the fly, with only a small bounded carry buffer. This is what `fs.createWriteStream` exposes to scripts — a multi-gigabyte file can be ingested without ever holding it in memory:

```js
const w = await fs.createWriteStream("/big/dump.bin");
for await (const chunk of source) {
  await w.write(chunk);         // any size pieces
}
await w.close();                // file appears atomically on close
```

The same incremental chunker also backs plain `writeFile`, so every write path produces identical chunk boundaries — consistent dedup regardless of how the bytes arrived. And `fs.copyFile(src, dest)` clones the source's entry *by reference* — same chunk list, zero bytes read or re-chunked.

**How a directory tree is stored.** A snapshot is a **recursive tree of content-addressed nodes** — one node per directory level, each mapping a single path component to a child. A child may carry a file entry, a subtree hash, or *both* (preserving the corner case where one name is simultaneously a file and a directory prefix). Each node is keyed by the blake3 hash of its binary (`bincode`) encoding.

Two powerful properties fall out of hashing every node:

1. **Deduplication at every level.** Identical chunks, identical files, and identical *whole subtrees* each exist exactly once in storage, shared across all snapshots that contain them.
2. **Structural sharing.** Editing one file rewrites only the nodes on the *spine* from that file up to the root. Every untouched subtree keeps its existing hash and is shared with the previous snapshot. A snapshot "copy" is O(changed paths), never O(tree).

Nodes carry no parent pointers and no history — they are *pure content*. The blob backend is the **same storage** the V8 heap snapshots use, with `fschunk:` and `fstree:` key prefixes so fs objects and heap objects never collide and GC can tell them apart.

### The overlay mount: ephemeral working copy, durable snapshot

When a `run_js` execution mounts a snapshot, nothing is materialised on disk. The mount (`SessionMount`) is an in-process overlay with two layers:

- a **read-only base** — the snapshot root hash, *pinned at mount time*;
- a **mutable upper** — a map of pending changes: `Data` entries (new/modified file content) and `Whiteout` entries (deletions).

Reads consult the upper first — a `Data` entry shadows the base; a `Whiteout` means "deleted" — and otherwise fall through to the base. A write performs a *copy-up*: the new bytes go into the content plane and a `Data` entry lands in the upper. Directories are **implicit** — a path is a directory if anything lives under it — so `mkdir` is effectively a no-op that exists to keep mkdir-then-write code happy. Recursive directory `rename` rewrites the path prefix of every descendant. Removing a non-empty directory without `recursive` fails with `ENOTEMPTY`, mirroring Node.

Crucially, **the base is never loaded in full**. A read lazily walks only the tree nodes along its own path (backed by a small bounded node cache). Mounting and using a multi-terabyte snapshot costs work proportional to the paths actually touched — O(touched), not O(total).

Two properties define the mount's lifecycle:

- **Pinning isolates the session.** Because `pull` pins the root hash once at mount time, other sessions moving the label concurrently can never shift the filesystem under a running execution.
- **The mount is ephemeral; the snapshot is durable.** The upper layer is never persisted as-is. When the execution finishes, `push` *folds* the upper over the base — applying every `Data`, removing every `Whiteout` — into a **new pure snapshot**, writing only the spine nodes of each change, and returns the new root id. That id is the durable artifact; the mount is discarded.

### Mounting from JavaScript's point of view

The `run_js` tool takes an `fs` parameter — either a **label name** (mount that label's current head) or a **64-hex CA id** (mount a specific snapshot, detached). It is fully independent of the `heap` parameter; a call can carry either, both, or neither, and each is restored and captured separately.

```json
{
  "tool": "run_js",
  "arguments": {
    "code": "await fs.writeFile('/data/out.txt', 'hello'); 'done'",
    "fs": "main"
  }
}
```

When the execution completes, its status reports the resulting snapshot id as the `fs` field:

```json
{ "status": "completed", "fs": "9f2c…<64 hex>" }
```

Note what does **not** happen: the new snapshot is *not* automatically attached to any label. Producing a snapshot and advancing a pointer are deliberately separate steps.

### Advancing a label: the push conflict model

Making a fresh snapshot the new head of a label is a coordinated pointer move with a deliberately safe default. Three behaviours exist:

- **Reject-and-rebase (the default).** The push carries the head the caller started from (`expected`) and succeeds only if the label still points there — an atomic **compare-and-set**. If another session advanced the label in the meantime, the push is rejected (HTTP `409`), and the caller is expected to re-pull, re-apply or merge, and retry. This is the optimistic-concurrency discipline of `git push --ff-only`: two agents racing to advance the same label cannot silently clobber each other — one wins the CAS, the other is told to rebase.
- **Force.** Explicit opt-in; moves the label unconditionally, skipping the compare. Still records a reflog entry.
- **Detach.** Explicit opt-in; returns/echoes the CA id without touching any label — useful for fan-out experiments that want snapshot ids but no shared pointer moves.

### The reflog, reset, and garbage collection

Every label move appends a `RefLogEntry` to that label's **append-only reflog**: a timestamp (`at`), the previous head (`from`), the new head (`to`), the operation (`create` / `push` / `reset` / `force`), and an optional commit-style `message` (capped at 4096 bytes so a single push can't bloat the log).

**Reset** is the rollback verb: it moves a label back to an earlier CA id. By default the target must already appear in the label's reflog — rollback stays within recorded history — with an explicit `allow_unlogged` escape hatch. Reset records its *own* reflog entry, so rolling back is itself history, and can be rolled forward.

**Garbage collection** is a reflog-rooted **mark-and-sweep**. The roots are:

1. every label's current head, **plus**
2. every CA id still named by a `to` field in any retained reflog entry (within the retention window).

From each root, GC walks the snapshot tree, marking every tree node and every file's chunk blobs, visiting each shared subtree only once — so marking is O(reachable nodes), not O(snapshots × tree). Any *fs-prefixed* blob left unmarked is swept; heap snapshots in a shared backend are never touched.

This yields the safety property that makes reset trustworthy: **a snapshot you reset past remains reachable through its reflog entry**, so you can always roll forward — right up until that entry ages out of the retention window. Only then does the orphaned snapshot become collectable.

### Merging snapshots

Two snapshots that diverged from a common ancestor can be merged into a new one.

**Structure: a recursive tree-diff that prunes by hash.** The merge walks the three trees (base, ours, theirs) together and skips whole subtrees the moment hashes agree — if both sides' subtree hashes match, or one side equals the base, that subtree is taken wholesale *without being loaded*. It descends only where all three differ, so cost is O(differing paths), not O(tree). The merged result is a new tree that shares every unchanged subtree node with its inputs.

**Decision rule (per path).** With a base (three-way):

- `ours == theirs` → take it (covers both-added-identical and both-deleted);
- `ours == base` → take theirs (only they changed);
- `theirs == base` → take ours (only we changed);
- otherwise → **conflict** (both diverged, or modify-vs-delete).

The decisions mirror Mercurial's `manifestmerge` (a flat per-path rule) rather than git's algorithm, but with git-like tree-pruning cost. Omitting the base gives a true two-way merge: any path both sides changed (and that differs) conflicts.

**Content-level rescue.** Before a divergent path is reported as a conflict, a **type-aware content merge** gets a shot — a model borrowed from Irmin's mergeable types. The file's bytes are sniffed; text files go through a **line-level three-way merge** (diff3, via the `diffy` crate), so edits to *different lines of the same file* reconcile silently. Binary types — including SQLite databases — are detected and labelled but not auto-merged; they are the designed extension point where richer drivers (e.g. a SQLite changeset merger) could plug in.

**Unresolved conflicts are structured data**, per path: a `kind`, each side's content id (`null` meaning absent on that side), and for text the diff3 conflict `markers` plus unified `diff_ours`/`diff_theirs`. A `prefer=ours|theirs` option auto-resolves whatever remains. A clean merge writes a normal snapshot and returns its CA id — and, like push-of-a-snapshot, it never moves a label on its own.

---

## Part 5: The three surfaces

Every label operation is exposed identically across three surfaces — MCP tools (for agents), HTTP endpoints (for services), and a CLI (for humans) — all routing through the same engine logic:

| Operation | MCP tool | HTTP | CLI |
|---|---|---|---|
| List labels | `fs_ls` | `GET /api/fs/labels` | `mcp-v8-cli fs ls` |
| Resolve label → head id | `fs_pull` | `GET /api/fs/labels/{label}` | `fs pull <label>` |
| Create/repoint a label | `fs_label` | `POST /api/fs/labels` | `fs label <name> <ca_id>` |
| Show a label's reflog | `fs_log` | `GET /api/fs/labels/{label}/log?limit=N` | `fs log <label>` |
| Advance a label (push) | `fs_push` | `POST /api/fs/push` | `fs push <ca_id> --label <l>` |
| Reset a label (rollback) | `fs_reset` | `POST /api/fs/reset` | `fs reset <label> <ca_id>` |
| Merge two snapshots | `fs_merge` | `POST /api/fs/merge` | `fs merge <ours> <theirs>` |

### The canonical agent workflow

1. **Pull:** `fs_pull main` → returns head CA id `A`.
2. **Run:** `run_js` with `fs: "main"` (or `fs: A`) — code reads and writes through the overlay mount.
3. **Collect:** execution status reports the new snapshot id `B` in its `fs` field.
4. **Push:** `fs_push` with `ca_id: B, label: "main", expected: A`.
   - If `main` still points at `A` → CAS succeeds, `main` now points at `B`, reflog records `A → B`.
   - If another agent moved `main` to `C` meanwhile → `409`. Re-pull, `fs_merge` `B` with `C` (base `A`), push the merged result.

This is exactly the git fetch/commit/push loop, reconstituted for agent filesystems.

---

## Part 6: Sessions, heaps, and how fs fits the bigger picture

The filesystem snapshot and the **V8 heap snapshot** are orthogonal, parallel features:

- A heap snapshot persists the JavaScript *memory* — variables, objects, closures — content-addressed in the same blob backend.
- An fs snapshot persists a *directory tree* — content-addressed with `fschunk:`/`fstree:` prefixes in that same backend.
- A `run_js` call may carry a `heap` handle, an `fs` handle, both, or neither. Each is restored before execution, captured after, and reported independently in the execution result.

So an agent can maintain long-lived program state *and* a long-lived workspace, version both, and wind either back independently.

---

## Part 7: Clustering — why the two-plane split matters

mcp-v8 supports Raft-replicated clustering, and the filesystem's two planes replicate in completely different ways:

**The pointer plane is the only replicated state.** Label moves route through the Raft leader, making every compare-and-set **linearizable cluster-wide**. Critically, a label move replicates the new head **and** its reflog entry as a *single atomic replicated entry* — the head can never advance without the reflog recording it.

That guarantee is not just asserted — it is **model-checked with TLA+** (`specs/FsLabelAtomicWrite.tla`). The model demonstrates that the naive design — writing the head and the reflog as two separate replicated entries — violates the invariant "a settled head is always covered by the reflog," while the combined single-entry design holds. This matters because *reset* and *reflog-rooted GC* both depend on the reflog covering every head a label has ever pointed at: a head that moved without a reflog record would be silent corruption — an unrollbackable state and a GC root leak.

**The content plane is node-local unless shared storage is configured.** Blobs written on node A don't exist on node B, so a label advanced on A would dangle on B. The server therefore **refuses to start** with `--enable-fs-snapshots` in cluster mode unless shared blob storage is configured — `--s3-bucket`, optionally fronted by `--cache-dir`, a *size-bounded* local write-through cache (it never mirrors the whole bucket, so a multi-terabyte snapshot store works from a node with a modest disk). With shared storage, content-addressing does its job: every node resolves every hash identically, with zero coordination.

---

## Part 8: Honest boundaries — what is deliberately *not* built

The docs maintain an explicit non-goals list; these are scope decisions, not oversights:

- **No materialised real-path mode, no reflink copy-up.** The mount is the in-process virtual overlay only — never real files on disk.
- **No rename/move detection in merge.** The merge is per-path; a moved file is a delete plus an add.
- **Some whole-file operations remain.** Streaming writes exist, but `fs.readFile`/`fs.writeFile` hand the entire value across in one call (use `createWriteStream` for big writes), and the merge's content step reads each *conflicting* file fully to reconcile it.
- **Binary/SQLite merge drivers are stubbed.** They're detected and labelled; only the text diff3 driver actually reconciles content.
- **No CRDT semantics.** Concurrency is reject-and-rebase plus explicit merge — not automatic conflict-free convergence.
- **No cross-label sharing UI.** Sharing a snapshot between labels is just pushing the same CA id to both.

---

## Part 9: Key takeaways

1. **Capability, not permission.** The `fs` global doesn't exist unless a policy grants it, and every single call re-consults that policy before any I/O. The policy is also the *only* sandbox — there's no chroot underneath, and raw path strings mean `..` traversal is the policy author's problem.
2. **Node-faithful on purpose.** `fs.promises`, `Uint8Array`-by-default reads, `Stats` methods, and POSIX error codes exist so real libraries — up to and including isomorphic-git — run unmodified inside the sandbox.
3. **Content and pointers are different problems.** Immutable, hash-addressed content needs no coordination ever; a handful of mutable label pointers need careful coordination (mutex on one node, Raft in a cluster). Splitting them is the design.
4. **The mount is a working copy; the snapshot is the commit.** An execution's overlay is ephemeral and pinned; the durable artifact is the folded snapshot's hash, and attaching it to a label is a separate, explicitly-safe (CAS) step.
5. **Everything is O(what you touched).** Lazy tree walks, spine-only rewrites, hash-pruned merges, and shared-subtree-aware GC all scale with the change, not the tree.
6. **History is the safety net.** The reflog makes every label move recorded and reversible, GC roots itself in that history, and the atomicity of "head + reflog entry" is proven with TLA+, not just hoped for.

---

## Glossary

| Term | Meaning |
|---|---|
| **CA id** | Content-addressed identifier — the blake3 hash (64 hex chars) naming a chunk, tree node, or snapshot root |
| **Snapshot** | An immutable directory tree, identified by the hash of its root tree node |
| **Label** | A mutable, human-readable name pointing at a snapshot — analogous to a git branch |
| **Reflog** | Per-label append-only history of every pointer move (`create`/`push`/`reset`/`force`) |
| **Mount / overlay** | The ephemeral in-process view an execution reads and writes: pinned read-only base + mutable upper layer |
| **Whiteout** | An upper-layer marker meaning "this base path is deleted" |
| **Copy-up** | Writing a modified file's content into the content plane and shadowing the base with an upper `Data` entry |
| **Push (of a mount)** | Folding the upper layer over the base into a new pure snapshot, returning its root id |
| **Push (of a label)** | Advancing a label to a CA id, by default via compare-and-set against an `expected` head |
| **Detach** | Producing/holding a snapshot id without moving any label |
| **Reset** | Rolling a label back to an earlier id from its reflog |
| **FastCDC** | Content-defined chunking algorithm — boundaries derive from the data, so local edits re-chunk only locally |
| **Structural sharing** | Unchanged subtrees keep their hash and are shared between snapshots — only the spine of a change is rewritten |
| **Two-plane model** | The separation of immutable content (no coordination) from mutable pointers (the only coordinated write) |
| **OPA / Rego** | Open Policy Agent and its policy language — how filesystem access is granted and constrained |
| **deno_core** | The Rust crate embedding V8 that hosts the sandbox and its native ops |
| **regorus** | The in-process Rego evaluator used for local policy files |
| **sled** | The embedded database storing labels and reflogs |
| **blake3** | The hash function used for all content addressing |
| **diff3** | The three-way text merge algorithm used by the content merger |

---

## Appendix: Where things live in the repository

| Path | Contents |
|---|---|
| `server/src/engine/fs.rs` | The deno_core ops: argument handling, policy checks, backend dispatch (mount vs real disk), streaming writers |
| `server/src/engine/fs_chunker.rs` | FastCDC content-defined chunking; 64 KiB inline threshold; streaming chunker |
| `server/src/engine/fs_tree.rs` | The recursive content-addressed tree: `TreeNode`/`TreeChild`, blake3-of-bincode keys, structural sharing |
| `server/src/engine/fs_store.rs` | The object store over the shared blob backend; `fschunk:`/`fstree:` prefixes; lazy tree walking |
| `server/src/engine/fs_mount.rs` | `SessionMount`: the pinned-base + upper-layer overlay; pull and push |
| `server/src/engine/fs_labels.rs` | Labels, reflog, CAS pointer moves; sled persistence; Raft routing in clusters |
| `server/src/engine/fs_merge.rs` | The per-path three-way/two-way merge decision rule |
| `server/src/engine/fs_content_merge.rs` | Type sniffing and the diff3 text merger; binary/SQLite detection |
| `server/src/engine/fs_gc.rs` | Reflog-rooted mark-and-sweep garbage collection |
| `policies/filesystem.rego` | Example policy, including correct destination checking and session confinement |
| `specs/FsLabelAtomicWrite.tla` | TLA+ model proving head+reflog atomicity in the replicated design |
| `site-docs/concepts/filesystem.md` | Concepts: policy gating and the capability model |
| `site-docs/concepts/fs-snapshots.md` | Concepts: the two-plane snapshot design |
| `site-docs/reference/filesystem.md` | Full API and policy-input reference |
| `site-docs/how-to/fs-snapshots.md` | Recipes for every label operation across MCP/HTTP/CLI |
