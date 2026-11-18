#!/usr/bin/env python3
"""Reuse exported fixture WASMs by checked-in SDK, CLI and application source.

Dependency resolution is deliberately not refreshed on a hit. Inventories come
from successful builds, and entries are published only after fixture validation.
"""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import tempfile
import tomllib


SCHEMA = 1


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def identity(value):
    return hashlib.sha256(json.dumps(value, sort_keys=True).encode()).hexdigest()


def cli_source_roots(root):
    """Include libraries used by CLI migration, generation and composition."""
    workspace = tomllib.loads((root / "Cargo.toml").read_text())["workspace"]["dependencies"]
    pending, found = [root / "cli/golem-cli/Cargo.toml"], set()
    while pending:
        manifest = pending.pop().resolve()
        if manifest in found:
            continue
        if not manifest.is_relative_to(root):
            raise ValueError("CLI dependency outside checkout")
        found.add(manifest)
        package = tomllib.loads(manifest.read_text())
        for table in [package, *package.get("target", {}).values()]:
            for kind in ("dependencies", "build-dependencies"):
                for name, spec in table.get(kind, {}).items():
                    base = manifest.parent
                    if isinstance(spec, dict) and spec.get("workspace"):
                        spec, base = workspace[name], root
                    if isinstance(spec, dict) and "path" in spec:
                        pending.append(base / spec["path"] / "Cargo.toml")
    return {str(path.parent.relative_to(root)) for path in found}


def fingerprint(root, app, variant):
    if Path(app).name != app or app in ("", ".", ".."):
        raise ValueError("invalid application directory")
    tracked = subprocess.run(["git", "ls-files", "-z"], cwd=root, capture_output=True,
                             check=True, timeout=30).stdout.decode().split("\0")
    component = f"test-components/{app}"
    if not any(path.startswith(component + "/") for path in tracked):
        raise ValueError("application has no checked-in source")
    components, pending = set(), [component]
    while pending:
        directory = pending.pop()
        if directory in components:
            continue
        components.add(directory)
        for relative in tracked:
            if relative.startswith(directory + "/"):
                # Shared fixture applications can be referenced by relative paths.
                text = (root / relative).read_bytes().decode("utf-8", errors="ignore")
                for sibling in re.findall(r"(?<![./])\.\./([A-Za-z0-9_-]+)", text):
                    candidate = root / "test-components" / sibling
                    if candidate.is_dir():
                        pending.append(f"test-components/{sibling}")
    roots = {"sdks", "wit", "cli", "golem-skills/skills", "openapi", ".cargo",
             ".github/actions", *components, *cli_source_roots(root)}
    paths = [path for path in tracked if path and (
        path in ("Cargo.toml", "Cargo.lock") or path.startswith("Makefile")
        or any(path.startswith(prefix + "/") for prefix in roots)
        or (path.startswith("test-components/") and path.count("/") == 1))]
    files = {}
    while paths:
        relative = paths.pop()
        if relative in files:
            continue
        path = root / relative
        if path.is_symlink():
            target = path.resolve()
            if not target.is_relative_to(root):
                raise ValueError("source symlink outside checkout")
            files[relative] = {"link": os.readlink(path)}
            target_relative = str(target.relative_to(root))
            paths.extend(p for p in tracked if p == target_relative or p.startswith(target_relative + "/"))
        else:
            files[relative] = [digest(path), path.stat().st_mode & 0o777]
    return identity({"schema": SCHEMA, "app": app, "variant": variant,
                     "sources": files, "helper": digest(Path(__file__))})


def stamps(root):
    return {path.name: [path.stat().st_size, path.stat().st_mtime_ns, path.stat().st_ctime_ns]
            for path in (root / "test-components").glob("*.wasm")}


def inventory(directory):
    return {path.name: {"sha256": digest(path), "size": path.stat().st_size,
                        "mode": path.stat().st_mode & 0o777}
            for path in directory.iterdir() if path.is_file() and not path.is_symlink()}


