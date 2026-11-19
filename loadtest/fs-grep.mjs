import assert from "node:assert/strict";
import { spawn, execFile } from "node:child_process";
import { once } from "node:events";
import { createReadStream } from "node:fs";
import { access, mkdir, mkdtemp, readdir, realpath, rename, rm, stat, writeFile } from "node:fs/promises";
import { createServer } from "node:net";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { pipeline } from "node:stream/promises";
import { setTimeout as sleep } from "node:timers/promises";
import { parseArgs, promisify } from "node:util";
import { createGunzip } from "node:zlib";

const exec = promisify(execFile);
const mdnUrl = "https://github.com/mdn/content";
const wikiUrl = "https://huggingface.co/datasets/marin-community/wikipedia-markdown";
const wikiApi = "https://huggingface.co/api/datasets/marin-community/wikipedia-markdown";
const MiB = 1024 * 1024;

const { values: options } = parseArgs({
  options: {
    binary: { type: "string", default: "target/release/server" },
    "s3-bucket": { type: "string" },
    "data-dir": { type: "string", default: ".fs-grep-data" },
    "mdn-files": { type: "string" },
    "skip-mdn": { type: "boolean" },
    "wiki-slice": { type: "string" },
    "wiki-revision": { type: "string" },
    "wiki-shards": { type: "string" },
    "wiki-articles": { type: "string" },
    help: { type: "boolean" },
  },
});

function limit(option) {
  if (options[option] === undefined) return Infinity;
  const value = Number(options[option]);
  assert(Number.isSafeInteger(value) && value > 0, `--${option} must be a positive integer`);
  return value;
}

async function download(url, file) {
  const temporary = `${file}.${process.pid}.part`;
  try {
    await access(file);
    return;
  } catch (error) {
    if (error.code !== "ENOENT") throw error;
  }
  try {
    await exec("curl", ["-fsSL", "--retry", "3", "-o", temporary, url]);
    await rename(temporary, file);
  } finally {
    await rm(temporary, { force: true });
  }
}

async function getJson(url) {
  const { stdout } = await exec("curl", ["-fsSL", "--retry", "3", url]);
  return JSON.parse(stdout);
}

async function* markdownFiles(root, prefix = "") {
  const entries = await readdir(join(root, prefix), { withFileTypes: true });
  entries.sort((a, b) => a.name.localeCompare(b.name, "en"));
  for (const entry of entries) {
    const path = join(prefix, entry.name);
    if (entry.isDirectory()) yield* markdownFiles(root, path);
    else if (entry.isFile() && path.endsWith(".md")) yield path;
  }
}

async function unpackWiki(shard, root, articleLimit) {
  let count = 0;
  let part = 0;
  let size = 0;
  let text = [];
  async function flush() {
    if (!text.length) return;
    await writeFile(join(root, `${part++}.md`), text.join("\n\n") + "\n");
    text = [];
    size = 0;
  }
  await pipeline(createReadStream(shard), createGunzip(), async (source) => {
    for await (const line of createInterface({ input: source, crlfDelay: Infinity })) {
      // Drain the stream even for a sample so gzip integrity is checked.
      if (count === articleLimit) continue;
      const article = JSON.parse(line);
      assert.equal(typeof article.text, "string", "Expected a Wikipedia Markdown text field");
      const bytes = Buffer.byteLength(article.text) + 2;
      if (size + bytes > MiB) await flush();
      text.push(article.text);
      size += bytes;
      count++;
    }
    await flush();
  });
  assert(count > 0, `No Wikipedia articles in ${shard}`);
  return count;
}

async function* partitions(root, fileLimit = Infinity) {
  let paths = [];
  let bytes = 0;
  let count = 0;
  for await (const path of markdownFiles(root)) {
    const fileBytes = (await stat(join(root, path))).size;
    if (paths.length && (paths.length === 512 || bytes + fileBytes > 32 * MiB)) {
      yield { paths, bytes };
      paths = [];
      bytes = 0;
    }
    paths.push(path);
    bytes += fileBytes;
    if (++count === fileLimit) break;
  }
  if (paths.length) yield { paths, bytes };
}

