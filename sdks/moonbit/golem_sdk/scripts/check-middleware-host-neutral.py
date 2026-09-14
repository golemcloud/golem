#!/usr/bin/env python3
"""Reject ambient tool-host dependencies from pure middleware packages."""

import re
from collections import deque
from pathlib import Path


MODULE = "golemcloud/golem_sdk"
FORBIDDEN = f"{MODULE}/interface/golem/tool/host"
ROOTS = (
    f"{MODULE}/tool-core",
    f"{MODULE}/tool-middleware",
    f"{MODULE}/gen-tool-middleware",
)
EXTERNAL_ALLOWLIST = frozenset(
    {
        "moonbitlang/core/deque",
        "moonbitlang/core/encoding/utf8",
        "moonbitlang/core/ref",
        "moonbitlang/core/set",
        "moonbitlang/core/string",
        "moonbitlang/core/strconv",
    }
)
PACKAGE_PATH = re.compile(
    r'"([A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+(?:/[^"\s]+)*)"'
)


def descriptor(module_root: Path, package: str) -> Path:
    relative = package.removeprefix(MODULE).removeprefix("/")
    directory = module_root / relative
    for name in ("moon.pkg", "moon.pkg.json"):
        candidate = directory / name
        if candidate.is_file():
            return candidate
    raise SystemExit(f"local package has no descriptor: {package}")


def imports(module_root: Path, package: str) -> list[str]:
    path = descriptor(module_root, package)
    return sorted(set(PACKAGE_PATH.findall(path.read_text())))


def check_dependency_closure(
    module_root: Path,
    roots: tuple[str, ...] = ROOTS,
    external_allowlist: frozenset[str] = EXTERNAL_ALLOWLIST,
) -> dict[str, str | None]:
    parent: dict[str, str | None] = {root: None for root in roots}
    pending = deque(roots)
    while pending:
        package = pending.popleft()
        if package == FORBIDDEN:
            chain = [package]
            while parent[chain[-1]] is not None:
                chain.append(parent[chain[-1]])
            chain.reverse()
            raise SystemExit(
                "pure middleware dependency closure imports ambient tool host:\n  "
                + "\n  -> ".join(chain)
            )
        for imported in imports(module_root, package):
            if not imported.startswith(f"{MODULE}/"):
                if imported not in external_allowlist:
                    chain = [package]
                    while parent[chain[-1]] is not None:
                        chain.append(parent[chain[-1]])
                    chain.reverse()
                    chain.append(imported)
                    raise SystemExit(
                        "pure middleware dependency closure imports an unapproved external package:\n  "
                        + "\n  -> ".join(chain)
                    )
                continue
            if imported not in parent:
                parent[imported] = package
                pending.append(imported)
    return parent


def main() -> None:
    module_root = Path(__file__).resolve().parent.parent
    closure = check_dependency_closure(module_root)
    print(
        f"middleware dependency closure is host-neutral ({len(closure)} local packages checked)"
    )


if __name__ == "__main__":
    main()
