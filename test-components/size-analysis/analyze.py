#!/usr/bin/env python3
"""Informational Rust component size reports; requires Python 3.11 and wasm-tools."""

import argparse
import collections
import hashlib
import json
import os
import re
import shutil
import subprocess
import time
from pathlib import Path

CORE = b"\0asm\x01\0\0\0"
COMPONENT = b"\0asm\x0d\0\x01\0"
CORE_SECTIONS = {
    0: "custom",
    1: "type",
    2: "import",
    3: "function",
    4: "table",
    5: "memory",
    6: "global",
    7: "export",
    8: "start",
    9: "element",
    10: "code",
    11: "data",
    12: "data-count",
    13: "tag",
}


def read_u32(data, offset):
    value = 0
    for shift in range(0, 35, 7):
        if offset >= len(data):
            raise ValueError("truncated u32 LEB128")
        byte = data[offset]
        offset += 1
        value |= (byte & 127) << shift
        if byte < 128:
            if value > 0xFFFFFFFF:
                raise ValueError("u32 overflow")
            return value, offset
    raise ValueError("invalid u32 LEB128")


def u32(value):
    result = bytearray()
    while value >= 128:
        result.append((value & 127) | 128)
        value >>= 7
    result.append(value)
    return bytes(result)


def sections(data):
    if data[:8] not in (CORE, COMPONENT):
        raise ValueError("expected a core module or component")
    offset = 8
    while offset < len(data):
        start = offset
        kind = data[offset]
        size, payload = read_u32(data, offset + 1)
        offset = payload + size
        if offset > len(data):
            raise ValueError("section extends beyond binary")
        yield kind, start, payload, offset


def custom_name(payload):
    length, offset = read_u32(payload, 0)
    if offset + length > len(payload):
        raise ValueError("invalid custom section name")
    return payload[offset : offset + length].decode("utf-8")


def rewrite_cores(data, transform):
    """Keep component wiring/types/custom metadata, replacing only embedded cores."""
    if data[:8] == CORE:
        return transform(data)
    result = bytearray(data[:8])
    for kind, start, payload, end in sections(data):
        if kind in (1, 4):  # embedded core module / nested component
            replacement = rewrite_cores(data[payload:end], transform)
            result.extend(bytes([kind]) + u32(len(replacement)) + replacement)
        else:
            result.extend(data[start:end])
    return bytes(result)


def preserve_component_types(original, optimized):
    """Binaryen must not lose the metadata needed by `component new` on raw cores."""

    def is_type(kind, payload):
        return kind == 0 and custom_name(payload).startswith("component-type")

    result = bytearray(CORE)
    for kind, start, payload, end in sections(optimized):
        if not is_type(kind, optimized[payload:end]):
            result.extend(optimized[start:end])
    for kind, start, payload, end in sections(original):
        if is_type(kind, original[payload:end]):
            result.extend(original[start:end])
    return bytes(result)


def inspect(data, label="component", offset=0):
    rows = [{"path": label, "section": "header", "offset": offset, "bytes": 8}]
    is_core = data[:8] == CORE
    for kind, start, payload, end in sections(data):
        name = CORE_SECTIONS.get(kind, str(kind)) if is_core else f"component-{kind}"
        if kind == 0:
            name = "custom:" + custom_name(data[payload:end])
        if not is_core and kind in (1, 4):
            # Attribute section framing separately, so all rows sum to file size.
            child = f"{label}/{len(rows)}"
            rows.append(
                {
                    "path": child,
                    "section": "framing",
                    "offset": offset + start,
                    "bytes": payload - start,
                }
            )
            rows.extend(inspect(data[payload:end], child, offset + payload))
        else:
            rows.append(
                {
                    "path": label,
                    "section": name,
                    "offset": offset + start,
                    "bytes": end - start,
                }
            )
    return rows


def run(command, output=None, env=None):
    print("+", " ".join(map(str, command)), flush=True)
    result = subprocess.run(
        list(map(str, command)), capture_output=True, env=env, check=False
    )
    if output:
        Path(output).write_bytes(result.stdout)
        Path(str(output) + ".stderr").write_bytes(result.stderr)
    if result.returncode:
        raise RuntimeError(
            f"command failed ({result.returncode}): {command}\n"
            + result.stderr.decode(errors="replace")[-4000:]
        )
    return result.stdout