async function startServer(binary, dir, policy, backend, bucket, cache) {
  await mkdir(dir, { recursive: true });
  const listener = createServer().listen(0, "127.0.0.1");
  await once(listener, "listening");
  const { port } = listener.address();
  await new Promise((done) => listener.close(done));
  const url = `http://127.0.0.1:${port}`;
  const args = [
    "--bind-host", "127.0.0.1", "--http-port", String(port),
    "--session-db-path", dir, "--heap-store", "none", "--fs-store", backend,
    "--execution-timeout", "300", "--heap-memory-max", "128",
    "--fs-passthrough", "--policies-json", policy,
  ];
  if (backend === "s3") args.push("--s3-bucket", bucket, "--cache-dir", cache);
  const child = spawn(binary, args, { stdio: ["ignore", "ignore", "pipe"] });
  let stderr = "";
  let spawnError;
  child.stderr.setEncoding("utf8").on("data", (chunk) => { stderr = (stderr + chunk).slice(-12000); });
  child.on("error", (error) => { spawnError = error; });
  const closed = new Promise((done) => child.once("close", done));
  async function stop() {
    child.kill();
    await closed;
  }
  try {
    const deadline = performance.now() + 30000;
    while (performance.now() < deadline) {
      if (spawnError || child.exitCode !== null || child.signalCode !== null) break;
      try {
        const response = await fetch(`${url}/api/version`, { signal: AbortSignal.timeout(1000) });
        if (response.ok) {
          await response.body.cancel();
          return { url, stop };
        }
      } catch {
        // The server can still be starting.
      }
      await sleep(100);
    }
    throw new Error(`Could not start ${backend}: ${spawnError ?? stderr}`);
  } catch (error) {
    await stop();
    throw error;
  }
}

async function request(url, body, signal) {
  const response = await fetch(url, body === undefined ? { signal } : {
    method: "POST", headers: { "Content-Type": "application/json" },
    body: JSON.stringify(body), signal,
  });
  if (!response.ok) throw new Error(`${url} returned ${response.status}: ${await response.text()}`);
  return response.json();
}

async function execute(server, code, snapshot) {
  const signal = AbortSignal.timeout(600000);
  const { execution_id: id } = await request(`${server.url}/api/exec`, { code, fs: snapshot }, signal);
  const url = `${server.url}/api/executions/${id}`;
  for (;;) {
    const run = await request(url, undefined, signal);
    if (run.status === "completed") return { ...run, url };
    if (!["running", "queued", "pending"].includes(run.status)) {
      throw new Error(`Execution ${id} ${run.status}: ${run.error ?? JSON.stringify(run)}`);
    }
    await sleep(10);
  }
}

async function seed(server, source, paths, label) {
  const code = `
    for (const path of ${JSON.stringify(paths)}) {
      const dest = "/workspace/corpus/" + path;
      await fs.mkdir(dest.slice(0, dest.lastIndexOf("/")), { recursive: true });
      await fs.writeFile(dest, await fs.readFile(${JSON.stringify(source + "/")} + path));
    }`;
  const run = await execute(server, code, label);
  assert.match(run.fs ?? "", /^[a-f0-9]{64}$/, "Expected a persisted snapshot ID");
  return run.fs;
}

