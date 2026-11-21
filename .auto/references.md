# Filesystem research references

Check out the pinned source revisions under `.auto/references/` if you
want local copies. The paper links point to publisher copies. Use the implementations and
papers to design experiments against mcp-js and preserve its filesystem
semantics.

## Source code

- **SeaweedFS**, commit `7b4bdb77bbcf03a56d1f885d3fd5e8aa68265e47`.
  Read `weed/filer/reader_at.go`, `weed/filer/filechunks_read.go`,
  `weed/server/filer_server_handlers_read_dir.go`, and
  `weed/storage/volume_read.go`. The chunk reader implements bounded
  prefetch. Compare the idea with a fixed-size pool for reading Markdown
  files and measure where additional concurrency stops helping.
  [Source](https://github.com/seaweedfs/seaweedfs/tree/7b4bdb77bbcf03a56d1f885d3fd5e8aa68265e47)
- **JuiceFS**, commit `33592a3016c1c39dfa71b92c87e8a046ca930a92`.
  Read `pkg/vfs/reader.go`, `pkg/chunk/prefetch.go`, and
  `pkg/chunk/cached_store.go`. The reader adapts its read-ahead window to
  sequential progress and memory pressure, and the chunk store coalesces
  duplicate work. Compare small fixed windows first and reserve adaptive
  sizing for a measured queueing or memory bottleneck.
  [Source](https://github.com/juicedata/juicefs/tree/33592a3016c1c39dfa71b92c87e8a046ca930a92)

## Papers and hypotheses

- **Finding a needle in Haystack**, Beaver et al., OSDI 2010.
  Haystack packs objects and keeps compact lookup metadata in memory to
  avoid per-object metadata I/O. In mcp-js, count list/stat/read operations
  before trying to reduce repeated lookups. A future runtime experiment can
  compare typed directory entries with per-file `lstat`. [Paper](https://www.usenix.org/legacy/events/osdi10/tech/full_papers/Beaver.pdf)
- **TABLE FS**, Ren and Gibson, USENIX ATC 2013. The paper studies
  metadata-heavy workloads and aggregation of directory metadata and tiny
  files. Compare the small-file-heavy MDN subset with the larger
  Wikipedia files before pursuing a metadata optimization. [Paper](https://www.usenix.org/system/files/conference/atc13/atc13-ren.pdf)
- **The Tail at Scale**, Dean and Barroso, CACM 2013. Fan-out increases
  sensitivity to slow operations. Test bounded concurrency and compare
  repeated cold and warm S3 timings. Avoid interpreting a noisy cold-pass
  result as a consistent improvement.
  [Paper](https://research.google/pubs/the-tail-at-scale/)
- **f4**, Muralidhar et al., OSDI 2014. Its warm BLOB storage uses a
  workload-specific layout and distinguishes common reads from recovery.
  Use it to frame a follow-up storage experiment around object packing and
  request count. Preserve deduplication and random access when evaluating
  any format change.
  [Paper](https://www.usenix.org/system/files/conference/osdi14/osdi14-paper-muralidhar.pdf)

For a future runtime experiment, inspect `FsStore::prefetch` in
`server/src/engine/fs_store.rs` and the write-through cache in
`server/src/engine/heap_storage.rs`. mcp-js stages blobs before the isolate
runs. The JS scan and S3 request/staging timers measure different costs.