def environment_override(value):
    key, separator, setting = value.partition("=")
    if not separator or not key or not setting:
        raise argparse.ArgumentTypeError("expected KEY=VALUE")
    if not key.startswith("CARGO_PROFILE_"):
        raise argparse.ArgumentTypeError("only CARGO_PROFILE_* overrides are recorded")
    return key, setting


def attribution(core, directory):
    if not shutil.which("twiggy"):
        (directory / "attribution-unavailable.txt").write_text(
            "Install twiggy for symbol attribution.\n"
        )
        return
    try:
        demangled = directory / "demangled.wasm"
        run(["wasm-tools", "demangle", core, "-o", demangled])
        raw = run(
            ["twiggy", "top", "-n", "1000000", "--format", "json", demangled],
            directory / "symbols.json",
        )
        groups = collections.Counter()
        for item in json.loads(raw):
            # Symbol namespace is an estimate, not source-level attribution after LTO.
            match = re.match(r"<?(\w+)(?:\[[0-9a-f]+\])?::", item["name"])
            groups[match[1] if match else "[unattributed]"] += item["shallow_size"]
        (directory / "crate-estimates.json").write_text(
            json.dumps(groups.most_common(), indent=2) + "\n"
        )
    except (RuntimeError, ValueError, KeyError) as error:
        (directory / "attribution-unavailable.txt").write_text(str(error) + "\n")


def report(wasm, directory):
    directory.mkdir(parents=True)
    data = wasm.read_bytes()
    run(["wasm-tools", "validate", "--features", "all", wasm])
    run(["wasm-tools", "objdump", wasm], directory / "objdump.txt")
    rows = inspect(data)
    (directory / "sections.json").write_text(json.dumps(rows, indent=2) + "\n")
    totals = collections.Counter()
    for row in rows:
        totals[row["section"]] += row["bytes"]
    assert sum(totals.values()) == len(data)
    summary = {
        "bytes": len(data),
        "code": totals["code"],
        "data": totals["data"],
        "custom": sum(v for k, v in totals.items() if k.startswith("custom:")),
        "sha256": hashlib.sha256(data).hexdigest(),
    }
    (directory / "summary.json").write_text(json.dumps(summary, indent=2) + "\n")
    cores = []

    def extract(core):
        target = directory / f"core-{len(cores)}"
        target.mkdir()
        path = target / "module.wasm"
        path.write_bytes(core)
        cores.append(path)
        attribution(path, target)
        return core

    rewrite_cores(data, extract)
    if data[:8] == COMPONENT:
        run(["wasm-tools", "component", "wit", wasm], directory / "world.wit")
    return summary


def optimize(wasm, output):
    """Optimize cores then re-bundle, never componentize an untyped extracted core."""
    work = output.parent / "optimization"
    work.mkdir()
    count = 0

    def transform(core):
        nonlocal count
        source = work / f"core-{count}.wasm"
        target = work / f"core-{count}-Oz.wasm"
        count += 1
        source.write_bytes(core)
        # Enabling all proposals lets Binaryen introduce typed references/GC that
        # Golem's engine rejects. Keep the Rust wasm32 core feature baseline.
        run(
            [
                "wasm-opt",
                source,
                "-Oz",
                "--enable-bulk-memory",
                "--enable-sign-ext",
                "--enable-nontrapping-float-to-int",
                "--enable-mutable-globals",
                "--enable-multivalue",
                "--enable-reference-types",
                "-g",
                "-o",
                target,
            ]
        )
        return preserve_component_types(core, target.read_bytes())

    data = wasm.read_bytes()
    output.write_bytes(rewrite_cores(data, transform))
    run(["wasm-tools", "validate", "--features", "all", output])
    if data[:8] == COMPONENT:
        before = run(["wasm-tools", "component", "wit", wasm])
        after = run(["wasm-tools", "component", "wit", output])
        if before != after:
            raise ValueError("optimization changed the component's WIT contract")


