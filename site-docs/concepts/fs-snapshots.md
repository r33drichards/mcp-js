# Filesystem snapshots

mcp-v8 can give a `run_js` execution a snapshottable, content-addressed filesystem alongside the V8 heap. A snapshot is an immutable directory tree identified by a hash; a label is a movable name that points at one. This page explains the two-plane model the feature is built on, why the mount is an ephemeral overlay, how pushes, the reflog, and merges work, and what changes in a cluster.

## Two planes: content and pointers

The whole design separates **what the bytes are** from **which snapshot is current**. Those are two independent planes with very different consistency needs.

| Plane | Holds | Keyed by | Mutability | Coordination |
|---|---|---|---|---|
| Content plane | File chunks + directory manifests | blake3 hash of the content | Immutable | None — write from any node |
| Pointer plane | Labels + per-label reflog | a human-readable label name | Mutable | The only coordinated write |

The content plane is purely content-addressed. A file's bytes are chunked, each chunk is hashed by its plaintext (so identical chunks dedup), and a directory is a **manifest**: a flat `BTreeMap` from path to entry, serialised deterministically and keyed by the blake3 hash of that encoding. A manifest carries no parent pointer and no lineage — it is *pure content*, so two identical trees always produce the same id, and identical subtrees across snapshots share storage. Because a content-addressed id always names the same bytes, the content plane needs no coordination at all: any node can write a chunk or manifest and the result is idempotent.

Lineage — "this snapshot came after that one" — lives entirely in the pointer plane. A **label** maps a name like `main` to the CA id of its current manifest, and every label carries an append-only **reflog** recording each move. This is the only place the system performs a coordinated, atomic write.

## How files are stored

Each file in a manifest is one `Entry` (mode, size, content, optional symlink). The content is stored one of two ways depending on size:

- **Small files** (at or below 64 KiB) are *inlined* — their bytes live directly in the manifest entry, with no separate blob.
- **Large files** are split into content-defined chunks with FastCDC, each chunk hashed by its plaintext bytes and stored (optionally zstd-compressed) as a blob keyed by that hash. The entry holds the ordered list of chunk hashes.

Content-defined chunking means an edit in the middle of a large file re-chunks only the affected region, so successive snapshots of a slowly-changing file share most of their chunks. Dedup happens at three levels: identical chunks, identical files, and — because manifests are pure content — identical whole subtrees.

The blob backend is the **same `HeapStorage`** the heap store already uses; fs objects are simply namespaced with `fschunk:` and `fsmanifest:` key prefixes so they never collide with heap snapshots in a shared backend.

## Independent of the heap

The filesystem snapshot and the V8 heap snapshot are orthogonal. A `run_js` call can carry a `heap` handle, an `fs` handle, both, or neither, and each is restored, captured, and reported independently. A completed execution reports its resulting filesystem snapshot as the `fs` field of its execution status, exactly as it reports `heap`. See [Stateful sessions & heap snapshots](sessions-and-heaps.md) for the heap side.

## The userspace overlay mount

When an execution mounts a snapshot it does **not** materialise files on a real disk and it does not use kernel overlayfs or FUSE. The mount is an in-process overlay with two layers:

- a **read-only base** — the manifest pinned at mount time;
- a **mutable upper** — a map of pending writes and whiteouts.

Reads resolve the upper first (a `Data` entry shadows the base, a `Whiteout` means "deleted") and fall through to the base otherwise. A write does a copy-up: it stores the new bytes in the content plane and records a `Data` entry in the upper. A delete records a `Whiteout`. Directories are **implicit**: a path is a directory if anything lives under it, so `mkdir` is a no-op that merely lets a caller mkdir-then-write. Recursive directory `rename` is supported — it rewrites the path prefix of every descendant. A non-recursive removal of a non-empty directory fails, mirroring Node's `fs.rm`.

Two properties follow from this design:

- **Pinning isolates a running session.** `pull` resolves a label to a CA id once, at mount time, and pins that manifest as the base. Concurrent label moves made by other sessions do not shift the filesystem under a running execution.
- **The mount is ephemeral; the manifest is durable.** Nothing about the upper layer is persistent. When the execution finishes, `push` flushes the upper over the base into a **new pure manifest** — applying each `Data` and removing each `Whiteout` — and returns its CA id. That CA id is the durable artifact (reported as the execution's `fs` field). The mount itself is then discarded.

Flushing the overlay into a manifest is deliberately *not* the same as advancing a label. A finished execution produces a snapshot id; making that snapshot the new head of a label is a separate, explicit step (see below).

## Advancing a label: the push conflict model

Turning a freshly produced snapshot into the new head of a label is a coordinated pointer move with a deliberately safe default. Three behaviours are available, and only the safe one is the default:

- **Reject-and-rebase (default).** The push carries the head the caller started from (`expected`) and succeeds only if the label still points there — an atomic compare-and-set. If the label moved in the meantime the push is *rejected* (surfaced as HTTP `409`) and the caller is expected to re-pull, re-apply or merge, and try again. This is the same optimistic-concurrency discipline as `git push --ff-only`.
- **Force.** An explicit opt-in that moves the label unconditionally, skipping the compare. It still records a reflog entry.
- **Detach.** An explicit opt-in that produces/echoes a CA id without touching any label at all — useful for fan-out experiments where you want the snapshot id but no shared pointer move.

Making reject-and-rebase the default means two agents racing to advance the same label cannot silently clobber each other; one wins the CAS and the other is told to rebase.

## The reflog, reset, and garbage collection

Every label move appends a `RefLogEntry` to that label's append-only reflog: when it happened (`at`), the previous head (`from`), the new head (`to`), the operation (`create` / `push` / `reset` / `force`), and an optional human `message` (a commit-style note, capped at 4096 bytes). The reflog is the recorded history of a label and the basis for rollback.

**Reset** is the rollback verb: it moves a label back to an earlier CA id. By default the target must already appear in the label's reflog, so a reset stays within recorded history; an explicit `allow_unlogged` escape hatch lifts that check. Reset records its own reflog entry, so rolling back is itself part of the history and can be rolled forward.

This interacts directly with garbage collection. GC is a **reflog-rooted mark-and-sweep**: the roots are every label's current head *plus* every CA id still named by a `to` in any retained reflog entry. From each root manifest, GC marks the manifest blob and all of its chunk blobs; any fs blob (and only fs-prefixed blobs, so a shared backend's heap snapshots are never touched) that ends up unmarked is swept. The consequence is the safety property that makes reset trustworthy: **a snapshot you reset past stays reachable through its reflog entry**, so you can always roll forward to it — right up until that reflog entry ages out of the retention window. Only then does the orphaned snapshot become collectable.

