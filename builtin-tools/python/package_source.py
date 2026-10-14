#!/usr/bin/env python3
"""Package the locked Python application and library sources for offline relinking."""

import hashlib
import json
import shutil
import subprocess
import sys
import tarfile
from pathlib import Path


def main():
    root = Path(__file__).resolve().parents[2]
    staging = Path(sys.argv[1]).resolve()
    source = staging / "python-source"
    source.mkdir()
    snapshot = staging / "source.tar"
    subprocess.run(
        ["git", "archive", "HEAD", "--output", str(snapshot)], cwd=root, check=True
    )
    with tarfile.open(snapshot) as archive:
        archive.extractall(source, filter="data")
    snapshot.unlink()

    manifest = root / "builtin-tools/python/Cargo.toml"
    vendor = source / "vendor"
    config = subprocess.check_output(
        ["cargo", "vendor", "--locked", "--versioned-dirs", "--manifest-path", str(manifest), str(vendor)],
        cwd=root,
        text=True,
    )
    with (source / ".cargo/config.toml").open("a") as output:
        output.write("\n" + config.replace(str(vendor), "vendor"))

    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--locked", "--format-version", "1", "--manifest-path", str(manifest)],
        cwd=root,
        text=True,
    ))
    # cargo vendor omits test directories from pylib's include list, but the Git
    # build freezes the entire Lib tree. Materialize that exact tree in the bundle.
    pylib = next(p for p in metadata["packages"] if p["name"] == "rustpython-pylib")
    shutil.copytree(
        Path(pylib["manifest_path"]).parent / "Lib",
        vendor / f"rustpython-pylib-{pylib['version']}" / "Lib",
        dirs_exist_ok=True,
        ignore=shutil.ignore_patterns("__pycache__", "*.pyc"),
    )

    asset = staging / "python-source.tar.gz"
    with tarfile.open(asset, "w:gz", dereference=True) as archive:
        archive.add(source, arcname="python-source")
    with asset.open("rb") as data:
        digest = hashlib.file_digest(data, "sha256").hexdigest()
    (staging / "python-source.tar.gz.sha256").write_text(f"{digest}  {asset.name}\n")
    provenance_path = staging / "provenance.json"
    provenance = json.loads(provenance_path.read_text())
    provenance["source"]["asset"] = asset.name
    provenance["source"]["sha256"] = digest
    provenance_path.write_text(json.dumps(provenance, indent=2) + "\n")
    shutil.rmtree(source)
    print(f"Source and relinking archive: {asset} ({digest})")


if __name__ == "__main__":
    main()