def verify(bundle, key):
    manifest = json.loads((bundle / "COMPLETE.json").read_text())
    if not isinstance(manifest, dict) or manifest.get("schema") != SCHEMA or manifest.get("key") != key:
        raise ValueError("bundle identity mismatch")
    files = manifest.get("files")
    if not isinstance(files, dict) or not files or any(
        not isinstance(name, str) or Path(name).name != name or not name.endswith(".wasm")
        for name in files
    ):
        raise ValueError("invalid output inventory")
    payload = bundle / "files"
    if payload.is_symlink() or any(not path.is_file() or path.is_symlink() for path in payload.iterdir()):
        raise ValueError("bundle contains non-regular files")
    if inventory(payload) != files or any(not info["size"] for info in files.values()):
        raise ValueError("bundle incomplete or corrupt")
    return files


def restore(root, bundle, key):
    files = verify(bundle, key)
    for name in files:
        target = root / "test-components" / name
        # Never copy through a stale checkout symlink.
        target.unlink(missing_ok=True)
        shutil.copy2(bundle / "files" / name, target)


def stage(root, pending, key, before):
    outputs = [name for name, stamp in stamps(root).items() if before.get(name) != stamp]
    if not outputs:
        raise ValueError("build exported no WASMs")
    destination = pending / key
    if destination.exists():
        shutil.rmtree(destination)
    with tempfile.TemporaryDirectory(prefix=".stage-", dir=pending) as temporary:
        bundle = Path(temporary) / "bundle"
        payload = bundle / "files"
        payload.mkdir(parents=True)
        for name in outputs:
            source = root / "test-components" / name
            if source.is_symlink():
                raise ValueError("exported WASM is a symlink")
            shutil.copy2(source, payload / name)
        (bundle / "COMPLETE.json").write_text(json.dumps(
            {"schema": SCHEMA, "key": key, "files": inventory(payload)}, sort_keys=True))
        verify(bundle, key)
        bundle.rename(destination)


def publish(pending, cache):
    cache.mkdir(parents=True, exist_ok=True)
    retained = set()
    for receipt in pending.glob("*.json"):
        key = json.loads(receipt.read_text())["key"]
        if not isinstance(key, str) or not re.fullmatch(r"[0-9a-f]{64}", key):
            raise ValueError("invalid cache receipt key")
        retained.add(key)
    for bundle in sorted(pending.iterdir()):
        if not bundle.is_dir() or bundle.name.startswith("."):
            continue
        key = bundle.name
        verify(bundle, key)
        retained.add(key)
        with tempfile.TemporaryDirectory(prefix=".publish-", dir=cache) as temporary:
            copied = Path(temporary) / "bundle"
            shutil.copytree(bundle, copied)
            verify(copied, key)
            destination = cache / key
            if destination.exists():
                shutil.rmtree(destination)
            copied.rename(destination)
        print(f"Fixture cache published: {key}", flush=True)
    # Called only after fixture checks succeed; interrupted cycles keep old entries.
    for bundle in cache.iterdir():
        if (re.fullmatch(r"[0-9a-f]{64}", bundle.name)
                and bundle.name not in retained and bundle.is_dir()):
            shutil.rmtree(bundle)
            print(f"Fixture cache evicted: {bundle.name}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("operation", choices=("begin", "restore", "stage", "publish"))
    parser.add_argument("app", nargs="?")
    parser.add_argument("variant", nargs="?", default="release")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    cache = Path(os.environ["GOLEM_FIXTURE_CACHE"])
    pending = cache.parent / "pending"
    if args.operation == "begin":
        shutil.rmtree(pending, ignore_errors=True)
        pending.mkdir(parents=True)
    elif args.operation == "publish":
        publish(pending, cache)
    else:
        receipt = pending / (identity([args.app, args.variant]) + ".json")
        try:
            key = fingerprint(root, args.app, args.variant)
            if args.operation == "restore":
                receipt.write_text(json.dumps({"key": key, "before": stamps(root)}))
                restore(root, cache / key, key)
                print(f"Fixture cache hit: {args.app} ({args.variant}) {key}", flush=True)
            else:
                previous = json.loads(receipt.read_text())
                if previous["key"] != key:
                    raise ValueError("checked-in build inputs changed during compilation")
                stage(root, pending, key, previous["before"])
        except (ValueError, KeyError, OSError, subprocess.SubprocessError) as error:
            print(f"Fixture cache {args.operation} miss: {args.app} ({args.variant}): {error}", flush=True)
            return 1 if args.operation == "restore" else 0
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
