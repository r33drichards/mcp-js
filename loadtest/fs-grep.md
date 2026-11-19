# Filesystem grep benchmark

Run from the repository root with Node.js 22+, Git, curl, a built server, and
an existing S3 bucket dedicated to benchmarking. The server uses the AWS SDK's
normal credential chain. For S3-compatible storage, set `AWS_ENDPOINT_URL`
and `AWS_S3_FORCE_PATH_STYLE=1` along with the endpoint's credentials and region.

```sh
cargo build --release -p server
node loadtest/fs-grep.mjs --s3-bucket YOUR_BENCHMARK_BUCKET
```

For a smaller run against the same upstream sources:

```sh
node loadtest/fs-grep.mjs --s3-bucket YOUR_BENCHMARK_BUCKET \
  --mdn-files 100 --wiki-shards 1 --wiki-articles 1000
```

Use `--binary` to select a different server binary. The harness downloads the
MDN content repository and every Wikipedia shard by default. It records both
source revisions on stderr. It reuses the MDN checkout and revision-specific
Wikipedia downloads in `.fs-grep-data`. Use `--data-dir` to put this cache on
another volume. Set `TMPDIR` to choose where the generated files, local snapshot
store and S3 write-through cache are staged. The full Wikipedia download is
about 12 GB compressed, and the uncompressed snapshots require significantly
more storage in both the local store and S3. The harness removes its temporary
local files after a run. It leaves snapshot objects in the S3 bucket, where they
can incur request and storage charges.

The workload preserves the MDN Markdown directory structure. It extracts the
Wikipedia records' Markdown text into roughly 1 MiB files grouped by shard. It
stages the same files in both filesystem configurations in snapshots of up to
512 files or about 32 MiB so each execution operates on a bounded working set.
Each pass searches every snapshot in the dataset. Guest JavaScript recursively
traverses the mounted filesystem and counts matching lines for the literal
strings `computer science` and `javascript` independently, ignoring case. A
line can count toward both terms.

The harness emits eight JSON lines to stdout, one per dataset, backend and pass.
The two timing fields are in milliseconds.

- `wall_ms` sums request-to-completion time across a pass, including filesystem
  mount and staging, execution, server completion handling and REST polling.
  The harness polls at 10 ms intervals.
- `grep_ms` sums time inside JavaScript for recursive listing, metadata lookup,
  file reads and matching. The S3 scan uses the server's write-through cache.
  That cache starts empty before the first S3 scan and stays in use for
  subsequent passes. A corpus larger than the cache may still require remote
  reads in later passes. The benchmark does not flush the host OS page cache.

Download, corpus ingestion and log retrieval are outside both timings. The
harness checks file and byte totals against the staged input and compares match
counts across both runs and backends. An emulator can verify those checks.
Use your intended S3 endpoint for representative storage timings.

## Verify with a local S3 emulator

Start Moto in a separate terminal:

```sh
uvx --from 'moto[server]' moto_server -H 127.0.0.1 -p 5019
```

Then create a bucket in that emulator and run the sample:

```sh
export AWS_ENDPOINT_URL=http://127.0.0.1:5019 AWS_S3_FORCE_PATH_STYLE=1
export AWS_REGION=us-east-1 AWS_ACCESS_KEY_ID=testing AWS_SECRET_ACCESS_KEY=testing
curl -fsS -X PUT "$AWS_ENDPOINT_URL/fs-grep-benchmark"
node loadtest/fs-grep.mjs --s3-bucket fs-grep-benchmark \
  --mdn-files 100 --wiki-shards 1 --wiki-articles 1000
```

## Run from a pull request

Add the `fs-benchmark` label to a pull request whose branch is in this repository.
The workflow builds the PR's server and searches all MDN Markdown files and all
Wikipedia shards. It distributes Wikipedia across 24 GitHub-hosted runners and
uses a local Moto S3 endpoint on each runner. The PR comment combines the slice
timings and match counts. Wikipedia timings are sums of the individual slice
timings. Remove and reapply the label to run it again against the latest commit.
