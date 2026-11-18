#!/usr/bin/env python3
"""Snapshot every cargo-test-r local-metadata executable before reusing Cargo output."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import struct
import subprocess
import sys
import tempfile


def digest(path):
    with path.open("rb") as binary:
        return hashlib.file_digest(binary, "sha256").hexdigest()


def dependencies(path, managed_targets):
    # ldd also accepts scripts and shared objects; only executable ELF inputs belong here.
    with path.open("rb") as binary:
        header = binary.read(64)
        if (len(header) < 52 or header[:4] != b"\x7fELF" or header[4] not in (1, 2)
                or header[5] not in (1, 2)):
            raise ValueError(f"not an ELF executable: {path}")
        endian = "<" if header[5] == 1 else ">"
        wide = header[4] == 2
        header_size = 64 if wide else 52
        if len(header) < header_size or struct.unpack(endian + "H", header[16:18])[0] not in (2, 3):
            raise ValueError(f"not an ELF executable: {path}")
        offset = struct.unpack_from(endian + ("Q" if wide else "I"), header, 32 if wide else 28)[0]
        entry_size, count = struct.unpack_from(endian + "HH", header, 54 if wide else 42)
        if (entry_size != (56 if wide else 32) or not count or offset < header_size
                or offset + entry_size * count > path.stat().st_size):
            raise ValueError(f"invalid ELF program headers: {path}")
        binary.seek(offset)
        segments = [struct.unpack_from(endian + "I", binary.read(entry_size))[0]
                    for _ in range(count)]
        if 1 not in segments:  # PT_LOAD
            raise ValueError(f"ELF has no loadable segments: {path}")
    result = subprocess.run(["ldd", str(path)], capture_output=True, text=True,
                            env={**os.environ, "LC_ALL": "C"}, timeout=60)
    evidence = result.stdout + result.stderr
    print(json.dumps({"binary": str(path), "ldd_exit": result.returncode,
                      "ldd": evidence}), flush=True)
    if "not found" in evidence:
        raise ValueError(f"unresolved dependencies: {path}")
    lines = [line.strip() for line in evidence.splitlines() if line.strip()]
    if lines in (["statically linked"], ["not a dynamic executable"]):
        if result.returncode not in (0, 1) or 3 in segments:  # PT_INTERP
            raise ValueError(f"ldd failed: {path}")
        return {}
    if result.returncode != 0 or not lines:
        raise ValueError(f"ldd failed: {path}")
    resolved = {}
    for line in lines:
        # Strip ASLR addresses, but retain the exact resolved path as well as its realpath.
        line = re.sub(r"\s+\(0x[0-9a-fA-F]+\)$", "", line)
        if line.startswith(("linux-vdso.so.", "linux-gate.so.")):
            continue
        if " => " in line:
            name, library = line.split(" => ", 1)
        else:
            name = library = line
        library_path = Path(library)
        if not library_path.is_absolute() or not library_path.is_file():
            raise ValueError(f"unsupported ldd resolution for {path}: {line}")
        real = library_path.resolve()
        if any(library_path.is_relative_to(root) or real.is_relative_to(root)
               for root in managed_targets):
            raise ValueError(f"dependency under Cargo-managed target: {library}")
        resolved[name] = (library, str(real))
    if not resolved:
        raise ValueError(f"no dependency resolution from ldd: {path}")
    return resolved


def load_metadata(path):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate metadata key: {key}")
            result[key] = value
        return result

    metadata = json.loads(path.read_text(), object_pairs_hook=unique_object)
    if not isinstance(metadata, dict):
        raise ValueError("metadata must be an object")
    build = metadata.get("rust-build-meta")
    if not isinstance(build, dict) or not isinstance(build.get("target-directory"), str):
        raise ValueError("metadata requires rust-build-meta.target-directory")
    if not Path(build["target-directory"]).is_absolute():
        raise ValueError("target-directory must be absolute")
    binaries = metadata.get("rust-binaries")
    if not isinstance(binaries, dict) or not binaries:
        raise ValueError("metadata requires nonempty rust-binaries")
    for identity, binary in binaries.items():
        if not identity or not isinstance(binary, dict):
            raise ValueError("invalid binary entry")
        for field in ("binary-id", "binary-name", "package-id", "kind", "binary-path", "build-platform"):
            if not isinstance(binary.get(field), str) or not binary[field]:
                raise ValueError(f"{identity}: invalid {field}")
        if binary["binary-id"] != identity or not Path(binary["binary-path"]).is_absolute():
            raise ValueError(f"{identity}: invalid identity or binary-path")
    return metadata


def freeze(metadata_path, destination, output):
    metadata = load_metadata(metadata_path)
    if destination.exists() or destination.is_symlink():
        raise ValueError(f"destination already exists: {destination}")
    destination = destination.resolve()
    output = output.absolute()
    manifest_path = output.with_name(output.name + ".manifest.json")
    for path in (output, manifest_path):
        if path.exists() or path.is_symlink():
            raise ValueError(f"output already exists: {path}")
    managed_targets = {Path(metadata["rust-build-meta"]["target-directory"]).resolve(),
                       (Path(__file__).resolve().parents[2] / "target").resolve()}
    if os.environ.get("CARGO_TARGET_DIR"):
        managed_targets.add(Path(os.environ["CARGO_TARGET_DIR"]).resolve())
    # mkdir without exist_ok also rejects empty pre-existing directories and symlinks.
    destination.mkdir(parents=True)
    manifest = {"version": 1, "snapshot-directory": str(destination), "binaries": {}}
    for index, (identity, binary) in enumerate(metadata["rust-binaries"].items()):
        source = Path(binary["binary-path"])
        mode = source.stat().st_mode
        if not stat.S_ISREG(mode) or not mode & 0o111 or source.stat().st_size == 0:
            raise ValueError(f"not a nonempty executable file: {source}")
        before_hash = digest(source)
        before_dependencies = dependencies(source, managed_targets)
        copied = destination / str(index) / source.name
        copied.parent.mkdir()
        shutil.copy2(source, copied)
        if source.samefile(copied) or stat.S_IMODE(copied.stat().st_mode) != stat.S_IMODE(mode):
            raise ValueError(f"copy did not preserve an independent executable: {source}")
        if digest(copied) != before_hash or digest(source) != before_hash:
            raise ValueError(f"binary changed during snapshot: {source}")
        if dependencies(copied, managed_targets) != before_dependencies:
            raise ValueError(f"dependency resolution changed after relocation: {source}")
        binary["binary-path"] = str(copied)
        manifest["binaries"][identity] = {"binary-path": str(copied), "sha256": before_hash,
                                          "mode": stat.S_IMODE(mode)}
        print(json.dumps({"binary-id": identity, "source": str(source), "copy": str(copied),
                          "sha256": before_hash, "dependencies": before_dependencies}), flush=True)
    encoded = json.dumps(metadata) + "\n"
    manifest["metadata-sha256"] = hashlib.sha256(encoded.encode()).hexdigest()
    check_snapshots(metadata, manifest)
    # The metadata is the commit marker. Never publish it before the manifest and copies.
    publish(manifest_path, json.dumps(manifest) + "\n")
    publish(output, encoded)


def publish(output, encoded):
    output.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.NamedTemporaryFile(mode="w", dir=output.parent, delete=False) as stream:
        temporary = Path(stream.name)
        try:
            stream.write(encoded)
            stream.flush()
            os.fsync(stream.fileno())
            os.replace(temporary, output)
        finally:
            temporary.unlink(missing_ok=True)


def check_snapshots(metadata, manifest):
    if not isinstance(manifest, dict) or manifest.get("version") != 1:
        raise ValueError("invalid snapshot manifest version")
    entries = manifest.get("binaries")
    if not isinstance(entries, dict) or entries.keys() != metadata["rust-binaries"].keys():
        raise ValueError("snapshot manifest identities differ from metadata")
    directory = manifest.get("snapshot-directory")
    if not isinstance(directory, str) or not Path(directory).is_absolute():
        raise ValueError("invalid snapshot directory")
    for identity, binary in metadata["rust-binaries"].items():
        entry = entries[identity]
        path = Path(binary["binary-path"])
        if (not isinstance(entry, dict) or entry.get("binary-path") != str(path)
                or not path.resolve().is_relative_to(Path(directory).resolve())
                or path.is_symlink() or not path.is_file()):
            raise ValueError(f"invalid snapshot path: {identity}")
        actual = digest(path)
        print(json.dumps({"verify": identity, "binary": str(path), "sha256": actual}), flush=True)
        if actual != entry.get("sha256") or stat.S_IMODE(path.stat().st_mode) != entry.get("mode"):
            raise ValueError(f"snapshot content or permissions changed: {identity}")


def verify(metadata_path):
    metadata = load_metadata(metadata_path)
    manifest_path = metadata_path.with_name(metadata_path.name + ".manifest.json")
    manifest = json.loads(manifest_path.read_text())
    if not isinstance(manifest, dict) or digest(metadata_path) != manifest.get("metadata-sha256"):
        raise ValueError("snapshot metadata hash changed")
    check_snapshots(metadata, manifest)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", type=Path)
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--verify", type=Path, metavar="METADATA_JSON")
    args = parser.parse_args()
    if args.verify:
        if args.metadata or args.destination or args.output:
            parser.error("--verify cannot be combined with freeze arguments")
    elif not all((args.metadata, args.destination, args.output)):
        parser.error("freeze requires --metadata, --destination and --output")
    try:
        if args.verify:
            verify(args.verify)
        else:
            freeze(args.metadata, args.destination, args.output)
    except (OSError, ValueError, struct.error, subprocess.SubprocessError) as error:
        print(f"freeze-binaries: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
