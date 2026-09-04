#!/usr/bin/env python3

import json
import os
import sys


def fail(message: str) -> None:
    print(message, file=sys.stderr)
    raise SystemExit(1)


if len(sys.argv) != 3:
    fail(f"usage: {sys.argv[0]} <cargo-json-messages> <repository-root>")

matches = {"lib": set(), "integration": set()}
repo_root = os.path.realpath(sys.argv[2])
expected_sources = {
    "lib": os.path.join(repo_root, "golem-worker-executor", "src", "lib.rs"),
    "integration": os.path.join(repo_root, "golem-worker-executor", "tests", "lib.rs"),
}
with open(sys.argv[1], encoding="utf-8") as messages:
    for line_number, line in enumerate(messages, 1):
        if not line.strip():
            continue
        try:
            message = json.loads(line)
        except json.JSONDecodeError as error:
            fail(f"invalid Cargo JSON message on line {line_number}: {error}")
        if message.get("reason") != "compiler-artifact":
            continue
        if not message.get("profile", {}).get("test"):
            continue
        target = message.get("target", {})
        executable = message.get("executable")
        if target.get("name") == "golem_worker_executor" and target.get("kind") == ["lib"]:
            label = "lib"
        elif target.get("name") == "integration" and target.get("kind") == ["test"]:
            label = "integration"
        else:
            continue
        if os.path.realpath(target.get("src_path", "")) != expected_sources[label]:
            continue
        if executable is not None:
            matches[label].add(executable)

for label, executables in matches.items():
    if len(executables) != 1:
        fail(f"expected exactly one {label} test executable from Cargo, got {sorted(executables)}")
    executable = executables.pop()
    if not os.path.isfile(executable) or not os.access(executable, os.X_OK):
        fail(f"Cargo emitted a missing or non-executable {label} test artifact: {executable}")
    print(f"{label}\t{executable}")
