#!/usr/bin/env python3
"""Shared helpers for reproducible Node compatibility corpus tooling."""

from __future__ import annotations

import hashlib
import json
import pathlib
import re
import shutil
import tarfile
import tempfile
import urllib.request

_EXTERNAL_FIELDS = {"repository", "commit", "archive_url", "sha256"}
_SHA256_RE = re.compile(r"^[0-9a-f]{64}$")


def validate_source(name: str, source: dict[str, str]) -> None:
    missing = sorted(_EXTERNAL_FIELDS - source.keys())
    if missing:
        raise ValueError(f"{name} missing required fields: {', '.join(missing)}")
    digest = source["sha256"]
    if not _SHA256_RE.fullmatch(digest):
        raise ValueError(f"{name} sha256 must be 64 lowercase hexadecimal characters")


def load_versions(path: pathlib.Path) -> dict[str, dict[str, str]]:
    versions = json.loads(path.read_text())
    if not isinstance(versions, dict):
        raise ValueError("versions metadata must be a JSON object")
    for name, source in versions.items():
        if not isinstance(source, dict):
            raise ValueError(f"{name} metadata must be a JSON object")
        if name == "node":
            missing = sorted({"repository", "tag", "vendored_by"} - source.keys())
            if missing:
                raise ValueError(f"{name} missing required fields: {', '.join(missing)}")
        else:
            validate_source(name, source)
    return versions


def sha256_file(path: pathlib.Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def safe_extract_tar(archive: pathlib.Path, destination: pathlib.Path) -> None:
    destination.mkdir(parents=True, exist_ok=True)
    destination_root = destination.resolve()
    with tarfile.open(archive, "r:gz") as tar:
        members = tar.getmembers()
        for member in members:
            member_path = pathlib.PurePosixPath(member.name)
            if member_path.is_absolute() or ".." in member_path.parts:
                raise ValueError(f"unsafe archive member: {member.name}")
            if member.issym() or member.islnk() or member.isdev():
                raise ValueError(f"unsafe archive member type: {member.name}")
            resolved = (destination / pathlib.Path(*member_path.parts)).resolve()
            if destination_root not in resolved.parents and resolved != destination_root:
                raise ValueError(f"unsafe archive member: {member.name}")
        tar.extractall(destination, members=members)


def _flatten_archive_root(extracted: pathlib.Path, destination: pathlib.Path) -> None:
    entries = list(extracted.iterdir())
    source = entries[0] if len(entries) == 1 and entries[0].is_dir() else extracted
    staging = destination.with_name(destination.name + ".ready")
    if staging.exists():
        shutil.rmtree(staging)
    if source == extracted:
        extracted.rename(staging)
    else:
        source.rename(staging)
        shutil.rmtree(extracted)
    staging.rename(destination)


def download_and_verify(
    name: str,
    source: dict[str, str],
    cache_dir: pathlib.Path,
    offline: bool = False,
) -> pathlib.Path:
    validate_source(name, source)
    cache_dir.mkdir(parents=True, exist_ok=True)
    commit = source["commit"]
    archive = cache_dir / f"{name}-{commit}.tar.gz"
    destination = cache_dir / f"{name}-{commit}"

    if destination.is_dir() and archive.is_file():
        if sha256_file(archive) == source["sha256"]:
            return destination
        shutil.rmtree(destination)

    if not archive.exists():
        if offline:
            raise FileNotFoundError(f"offline corpus archive is missing: {archive}")
        with tempfile.NamedTemporaryFile(dir=cache_dir, delete=False) as handle:
            temporary = pathlib.Path(handle.name)
            with urllib.request.urlopen(source["archive_url"]) as response:
                shutil.copyfileobj(response, handle)
        temporary.replace(archive)

    actual = sha256_file(archive)
    if actual != source["sha256"]:
        archive.unlink(missing_ok=True)
        raise ValueError(
            f"{name} checksum mismatch: expected {source['sha256']}, got {actual}"
        )

    if offline and destination.is_dir():
        return destination

    extraction = pathlib.Path(tempfile.mkdtemp(prefix=f".{name}-", dir=cache_dir))
    try:
        safe_extract_tar(archive, extraction)
        if destination.exists():
            shutil.rmtree(destination)
        _flatten_archive_root(extraction, destination)
    except Exception:
        shutil.rmtree(extraction, ignore_errors=True)
        raise
    return destination