## Merging snapshots

Two snapshots that diverged from a common point can be merged into a new one. Because a manifest is a *flat* map of paths (there is no per-directory hash), the merge mirrors Mercurial's `manifestmerge` — diff the three flat path maps and decide each path independently — rather than git's recursive tree walk.

The merge is **three-way** when given a `base` (the common ancestor both sides were mounted from): a path conflicts only if *both* sides changed it away from the base and away from each other. Omitting the base gives a **two-way** merge, where any path both sides changed (and that now differs) conflicts. Per path, the cheap short-circuit is content equality: if both sides are identical, or one side never moved from the base, there is nothing to reconcile.

Before reporting a divergent path as a conflict, a **type-aware content merge** runs. Text files are sniffed and merged at line level via diff3, so edits to different lines of the same file reconcile silently. Only when the content merge cannot resolve a path is it surfaced as a structured conflict carrying, per path: a `kind`, each side's content id (`null` meaning the file is absent on that side), and — for text — the diff3 conflict `markers` plus unified `diff_ours` / `diff_theirs`. A `prefer=ours|theirs` setting auto-resolves any remaining conflict to that side. A clean merge writes a normal pure manifest and returns its CA id; like a push of a finished snapshot, the merge does **not** move any label on its own.

## In a cluster

The two-plane split is what makes clustering tractable. The planes are replicated very differently:

- **The pointer plane is the only replicated state.** Label moves route through the Raft leader so a compare-and-set is linearizable across the whole cluster. Crucially, a label move replicates the head pointer **and** its reflog entry as a *single atomic replicated entry* — the head can never advance without the reflog recording it. That guarantee is not just asserted; it is checked with a TLA+ model in [`specs/FsLabelAtomicWrite.tla`](https://github.com/r33drichards/mcp-js/blob/main/specs/FsLabelAtomicWrite.tla): the model proves that the naive design, writing the head and the reflog as two separate replicated entries, violates the invariant that a settled head is always covered by the reflog, while the combined single-entry design holds. Since reset and the reflog-rooted GC both depend on the reflog covering every head a label has ever pointed at, a head that moved without a recorded reflog entry would be silent corruption.
- **The content plane is node-local unless shared storage is configured.** Blobs and manifests are written wherever that node's blob backend points. With node-local file storage, a manifest written on one node does not exist on another — so a label advanced on node A would resolve on node B to a tree node B is missing. For that reason node-local blobs are **single-node only**, and the server **refuses to enable fs snapshots in cluster mode without shared storage** (`--s3-bucket`, optionally fronted by a write-through cache). With shared storage the content plane resolves identically on every node, exactly as content-addressing intends.

## Out of scope / non-goals

The feature is deliberately scoped to a content-addressed overlay with labels and a three-way merge. The following are explicitly *not* implemented:

- **Materialised real-path mode** (backing the mount with actual files on disk) and **reflink copy-up**. The mount is the in-process virtual overlay only.
- **Rename/move detection in merge**, and a **recursive virtual base** for resolving criss-cross histories. The merge is flat and per-path; it does not track that a file moved.
- **Rich binary or SQLite merge drivers.** The merge reports a `kind` for these, but only the text line-level driver actually reconciles content; binary/SQLite drivers are stubbed, not implemented.
- **CRDT semantics.** Concurrency is handled by reject-and-rebase plus explicit merge, not by automatic conflict-free convergence.
- **A cross-label sharing UI.** Sharing a snapshot between labels is done by pushing the same CA id; there is no higher-level UI for it.

## See also

- [How-to: Filesystem snapshots](../how-to/fs-snapshots.md)
- [Concepts: Stateful sessions & heap snapshots](sessions-and-heaps.md)
- [Concepts: Heap storage backends](storage-backends.md)
- [Concepts: Clustering & replication (Raft)](clustering.md)
- [Concepts: Policies](policies.md)