// Serialized into the guest so both backends run the same JavaScript.
async function grep() {
  const started = performance.now();
  const counts = { files: 0, bytes: 0, "computer science": 0, javascript: 0 };
  const directories = ["/workspace/corpus"];
  const terms = ["computer science", "javascript"];
  while (directories.length) {
    const dir = directories.pop();
    for (const name of await fs.readdir(dir)) {
      const path = dir + "/" + name;
      const info = await fs.lstat(path);
      if (info.isDirectory()) {
        directories.push(path);
      } else if (info.isFile() && name.endsWith(".md")) {
        const text = await fs.readFile(path, "utf8");
        counts.files++;
        counts.bytes += info.size;
        for (const line of text.split("\n")) {
          const lower = line.toLowerCase();
          for (const term of terms) {
            if (lower.includes(term)) counts[term]++;
          }
        }
      }
    }
  }
  console.log(JSON.stringify({ counts, grep_ms: performance.now() - started }));
}

async function scan(server, source, backend, pass) {
  let grep_ms = 0;
  let wall_ms = 0;
  const counts = { files: 0, bytes: 0, "computer science": 0, javascript: 0 };
  for (const part of source.parts) {
    const started = performance.now();
    const run = await execute(server, `await (${grep.toString()})();`, part[backend]);
    wall_ms += performance.now() - started;
    const output = await request(run.url + "/output", undefined, AbortSignal.timeout(30000));
    const result = JSON.parse(output.data);
    assert.equal(result.counts.files, part.files, "Scan file count differs from input");
    assert.equal(result.counts.bytes, part.bytes, "Scan byte count differs from input");
    grep_ms += result.grep_ms;
    for (const key of Object.keys(counts)) counts[key] += result.counts[key];
  }
  console.log(JSON.stringify({
    dataset: source.name, backend, pass, partitions: source.parts.length,
    wall_ms: Math.round(wall_ms), grep_ms: Math.round(grep_ms), ...counts,
    ...(source.articles === undefined ? {} : { articles: source.articles, shards: source.shards }),
  }));
  return counts;
}