def build(manifest, package, profile, directory, overrides):
    env = os.environ.copy()
    env.update(overrides)
    workspace = Path(
        run(
            [
                "cargo",
                "locate-project",
                "--workspace",
                "--message-format",
                "plain",
                "--manifest-path",
                manifest,
            ]
        )
        .decode()
        .strip()
    ).parent
    shutil.copyfile(workspace / "Cargo.lock", directory / "Cargo.lock")
    command = [
        "cargo",
        "build",
        "--locked",
        "--manifest-path",
        manifest,
        "--target",
        "wasm32-wasip2",
        "--profile",
        profile,
        "--message-format=json-render-diagnostics",
    ]
    if package:
        command += ["-p", package]
    start = time.monotonic()
    raw = run(command, directory / "build.jsonl", env)
    artifacts = set()
    for line in raw.splitlines():
        message = json.loads(line)
        if message.get("reason") == "compiler-artifact":
            artifacts.update(
                Path(p) for p in message["filenames"] if p.endswith(".wasm")
            )
    if len(artifacts) != 1:
        raise ValueError(
            f"expected one WASM artifact; select --package (found {artifacts})"
        )
    path = directory / "component.wasm"
    shutil.copyfile(artifacts.pop(), path)
    (directory / "build-settings.json").write_text(
        json.dumps(
            {
                "command": list(map(str, command)),
                "overrides": overrides,
                "seconds": time.monotonic() - start,
            },
            indent=2,
        )
        + "\n"
    )
    return path


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    source = parser.add_mutually_exclusive_group(required=True)
    source.add_argument("--manifest", type=Path)
    source.add_argument("--wasm", type=Path)
    parser.add_argument("--package")
    parser.add_argument("--profile", default="release")
    parser.add_argument(
        "--out", required=True, type=Path, help="new, non-existing report directory"
    )
    parser.add_argument(
        "--matrix", action="store_true", help="independent release profile comparisons"
    )
    parser.add_argument(
        "--optimize", action="store_true", help="experimental per-core Binaryen -Oz"
    )
    parser.add_argument(
        "--override",
        action="append",
        default=[],
        type=environment_override,
        metavar="CARGO_PROFILE_*=VALUE",
        help="recorded Cargo profile override (repeatable)",
    )
    args = parser.parse_args()
    if args.matrix and (not args.manifest or args.profile != "release"):
        parser.error("--matrix requires --manifest and --profile release")
    if args.matrix and args.override:
        parser.error("--matrix and --override cannot be combined")
    if args.optimize and not shutil.which("wasm-opt"):
        parser.error("--optimize requires wasm-opt on PATH")
    args.out.mkdir(parents=True, exist_ok=False)
    versions = {}
    for tool in ("rustc", "cargo", "wasm-tools", "wasm-opt", "twiggy"):
        versions[tool] = (
            run([tool, "--version"]).decode().strip()
            if shutil.which(tool)
            else "unavailable"
        )
    (args.out / "versions.json").write_text(json.dumps(versions, indent=2) + "\n")
    variants = {"current": dict(args.override)}
    if args.matrix:
        baseline = {
            "CARGO_PROFILE_RELEASE_OPT_LEVEL": "s",
            "CARGO_PROFILE_RELEASE_LTO": "true",
            "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "16",
            "CARGO_PROFILE_RELEASE_PANIC": "unwind",
            "CARGO_PROFILE_RELEASE_STRIP": "debuginfo",
            "CARGO_PROFILE_RELEASE_DEBUG": "false",
        }
        variants = {"baseline-s": baseline}
        for label, key, value in (
            ("strip", "STRIP", "symbols"),
            ("cgu1", "CODEGEN_UNITS", "1"),
            ("abort", "PANIC", "abort"),
            ("z", "OPT_LEVEL", "z"),
        ):
            variants[label] = baseline | {"CARGO_PROFILE_RELEASE_" + key: value}
    summaries = {}
    for label, overrides in variants.items():
        directory = args.out / label
        directory.mkdir()
        if args.manifest:
            wasm = build(
                args.manifest, args.package, args.profile, directory, overrides
            )
        else:
            wasm = directory / "component.wasm"
            shutil.copyfile(args.wasm, wasm)
        summaries[label] = report(wasm, directory / "report")
        if args.optimize:
            optimized = directory / "optimized.wasm"
            optimize(wasm, optimized)
            summaries[label + "-Oz"] = report(optimized, directory / "optimized-report")
        (args.out / "summary.json").write_text(json.dumps(summaries, indent=2) + "\n")
    print(json.dumps(summaries, indent=2))


if __name__ == "__main__":
    main()
