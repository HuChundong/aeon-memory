#!/usr/bin/env python3
"""Build a verified, self-contained native Aeon Memory release archive."""

from __future__ import annotations

import argparse
import hashlib
import os
import re
import shutil
import stat
import tarfile
import tempfile
import time
import urllib.request
import zipfile
from pathlib import Path

SQLITE_VEC_VERSION = "0.1.9"
SQLITE_VEC_BASE_URL = (
    f"https://github.com/asg017/sqlite-vec/releases/download/v{SQLITE_VEC_VERSION}"
)

# SHA-256 values are from the upstream v0.1.9 checksums.txt release asset.
TARGETS = {
    "x86_64-unknown-linux-gnu": {
        "asset": "sqlite-vec-0.1.9-loadable-linux-x86_64.tar.gz",
        "sha256": "b959baa1d8dc88861b1edb337b8587178cdcb12d60b4998f9d10b6a82052d5d7",
        "vec": "vec0.so",
        "format": "tar.gz",
    },
    "aarch64-unknown-linux-gnu": {
        "asset": "sqlite-vec-0.1.9-loadable-linux-aarch64.tar.gz",
        "sha256": "ea03d39541e478fab5974253c461e1cb5d77742f69e40cf96e3fad5bc309a37c",
        "vec": "vec0.so",
        "format": "tar.gz",
    },
    "x86_64-apple-darwin": {
        "asset": "sqlite-vec-0.1.9-loadable-macos-x86_64.tar.gz",
        "sha256": "53ad76e400786515e2edcaed2f01271dda846316390b761fadbd2dcf56aa4713",
        "vec": "vec0.dylib",
        "format": "tar.gz",
    },
    "aarch64-apple-darwin": {
        "asset": "sqlite-vec-0.1.9-loadable-macos-aarch64.tar.gz",
        "sha256": "8282126333399ddfe98bbbcc7a1936e7252625aac49df056a98be602e46bfd29",
        "vec": "vec0.dylib",
        "format": "tar.gz",
    },
    "x86_64-pc-windows-msvc": {
        "asset": "sqlite-vec-0.1.9-loadable-windows-x86_64.tar.gz",
        "sha256": "51581189d52066b4dfc6631f6d7a3eab7dedc2260656ab09ca97ab3fb8165983",
        "vec": "vec0.dll",
        "format": "zip",
    },
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(chunk)
    return digest.hexdigest()


def download_verified(url: str, destination: Path, expected: str) -> None:
    if destination.exists() and sha256(destination) == expected:
        return
    destination.unlink(missing_ok=True)
    last_error = None
    for attempt in range(4):
        try:
            request = urllib.request.Request(
                url, headers={"User-Agent": "aeon-memory-release-builder/1"}
            )
            with urllib.request.urlopen(request, timeout=120) as response, destination.open(
                "wb"
            ) as out:
                shutil.copyfileobj(response, out)
            actual = sha256(destination)
            if actual != expected:
                raise RuntimeError(
                    f"sqlite-vec checksum mismatch for {url}: expected {expected}, got {actual}"
                )
            return
        except Exception as error:  # network/TLS errors vary across Python platforms
            last_error = error
            destination.unlink(missing_ok=True)
            if attempt < 3:
                time.sleep(2**attempt)
    raise RuntimeError(f"failed to download verified sqlite-vec asset {url}: {last_error}")


def extract_named_member(archive: Path, member_name: str, destination: Path) -> None:
    with tarfile.open(archive, "r:gz") as source:
        matches = [member for member in source.getmembers() if Path(member.name).name == member_name]
        if len(matches) != 1 or not matches[0].isfile():
            raise RuntimeError(f"expected exactly one regular {member_name} in {archive}")
        stream = source.extractfile(matches[0])
        if stream is None:
            raise RuntimeError(f"could not read {member_name} from {archive}")
        with destination.open("wb") as out:
            shutil.copyfileobj(stream, out)


def write_checksums(package_dir: Path) -> None:
    entries = []
    for path in sorted(package_dir.iterdir(), key=lambda item: item.name):
        if path.is_file() and path.name != "SHA256SUMS":
            entries.append(f"{sha256(path)}  {path.name}")
    (package_dir / "SHA256SUMS").write_text("\n".join(entries) + "\n", encoding="utf-8")


def create_archive(package_dir: Path, output_dir: Path, archive_format: str) -> Path:
    if archive_format == "zip":
        output = output_dir / f"{package_dir.name}.zip"
        with zipfile.ZipFile(output, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
            for path in sorted(package_dir.rglob("*")):
                if path.is_file():
                    archive.write(path, path.relative_to(package_dir.parent))
        return output
    output = output_dir / f"{package_dir.name}.tar.gz"
    with tarfile.open(output, "w:gz", compresslevel=9) as archive:
        archive.add(package_dir, arcname=package_dir.name)
    return output


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    parser.add_argument("--app-version", required=True)
    parser.add_argument("--binary-dir", type=Path)
    parser.add_argument("--output-dir", type=Path, default=Path("dist"))
    parser.add_argument("--cache-dir", type=Path)
    args = parser.parse_args()

    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:[-+][0-9A-Za-z.-]+)?", args.app_version):
        raise SystemExit(f"invalid application version: {args.app_version}")

    repo = Path(__file__).resolve().parent.parent
    target = TARGETS[args.target]
    binary_dir = args.binary_dir or repo / "target" / args.target / "release"
    output_dir = args.output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    cache_dir = (args.cache_dir or output_dir / ".cache").resolve()
    cache_dir.mkdir(parents=True, exist_ok=True)

    suffix = ".exe" if args.target == "x86_64-pc-windows-msvc" else ""
    binaries = [binary_dir / f"aeon-memory{suffix}", binary_dir / f"aeon-memory-server{suffix}"]
    missing = [str(path) for path in binaries if not path.is_file()]
    if missing:
        raise SystemExit(f"missing release binaries: {', '.join(missing)}")

    asset = cache_dir / target["asset"]
    download_verified(
        f"{SQLITE_VEC_BASE_URL}/{target['asset']}", asset, target["sha256"]
    )

    package_name = f"aeon-memory-{args.app_version}-{args.target}"
    with tempfile.TemporaryDirectory(prefix="aeon-memory-native-package-") as temporary:
        package_dir = Path(temporary) / package_name
        package_dir.mkdir()
        for binary in binaries:
            destination = package_dir / binary.name
            shutil.copy2(binary, destination)
            if os.name != "nt":
                destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)

        extract_named_member(asset, target["vec"], package_dir / target["vec"])
        shutil.copy2(
            repo / "config" / "aeon-memory.example.yaml",
            package_dir / "aeon-memory.yaml",
        )
        shutil.copy2(repo / "NATIVE_PACKAGE_CN.md", package_dir / "使用说明.md")
        shutil.copy2(repo / "README_CN.md", package_dir / "README_CN.md")
        shutil.copy2(repo / "OFFLOAD_API_CONTRACT.md", package_dir / "OFFLOAD_API_CONTRACT.md")
        shutil.copy2(repo / "LICENSE", package_dir / "LICENSE")
        shutil.copy2(repo / "THIRD_PARTY_NOTICES.md", package_dir / "THIRD_PARTY_NOTICES.md")
        write_checksums(package_dir)
        output = create_archive(package_dir, output_dir, target["format"])

    print(output)


if __name__ == "__main__":
    main()
