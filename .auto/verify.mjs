import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { join } from "node:path";

const expected = JSON.parse(readFileSync(new URL("./expected.json", import.meta.url), "utf8"));
const backends = ["dir", "s3"];
const passes = [1, 2];
const keys = datasets => datasets.flatMap(dataset =>
  backends.flatMap(backend => passes.map(pass => `${dataset}/${backend}/${pass}`)),
);
const totals = new Map();
for (let slice = 0; slice < 24; slice++) {
  const file = join(process.argv[2], `fs-grep-${slice}.jsonl`);
  const rows = readFileSync(file, "utf8").trim().split("\n").map(JSON.parse);
  const remaining = new Set(keys(slice === 0 ? Object.keys(expected) : ["wikipedia"]));
  for (const row of rows) {
    const key = `${row.dataset}/${row.backend}/${row.pass}`;
    assert(remaining.delete(key), `Unexpected result ${key} in slice ${slice}`);
    assert(row.wall_ms >= row.grep_ms && row.grep_ms > 0, `Invalid timing for ${key}`);
    const total = totals.get(key) ?? {};
    for (const field of [...Object.keys(expected[row.dataset]), "wall_ms", "grep_ms"]) {
      assert(Number.isSafeInteger(row[field]) && row[field] >= 0, `Invalid ${field} for ${key}`);
      total[field] = (total[field] ?? 0) + row[field];
    }
    totals.set(key, total);
  }
  assert.equal(remaining.size, 0, `Incomplete slice ${slice}`);
}

const localWall = { 1: 0, 2: 0 };
const s3Wall = { 1: 0, 2: 0 };
const scan = { dir: 0, s3: 0 };
for (const [key, total] of totals) {
  const [dataset, backend, pass] = key.split("/");
  for (const [field, value] of Object.entries(expected[dataset])) {
    assert.equal(total[field], value, `Full-corpus check failed for ${key} ${field}`);
  }
  const wall = backend === "dir" ? localWall : s3Wall;
  wall[pass] += total.wall_ms;
  scan[backend] += total.grep_ms;
}

console.log(`METRIC s3_cold_wall_ms=${s3Wall[1]}`);
console.log(`METRIC s3_warm_wall_ms=${s3Wall[2]}`);
console.log(`METRIC local_wall_ms=${localWall[1] + localWall[2]}`);
console.log(`METRIC local_grep_ms=${scan.dir}`);
console.log(`METRIC s3_grep_ms=${scan.s3}`);
