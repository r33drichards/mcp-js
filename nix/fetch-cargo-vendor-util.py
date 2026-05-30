import functools
import hashlib
import json
import multiprocessing as mp
import re
import shutil
import subprocess
import sys
import tomllib
from os.path import islink, realpath
from pathlib import Path
from typing import Any, TypedDict, cast
from urllib.parse import unquote

import requests
import tomli_w
from requests.adapters import HTTPAdapter, Retry

eprint = functools.partial(print, file=sys.stderr)


def load_toml(path: Path) -> dict[str, Any]:
    with open(path, "rb") as f:
        return tomllib.load(f)


def get_lockfile_version(cargo_lock_toml: dict[str, Any]) -> int:
    version = cargo_lock_toml.get("version", 2)
    return version


def create_http_session() -> requests.Session:
    retries = Retry(total=5, backoff_factor=0.5, status_forcelist=[500, 502, 503, 504])
    session = requests.Session()
    session.mount("http://", HTTPAdapter(max_retries=retries))
    session.mount("https://", HTTPAdapter(max_retries=retries))
    return session


def download_file_with_checksum(session: requests.Session, url: str, destination_path: Path) -> str:
    sha256_hash = hashlib.sha256()
    with session.get(url, stream=True) as response:
        if not response.ok:
            raise Exception(f"Failed to fetch file from {url}. Status code: {response.status_code}")
        with open(destination_path, "wb") as file:
            for chunk in response.iter_content(1024):
                if chunk:
                    file.write(chunk)
                    sha256_hash.update(chunk)

    return sha256_hash.hexdigest()


def get_download_url_for_tarball(pkg: dict[str, Any]) -> str:
    if pkg["source"] != "registry+https://github.com/rust-lang/crates.io-index":
        raise Exception("Only the default crates.io registry is supported.")

    name = pkg["name"]
    version = pkg["version"]
    return f"https://static.crates.io/crates/{name}/{name}-{version}.crate"


def download_tarball(session: requests.Session, pkg: dict[str, Any], out_dir: Path) -> None:
    url = get_download_url_for_tarball(pkg)
    filename = f'{pkg["name"]}-{pkg["version"]}.tar.gz'
    expected_checksum = pkg["checksum"]

    tarball_out_dir = out_dir / "tarballs" / filename
    eprint(f"Fetching {url} -> tarballs/{filename}")

    calculated_checksum = download_file_with_checksum(session, url, tarball_out_dir)

    if calculated_checksum != expected_checksum:
        raise Exception(
            f"Hash mismatch! File fetched from {url} had checksum {calculated_checksum}, expected {expected_checksum}."
        )


def download_git_tree(url: str, git_sha_rev: str, out_dir: Path) -> None:
    tree_out_dir = out_dir / "git" / git_sha_rev
    eprint(f"Fetching {url}#{git_sha_rev} -> git/{git_sha_rev}")

    cmd = [
        "nix-prefetch-git",
        "--builder",
        "--quiet",
        "--fetch-submodules",
        "--url",
        url,
        "--rev",
        git_sha_rev,
        "--out",
        str(tree_out_dir),
    ]
    subprocess.check_output(cmd)


GIT_SOURCE_REGEX = re.compile("git\\+(?P<url>[^?]+)(\\?(?P<type>rev|tag|branch)=(?P<value>.*))?#(?P<git_sha_rev>.*)")


class GitSourceInfo(TypedDict):
    url: str
    type: str | None
    value: str | None
    git_sha_rev: str


def parse_git_source(source: str, lockfile_version: int) -> GitSourceInfo:
    match = GIT_SOURCE_REGEX.match(source)
    if match is None:
        raise Exception(f"Unable to process git source: {source}.")

    source_info = cast(GitSourceInfo, match.groupdict(default=None))

    if lockfile_version >= 4 and source_info["value"] is not None:
        source_info["value"] = unquote(source_info["value"])

    return source_info


def create_vendor_staging(lockfile_path: Path, out_dir: Path) -> None:
    cargo_lock_toml = load_toml(lockfile_path)
    lockfile_version = get_lockfile_version(cargo_lock_toml)

    git_packages: list[dict[str, Any]] = []
    registry_packages: list[dict[str, Any]] = []

    for pkg in cargo_lock_toml["package"]:
        if "source" not in pkg.keys():
            eprint(f'Skipping local dependency: {pkg["name"]}')
            continue
        source = pkg["source"]

        if source.startswith("git+"):
            git_packages.append(pkg)
        elif source.startswith("registry+"):
            registry_packages.append(pkg)
        else:
            raise Exception(f"Can't process source: {source}.")

    git_sha_rev_to_url: dict[str, str] = {}
    for pkg in git_packages:
        source_info = parse_git_source(pkg["source"], lockfile_version)
        git_sha_rev_to_url[source_info["git_sha_rev"]] = source_info["url"]

    out_dir.mkdir(exist_ok=True)
    shutil.copy(lockfile_path, out_dir / "Cargo.lock")

    if len(git_packages) != 0:
        (out_dir / "git").mkdir()
        for git_sha_rev, url in git_sha_rev_to_url.items():
            download_git_tree(url, git_sha_rev, out_dir)

    with mp.Pool(min(5, mp.cpu_count())) as pool:
        if len(registry_packages) != 0:
            (out_dir / "tarballs").mkdir()
            session = create_http_session()
            tarball_args_gen = ((session, pkg, out_dir) for pkg in registry_packages)
            pool.starmap(download_tarball, tarball_args_gen)