async function main() {
  if (options.help) {
    console.log("node loadtest/fs-grep.mjs --s3-bucket BUCKET [--binary PATH] [--data-dir PATH] [--mdn-files N] [--wiki-shards N] [--wiki-articles N] [--wiki-revision SHA] [--wiki-slice INDEX/TOTAL] [--skip-mdn]");
    return;
  }
  const bucket = options["s3-bucket"];
  assert(bucket, "Pass --s3-bucket with an existing benchmark bucket");
  const mdnLimit = limit("mdn-files");
  const wikiLimit = limit("wiki-shards");
  const articleLimit = limit("wiki-articles");
  const binary = await realpath(options.binary);
  const data = resolve(options["data-dir"]);
  await mkdir(data, { recursive: true });
  let mdnRoot;
  let mdnRevision;
  if (!options["skip-mdn"]) {
    const mdn = join(data, "mdn");
    try {
      await access(join(mdn, ".git"));
    } catch (error) {
      if (error.code !== "ENOENT") throw error;
      const scratch = await mkdtemp(join(data, "mdn-clone-"));
      try {
        await exec("git", ["clone", "--depth", "1", mdnUrl, scratch]);
        await rename(scratch, mdn);
      } finally {
        await rm(scratch, { recursive: true, force: true });
      }
    }
    mdnRevision = (await exec("git", ["-C", mdn, "rev-parse", "HEAD"])).stdout.trim();
    mdnRoot = await realpath(join(mdn, "files"));
  }
  const wikiRevision = options["wiki-revision"] ?? (await getJson(wikiApi)).sha;
  assert.match(wikiRevision, /^[a-f0-9]{40}$/);
  const wikiCache = join(data, wikiRevision);
  await mkdir(wikiCache, { recursive: true });
  const tree = await getJson(`${wikiApi}/tree/${wikiRevision}?limit=1000`);
  assert(tree.length < 1000, "Wikipedia listing needs pagination");
  const slice = options["wiki-slice"] ?? "0/1";
  assert.match(slice, /^\d+\/[1-9]\d*$/, "Expected --wiki-slice INDEX/TOTAL (zero-based)");
  const [sliceIndex, slices] = slice.split("/").map(Number);
  assert(Number.isSafeInteger(slices) && sliceIndex < slices, "Invalid Wikipedia slice");
  const allShards = tree.filter(({ type, path }) => type === "file" && /^enwiki_namespace_0_\d+\.jsonl\.gz$/.test(path))
    .map(({ path }) => path).sort((a, b) => a.localeCompare(b, "en", { numeric: true })).slice(0, wikiLimit);
  const shards = allShards.filter((_, index) => index % slices === sliceIndex);
  assert(shards.length, "No Wikipedia shards found");
  console.error(JSON.stringify({ mdn_revision: mdnRevision, wikipedia_revision: wikiRevision,
    wikipedia_shards: shards.length, wikipedia_slice: slice, mdn_file_limit: mdnLimit, wikipedia_articles_per_shard: articleLimit }));

  const work = await mkdtemp(join(await realpath(tmpdir()), "mcp-fs-grep-"));
  const servers = new Map();
  try {
    const wikiRoot = join(work, "wiki");
    const rego = join(work, "fs.rego");
    await writeFile(rego, `package mcp.filesystem
default allow = false
allow if {
  startswith(input.path, "/workspace/corpus/")
  input.operation in {"mkdir", "readFile", "readdir", "lstat", "writeFile"}
}
allow if {
  input.path == "/workspace/corpus"
  input.operation in {"mkdir", "readdir"}
}
allow if {
  input.operation == "readFile"
  some root in ${JSON.stringify([mdnRoot, wikiRoot].filter(Boolean).map((root) => root + "/"))}
  startswith(input.path, root)
}
`);
    const policy = JSON.stringify({ filesystem: { policies: [{ url: `file://${rego}` }] } });
    const localDir = join(work, "local");
    const s3Dir = join(work, "s3");
    const seedCache = join(work, "seed-cache");
    servers.set("dir", await startServer(binary, localDir, policy, "dir"));
    servers.set("s3", await startServer(binary, s3Dir, policy, "s3", bucket, seedCache));
    const mdnSource = { name: "mdn", parts: [] };
    const wikiSource = { name: "wikipedia", parts: [], articles: 0, shards: shards.length };

    async function stage(source, root, maxFiles) {
      for await (const { paths, bytes } of partitions(root, maxFiles)) {
        const label = `grep-${source.name}-${source.parts.length}`;
        const part = { files: paths.length, bytes };
        for (const [backend, server] of servers) part[backend] = await seed(server, root, paths, label);
        source.parts.push(part);
      }
    }

    if (mdnRoot) {
      console.error("Staging MDN Markdown");
      await stage(mdnSource, mdnRoot, mdnLimit);
      assert(mdnSource.parts.length, "No MDN Markdown files found");
    }
    for (const [index, shard] of shards.entries()) {
      console.error(`Staging Wikipedia shard ${index + 1}/${shards.length}: ${shard}`);
      const file = join(wikiCache, shard);
      await download(`${wikiUrl}/resolve/${wikiRevision}/${shard}`, file);
      await mkdir(wikiRoot);
      wikiSource.articles += await unpackWiki(file, wikiRoot, articleLimit);
      await stage(wikiSource, wikiRoot);
      await rm(wikiRoot, { recursive: true });
    }

    await servers.get("s3").stop();
    servers.delete("s3");
    await rm(seedCache, { recursive: true, force: true });
    servers.set("s3", await startServer(binary, s3Dir, policy, "s3", bucket, join(work, "scan-cache")));
    for (const source of mdnRoot ? [mdnSource, wikiSource] : [wikiSource]) {
      let expected;
      for (const [backend, server] of servers) {
        for (const pass of [1, 2]) {
          const counts = await scan(server, source, backend, pass);
          expected ??= counts;
          assert.deepEqual(counts, expected, `Search results changed for ${source.name} ${backend} pass ${pass}`);
        }
      }
    }
  } finally {
    await Promise.all([...servers.values()].map((server) => server.stop()));
    await rm(work, { recursive: true, force: true });
  }
}

await main();
