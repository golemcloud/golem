#!/usr/bin/env python3
"""Build same-world generated fixtures, verify them, and attribute release code size.

Use separate --output directories before/after regenerating bindings to benchmark
an updated wit-bindgen without modifying generated bindings by hand.
"""
import argparse
import collections
import json
from pathlib import Path
import re
import shutil
import subprocess


def run(*args, cwd, env=None):
    result = subprocess.run(list(map(str, args)), cwd=cwd, env=env,
                            stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    if result.returncode:
        raise RuntimeError(f"{' '.join(map(str, args))}\n{result.stdout}")
    return result.stdout


def uleb(data, pos):
    value = shift = 0
    while True:
        byte = data[pos]
        pos += 1
        value |= (byte & 127) << shift
        if byte < 128:
            return value, pos
        shift += 7


def string(data, pos):
    length, pos = uleb(data, pos)
    return data[pos:pos + length].decode(), pos + length


def attribute(wasm):
    data = wasm.read_bytes()
    pos = 8
    sections = collections.Counter()
    names = {}
    bodies = []
    while pos < len(data):
        kind = data[pos]
        size, start = uleb(data, pos + 1)
        end = start + size
        sections[str(kind)] += end - pos
        if kind == 0:
            name, cursor = string(data, start)
            if name == "name":
                while cursor < end:
                    subkind = data[cursor]
                    subsize, cursor = uleb(data, cursor + 1)
                    subend = cursor + subsize
                    if subkind == 1:
                        count, cursor = uleb(data, cursor)
                        for _ in range(count):
                            index, cursor = uleb(data, cursor)
                            names[index], cursor = string(data, cursor)
                    cursor = subend
        elif kind == 10:
            count, cursor = uleb(data, start)
            for _ in range(count):
                length, cursor = uleb(data, cursor)
                bodies.append(length)
                cursor += length
        pos = end
    # Defined functions follow imported functions; their indices are contiguous.
    # Count imports from wasm-tools text, rather than guessing from name entries.
    text = run("wasm-tools", "print", wasm, cwd=wasm.parent)
    imported = len(re.findall(r'^  \(import .*\(func ', text, re.M))
    packages = collections.Counter()
    for offset, length in enumerate(bodies):
        name = names.get(imported + offset, "<unnamed>")
        match = re.match(r'_M0[FMIP]*P(\d+)(?=10golemcloud)', name)
        package = "<runtime/fixture>"
        if match:
            cursor = match.end()
            parts = []
            for _ in range(int(match[1])):
                size = re.match(r'\d+', name[cursor:])
                assert size, name
                cursor += len(size[0])
                part = name[cursor:cursor + int(size[0])]
                parts.append(part.replace("__", "_").replace("_2d", "-"))
                cursor += int(size[0])
            package = "/".join(parts)
        packages[package] += length
    return {"sections": dict(sections), "code_by_package": dict(packages.most_common()),
            "functions": len(bodies)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--sdk", type=Path, default=Path(__file__).resolve().parents[1])
    parser.add_argument("--tools", type=Path)
    parser.add_argument("--measure-only", action="store_true", help="Skip behavioral tests for baseline measurements")
    args = parser.parse_args()
    sdk = args.sdk.resolve()
    tools = (args.tools or sdk.parent / "golem_sdk_tools").resolve()
    fixtures = Path(__file__).resolve().parents[2] / "golem_sdk_tools/test-data/minimal-exports"
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    project = output / "fixtures"
    project.mkdir(exist_ok=True)
    (output / "moon.work").write_text(f'members = [{json.dumps(str(sdk))}, "./fixtures"]\n')
    (project / "moon.mod").write_text('name = "golemcloud/minimal_export_fixtures"\npreferred_target = "wasm"\nimport { "golemcloud/golem_sdk@0.5.1" }\n')
    expected_exports = re.findall(r'"[^":\n]+:([^"\n]+)"', (sdk / "gen/moon.pkg").read_text())
    expected_exports = [s for s in expected_exports if "#" in s or s == "cabi_realloc"]
    summaries = {}
    for name, agents, tools_present in [("empty", False, False), ("tool", False, True), ("agent", True, False), ("mixed", True, True)]:
        package = project / name
        if package.exists():
            shutil.rmtree(package)
        package.mkdir()
        (package / "moon.pkg").write_text('import {\n}\noptions(\n  "is-main": true,\n)\n')
        (package / "main.mbt").write_text("fn main {}\n")
        for enabled, source in [(agents, "agent.mbt"), (tools_present, "tool.mbt")]:
            if enabled:
                shutil.copyfile(fixtures / source, package / source)
        run("moon", "run", "cmd", "--", "reexports", sdk, package, cwd=tools)
        run("moon", "run", "cmd", "--", "agents", project, "--component-dir", name, cwd=tools)
        before = {p.name: p.read_bytes() for p in package.iterdir()}
        run("moon", "run", "cmd", "--", "agents", project, "--component-dir", name, cwd=tools)
        assert before == {p.name: p.read_bytes() for p in package.iterdir()}, "codegen is not idempotent"
        run("moon", "build", "--target", "wasm", "--release", "--no-strip", name, cwd=project)
        candidates = list((output / "_build").rglob(f"{name}.wasm"))
        assert len(candidates) == 1, candidates
        wasm = output / f"{name}.wasm"
        shutil.copyfile(candidates[0], wasm)
        stripped = output / f"{name}.stripped.wasm"
        run("wasm-tools", "strip", "--all", wasm, "-o", stripped, cwd=output)
        embedded = output / f"{name}.embedded.wasm"
        component = output / f"{name}.component.wasm"
        run("wasm-tools", "component", "embed", sdk / "wit", stripped, "--encoding", "utf16", "-o", embedded, cwd=output)
        run("wasm-tools", "component", "new", embedded, "-o", component, cwd=output)
        run("wasm-tools", "validate", "--features", "all", component, cwd=output)
        abi = json.loads(run("node", Path(__file__).with_name("measure-minimal-exports.mjs"), wasm, json.dumps(expected_exports), str(int(agents)), str(int(tools_present)), cwd=output))
        summary = {"release_bytes": wasm.stat().st_size, "stripped_bytes": stripped.stat().st_size,
                   "component_bytes": component.stat().st_size, **attribute(wasm), **abi}
        summary["generated_export_code_bytes"] = sum(
            size for package, size in summary["code_by_package"].items()
            if package.startswith("golemcloud/golem_sdk/gen/"))
        if not args.measure_only:
            absent = ["rpc", "tool-middleware", "tool-middleware-exports"]
            if not agents:
                absent += ["agents", "agent-exports"]
            if not tools_present:
                absent += ["tool", "tool-core", "tool-exports"]
            for package_name in absent:
                assert f"golemcloud/golem_sdk/{package_name}" not in summary["code_by_package"], package_name
            imports = {
                "gen/interface/golem/agent/guest": "agentGuest", "gen/interface/golem/tool/guest": "toolGuest",
                "gen/interface/golem/tool/tool-middleware-guest": "middlewareGuest",
                "gen/interface/golem/api/save-snapshot": "saveSnapshot", "gen/interface/golem/api/load-snapshot": "loadSnapshot",
                "async-core": "asyncCore", "interface/golem/core/types": "types",
                "interface/golem/agent/common": "common", "schema_model": "model", "schema_model_host": "modelHost",
            }
            with (package / "moon.pkg").open("a") as file:
                file.write('\nimport {\n' + ''.join(f'  "golemcloud/golem_sdk/{path}" @{alias},\n' for path, alias in imports.items()) + '} for "wbtest"\n')
            source = f'let has_agents : Bool = {str(agents).lower()}\nlet has_tools : Bool = {str(tools_present).lower()}\n'
            (package / "checks_wbtest.mbt").write_text(source + (fixtures / "checks_wbtest.mbt").read_text())
            summary["tests"] = run("bash", sdk / "scripts/run-sdk-tests.sh", package, cwd=project).strip().splitlines()[-1]
        summaries[name] = summary
        print(name, json.dumps(summary), flush=True)
    (output / "measurements.json").write_text(json.dumps(summaries, indent=2) + "\n")


if __name__ == "__main__":
    main()