def get_manifest_metadata(manifest_path: Path) -> dict[str, Any]:
    cmd = ["cargo", "metadata", "--format-version", "1", "--no-deps", "--manifest-path", str(manifest_path)]
    output = subprocess.check_output(cmd)
    return json.loads(output)


def try_get_crate_manifest_path_from_manifest_path(manifest_path: Path, crate_name: str) -> Path | None:
    try:
        metadata = get_manifest_metadata(manifest_path)
    except subprocess.CalledProcessError:
        eprint(f"Warning: cargo metadata failed for {manifest_path}, skipping")
        return None

    for pkg in metadata["packages"]:
        if pkg["name"] == crate_name:
            return Path(pkg["manifest_path"])

    return None


def find_crate_manifest_in_tree(tree: Path, crate_name: str) -> Path:
    manifest_paths = sorted(tree.glob("**/Cargo.toml"), key=lambda path: (len(path.parts), str(path)))

    for manifest_path in manifest_paths:
        res = try_get_crate_manifest_path_from_manifest_path(manifest_path, crate_name)
        if res is not None:
            return res

    raise Exception(f"Couldn't find manifest for crate {crate_name} inside {tree}.")


def copy_and_patch_git_crate_subtree(git_tree: Path, crate_name: str, crate_out_dir: Path) -> None:
    def ignore_func(dir_str: str, path_strs: list[str]) -> list[str]:
        ignorelist: list[str] = []

        dir = Path(realpath(dir_str, strict=True))

        for path_str in path_strs:
            path = dir / path_str
            if not islink(path):
                continue

            try:
                target_path = Path(realpath(path, strict=True))
            except OSError:
                ignorelist.append(path_str)
                eprint(f"Failed to resolve symlink, ignoring: {path}")
                continue

            if not target_path.is_relative_to(git_tree):
                ignorelist.append(path_str)
                eprint(f"Symlink points outside of the crate's base git tree, ignoring: {path} -> {target_path}")
                continue

        return ignorelist

    crate_manifest_path = find_crate_manifest_in_tree(git_tree, crate_name)
    crate_tree = crate_manifest_path.parent

    eprint(f"Copying to {crate_out_dir}")
    shutil.copytree(crate_tree, crate_out_dir, ignore=ignore_func, dirs_exist_ok=False)

    cargo_toml = load_toml(crate_out_dir / "Cargo.toml")

    if "package" in cargo_toml and "version" in cargo_toml["package"]:
        cargo_toml["package"]["version"] = "0.0.1"

    with open(crate_out_dir / "Cargo.toml", "wb") as f:
        tomli_w.dump(cargo_toml, f)


def unpack(out_dir: Path) -> None:
    lockfile_path = out_dir / "Cargo.lock"
    cargo_lock_toml = load_toml(lockfile_path)
    lockfile_version = get_lockfile_version(cargo_lock_toml)

    vendor_dir = out_dir / "vendor"
    vendor_dir.mkdir(exist_ok=True)

    for pkg in cargo_lock_toml["package"]:
        if "source" not in pkg.keys():
            eprint(f'Skipping local dependency: {pkg["name"]}')
            continue

        crate_out_dir = vendor_dir / f'{pkg["name"]}-{pkg["version"]}'
        source = pkg["source"]

        if source.startswith("registry+"):
            filename = f'{pkg["name"]}-{pkg["version"]}.tar.gz'
            tarball = out_dir / "tarballs" / filename
            eprint(f"Unpacking tarball {tarball}")
            shutil.unpack_archive(tarball, vendor_dir, "gztar")
        elif source.startswith("git+"):
            source_info = parse_git_source(source, lockfile_version)
            git_tree = out_dir / "git" / source_info["git_sha_rev"]
            copy_and_patch_git_crate_subtree(git_tree, pkg["name"], crate_out_dir)
        else:
            raise Exception(f"Can't process source: {source}.")


def main() -> None:
    subcommand_to_func = {
        "create-vendor-staging": lambda: create_vendor_staging(lockfile_path=Path(sys.argv[2]), out_dir=Path(sys.argv[3])),
        "unpack": lambda: unpack(out_dir=Path(sys.argv[2])),
    }
    subcommand = sys.argv[1]
    subcommand_func = subcommand_to_func.get(subcommand)
    if subcommand_func is None:
        raise Exception(f"Unknown subcommand: {subcommand}")
    subcommand_func()


if __name__ == "__main__":
    main()
