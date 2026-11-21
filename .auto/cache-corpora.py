#!/usr/bin/env python3
import concurrent.futures
import json
from pathlib import Path
import re
import subprocess
import sys
from tempfile import TemporaryDirectory

ROOT = Path(sys.argv[1]).resolve()
REVISION = "4c6c554cca365a075834805cc0d15d11cbdc7aaa"
MDN = "54ef250ba91a42a7755b7dbd3fd6420c128b7d53"
DATASET = "marin-community/wikipedia-markdown"
CACHE = ROOT / REVISION

def curl(*args):
    return subprocess.check_output(["curl", "-fsSL", "--connect-timeout", "20",
                                    "--max-time", "900", "--retry", "3", *args])

def download(shard):
    path = CACHE / shard["path"]
    if path.is_file() and path.stat().st_size == shard["size"]:
        return
    part = path.with_suffix(".part")
    curl("-o", str(part), f"https://huggingface.co/datasets/{DATASET}/resolve/{REVISION}/{path.name}")
    if part.stat().st_size != shard["size"]:
        raise RuntimeError(f"Incomplete download: {path.name}")
    part.replace(path)

ROOT.mkdir(parents=True, exist_ok=True)
mdn = ROOT / "mdn"
if not mdn.exists():
    with TemporaryDirectory(prefix="mdn-download-", dir=ROOT) as directory:
        checkout = Path(directory) / "checkout"
        subprocess.run(["git", "init", "--quiet", str(checkout)], check=True)
        subprocess.run(["git", "-C", str(checkout), "fetch", "--quiet", "--depth", "1",
                        "https://github.com/mdn/content.git", MDN], check=True)
        subprocess.run(["git", "-C", str(checkout), "checkout", "--quiet", "--detach", "FETCH_HEAD"],
                       check=True)
        checkout.rename(mdn)
sha = subprocess.check_output(["git", "-C", str(mdn), "rev-parse", "HEAD"], text=True).strip()
if sha != MDN:
    raise RuntimeError(f"Unexpected MDN revision: {sha}")
CACHE.mkdir(parents=True, exist_ok=True)
manifest = CACHE / "manifest.json"
if manifest.exists():
    entries = json.loads(manifest.read_text())
else:
    entries = json.loads(curl(f"https://huggingface.co/api/datasets/{DATASET}/tree/{REVISION}?limit=1000"))
shards = [entry for entry in entries if re.fullmatch(r"enwiki_namespace_0_\d+\.jsonl\.gz", entry["path"])]
if len(shards) != 385:
    raise RuntimeError(f"Expected 385 shards, found {len(shards)}")
with concurrent.futures.ThreadPoolExecutor(max_workers=8) as pool:
    for index, _ in enumerate(pool.map(download, shards), 1):
        if index % 25 == 0 or index == len(shards):
            print(f"Cached {index}/{len(shards)} Wikipedia shards", file=sys.stderr, flush=True)
manifest.write_text(json.dumps(entries))
print(ROOT)
