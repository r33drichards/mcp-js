# Formal specs

TLA+ models backing correctness-sensitive parts of the server.

## FsLabelAtomicWrite

Models a cluster-mode label move (`server/src/engine/fs_labels.rs`), which must
move the label head pointer **and** append a reflog entry recording that move.
`fs_reset` and the reflog-rooted GC both depend on the reflog covering every
head the label has ever pointed at, so a head that moves without a reflog entry
is silent corruption.

The `Mode` constant selects the implementation:

- `split` — the pre-fix code: the head and the reflog are two separate
  replicated writes. The model proves this is unsafe.
- `combined` — the fix: the head write and the reflog write ride in a single
  replicated log entry and are applied as one atomic batch. The model proves
  this is safe.

### Invariant

```
StableReflogCoversHead == (phase = "idle") => ((head = 0) \/ (head \in reflog))
```

In any settled state, a non-initial head is recorded in the reflog. (The
stronger `AlwaysReflogCoversHead` drops the `phase = "idle"` guard and holds for
`combined` too, showing the head is never even transiently observable ahead of
the reflog.)

### Running

Grab the TLA+ tools jar once (gitignored):

```sh
curl -fsSL -o tla2tools.jar \
  https://github.com/tlaplus/tlaplus/releases/download/v1.8.0/tla2tools.jar
```

Then model-check each config (`-deadlock` is passed only because the model
terminates after `MaxMoves` rather than stuttering):

```sh
# safe — "Model checking completed. No error has been found."
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config FsLabelAtomicWrite_combined.cfg FsLabelAtomicWrite.tla

# unsafe — "Invariant StableReflogCoversHead is violated" + counterexample
java -cp tla2tools.jar tlc2.TLC -deadlock \
  -config FsLabelAtomicWrite_split.cfg FsLabelAtomicWrite.tla
```

### Result

- `combined`: all reachable states satisfy the invariants — **no error**.
- `split`: violated. Minimal counterexample:

  | step | action | head | reflog | phase |
  |------|--------|------|--------|-------|
  | 1 | Init | 0 | {} | idle |
  | 2 | StartMove | 1 | {} | reflogPending |
  | 3 | FailReflog | 1 | {} | idle |

  After step 3 the label head is committed at `1` but the reflog is empty, so
  `fs_reset`/GC cannot see how the label reached its current head.

The fix in `cluster.rs` (`cas_with` / `put_with` carrying `extra` writes applied
via a single `apply_batch`) and `fs_labels.rs` (using them for `create`/`cas`/
`force` in cluster mode) implements the `combined` design.
