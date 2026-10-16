#!/usr/bin/env python3
"""Type-check built-in components against the current host WIT at the build boundary.

Run after fetch-builtin-artifacts to check release pins, or after the owning build
tasks to check local outputs. wasm-tools must be installed. No guest code executes.
"""

import argparse
import json
from pathlib import Path
import shutil
import subprocess
import tempfile


ROOT = Path(__file__).resolve().parents[1]


def run(*args):
    return subprocess.run(args, check=True, capture_output=True, text=True).stdout


def interface_name(resolve, interface_id):
    interface = resolve["interfaces"][interface_id]
    package = resolve["packages"][interface["package"]]["name"]
    name, _, version = package.partition("@")
    return f"{name}/{interface['name']}" + (f"@{version}" if version else "")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--artifact-dir", type=Path,
                        help="Directory with <component>.wasm files; default: owning output paths")
    args = parser.parse_args()
    lock = json.loads((ROOT / "builtin-artifacts.lock.json").read_text())
    resolve = json.loads(run("wasm-tools", "component", "wit", str(ROOT / "wit"), "--json"))
    host = next(world for world in resolve["worlds"] if world["name"] == "golem")
    imports = {interface_name(resolve, item["interface"]["id"])
               for item in host["imports"].values()}
    tool_exports = {interface_name(resolve, item["interface"]["id"])
                    for item in host["exports"].values()}
    tool_exports.add("golem:tool/tool-middleware-guest@0.1.0")
    # The executor links WASI P2 (except HTTP) and P3 alongside the Golem world.
    # Use canonical in-tree definitions, never the artifact's embedded definitions.
    for index, interface in enumerate(resolve["interfaces"]):
        package = resolve["packages"][interface["package"]]["name"]
        if (package.startswith("wasi:")
                and not package.startswith("wasi:http@0.2.")
                and interface["name"] not in {"run", "handler", "handle-watch"}):
            imports.add(interface_name(resolve, index))

    failed = False
    with tempfile.TemporaryDirectory(prefix="builtin-host-contract-") as temp:
        wit = Path(temp)
        shutil.copytree(ROOT / "wit" / "deps", wit / "deps")
        for artifact_id, entry in lock["artifacts"].items():
            component = entry["component"]
            output_dir = "plugins" if artifact_id == "otlp_exporter" else "builtin-tools"
            wasm = ((args.artifact_dir or ROOT / output_dir) / f"{component}.wasm").resolve()
            exports = ({"golem:api/oplog-processor@1.5.0"} if artifact_id == "otlp_exporter"
                       else tool_exports)
            (wit / "host.wit").write_text(
                "package builtin:validation;\nworld builtin {\n"
                + "".join(f"  import {name};\n" for name in sorted(imports))
                + "".join(f"  export {name};\n" for name in sorted(exports)) + "}\n")
            try:
                run("wasm-tools", "validate", "--features", "all", str(wasm))
                # Wasmtime supports compatible WASI patch releases (e.g. 0.2.12
                # imports with the in-tree 0.2.6 definitions).
                run("wasm-tools", "component", "targets", str(wit),
                    "--world", "builtin:validation/builtin", "--semver-compatible", str(wasm))
                print(f"PASS {component}: imports and exports ({', '.join(sorted(exports))})")
            except subprocess.CalledProcessError as error:
                failed = True
                print(f"FAIL {component}: {wasm}\n{error.stderr}")
    return int(failed)


if __name__ == "__main__":
    raise SystemExit(main())
