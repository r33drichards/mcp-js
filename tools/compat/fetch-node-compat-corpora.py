#!/usr/bin/env python3
"""Download and verify pinned Node compatibility corpora."""

from __future__ import annotations

import argparse
import pathlib

from node_compat_common import download_and_verify, load_versions

REPO = pathlib.Path(__file__).resolve().parents[2]
VERSIONS = REPO / "server/tests/node_compat/versions.json"


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--source",
        choices=("deno_node_test", "citgm", "all"),
        default="all",
    )
    parser.add_argument(
        "--cache-dir",
        type=pathlib.Path,
        default=REPO / ".cache/node-compat",
    )
    parser.add_argument("--offline", action="store_true")
    args = parser.parse_args()

    versions = load_versions(VERSIONS)
    names = ("deno_node_test", "citgm") if args.source == "all" else (args.source,)
    for name in names:
        path = download_and_verify(
            name,
            versions[name],
            args.cache_dir,
            offline=args.offline,
        )
        print(path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
