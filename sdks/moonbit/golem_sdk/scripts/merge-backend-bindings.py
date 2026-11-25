#!/usr/bin/env python3
"""Assemble one SDK from the two wit-bindgen backends and maintained stubs."""

from pathlib import Path
import re
import shutil
import sys


def merge(wasm: Path, gc: Path, preserved: Path, output: Path) -> None:
    roots = ("interface", "async-core", "world", "gen")
    for root in roots:
        # The generator emits the same package layout, with a GC-only cabi package.
        paths = sorted(
            {p.relative_to(wasm) for p in (wasm / root).rglob("*") if p.is_file()}
            | {p.relative_to(gc) for p in (gc / root).rglob("*") if p.is_file()}
            | {p.relative_to(preserved) for p in (preserved / root).rglob("*") if p.is_file()}
        )
        targets: dict[Path, dict[str, list[str]]] = {}
        for path in paths:
            if path.name == "moon.pkg":
                continue
            plain, collected = wasm / path, gc / path
            dest = output / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            if (preserved / path).exists():
                shutil.copyfile(preserved / path, dest)
            elif plain.exists() and collected.exists() and plain.read_bytes() == collected.read_bytes():
                shutil.copyfile(plain, dest)
            else:
                for source, backend in ((plain, "wasm"), (collected, "wasm-gc")):
                    if not source.exists():
                        continue
                    name = path.name
                    if backend == "wasm-gc" and plain.exists():
                        name = path.stem + "_gc" + path.suffix
                    shutil.copyfile(source, dest.with_name(name))
                    if path.suffix == ".mbt":
                        targets.setdefault(path.parent, {})[name] = [backend]

        for path in (p for p in paths if p.name == "moon.pkg"):
            plain, collected, maintained = wasm / path, gc / path, preserved / path
            source = maintained if maintained.exists() else plain if plain.exists() else collected
            descriptor = re.sub(
                r'^\s*// BEGIN generated backend\n.*?^\s*// END generated backend\n',
                '', source.read_text(), flags=re.M | re.S,
            ).rstrip() + "\n"
            if plain.exists() and collected.exists():
                descriptor = re.sub(r'supported_targets\s*=\s*"[^"]*"',
                                    'supported_targets = "+wasm+wasm-gc"', descriptor)
                # Keep ordinary imports and their aliases unchanged. The canonical
                # allocator is the only GC-specific dependency in generated packages.
                for line in collected.read_text().splitlines():
                    if "@witCabi" in line and "@witCabi" not in descriptor:
                        descriptor += '\n// BEGIN generated backend\n#cfg(target = "wasm-gc")\nimport {\n' + line + '\n}\n// END generated backend\n'
                if '"wasm": {' in descriptor:
                    gc_link = re.search(r'    "wasm-gc": \{.*?^    \},', collected.read_text(), re.M | re.S)
                    if gc_link is None:
                        raise ValueError(f"missing GC link configuration: {path}")
                    descriptor = descriptor.replace('  link: {', '  link: {\n    // BEGIN generated backend\n' + gc_link[0] + '\n    // END generated backend', 1)
            if path.parent in targets:
                rules = "\n".join(
                    f'    "{name}": ["{backends[0]}"],'
                    for name, backends in sorted(targets[path.parent].items())
                )
                rules = "  targets: {\n" + rules + "\n  },\n"
                if "options(\n" in descriptor:
                    descriptor = descriptor.replace(
                        "options(\n", "options(\n  // BEGIN generated backend\n" + rules + "  // END generated backend\n", 1,
                    )
                else:
                    descriptor += "\n// BEGIN generated backend\noptions(\n" + rules + ")\n// END generated backend\n"
            dest = output / path
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(descriptor)


if __name__ == "__main__":
    merge(*map(Path, sys.argv[1:]))
