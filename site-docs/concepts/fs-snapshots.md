# Filesystem snapshots

The snapshottable filesystem gives a session a **content-addressed, versioned
virtual filesystem** that is mounted by a human-readable label. Every `push`
and `pull` is an immutable content-addressed (CA) snapshot, and a label can be
`reset` to any earlier snapshot via its reflog.

It is enabled with `--enable-fs-snapshots` and is **independent of the heap**:
an execution can mount an fs snapshot, persist a new one, and move its heap
snapshot entirely separately.

## Two planes

The design separates immutable content from a mutable pointer:

- **Content plane (immutable).** *Blobs* (file content — small files inlined,
  large files split into [FastCDC](https://en.wikipedia.org/wiki/Rolling_hash)
  content-defined chunks with per-chunk `zstd`) and *manifests* (pure-content
  directory trees), each keyed by its `blake3` hash and stored in the same blob
  backend the heap store uses. Identical trees produce identical manifest ids
  (tree-level dedup). A manifest never embeds lineage — it is a pure function of
  the file contents.
- **Pointer plane (mutable, coordinated).** *Labels* map a name to a current
  manifest CA id, with a per-label append-only **reflog** enabling rollback.
  Labels are the only coordinated write; in a cluster they route through the
  Raft leader so compare-and-set is linearizable.

## The mount

Mounting is a userspace overlay implemented entirely in-process: a read-only
**base** manifest pinned at `pull` time plus a mutable **upper** write layer
(copy-up on write, whiteouts on delete). There is no kernel overlayfs or FUSE.
Because `pull` pins the base CA id, concurrent label moves never shift a running
session. The mount is ephemeral — the durable artifact is the manifest hash that
`push` returns.

## Push conflicts

The default is **reject-and-rebase**: if a label moved since the session pulled,
`push` fails and the caller must re-pull. `force` moves the pointer
unconditionally; `detach` returns the CA id without touching any label. Both are
explicit opt-ins, never defaults.

## Garbage collection

GC is mark-and-sweep. Roots are the current head of every label plus every CA id
still in every label's reflog (within the retention window). A label that was
`reset` past a snapshot keeps that snapshot reachable through its reflog, so
roll-forward still works; only once the reflog entry is pruned does the snapshot
become collectable.

## Out of scope

These are deliberate non-goals of the current (virtual) implementation:

- **Materialized mode.** Checking a manifest out to a per-session scratch
  directory so subprocesses, native code, or WASM that use real-path IO can see
  it, then pushing by diffing the directory. This is where reflink copy-up
  (APFS `clonefile`, XFS/Btrfs `FICLONE`, ReFS block clone) would belong. v1
  intercepts only the in-process Node `fs` surface.
- **Merging diverged labels.** v1 is reject-and-rebase plus `force` only.
- **Cross-label / cross-repo sharing UI.**

See the [how-to guide](../how-to/fs-snapshots.md) for the tools, HTTP endpoints,
and CLI verbs.
