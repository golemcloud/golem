#!/usr/bin/env python3

import json
import os
import sys


TARGETS = {
    "lib": ("golem_worker_executor", ["lib"], "golem-worker-executor/src/lib.rs"),
    "integration": ("integration", ["test"], "golem-worker-executor/tests/lib.rs"),
}


def select_executables(messages, repo_root, requested):
    if not requested or len(set(requested)) != len(requested):
        raise ValueError("request each test target exactly once")
    if any(label not in TARGETS for label in requested):
        raise ValueError(f"unknown test target; expected one of {sorted(TARGETS)}")
    matches = {label: set() for label in requested}
    expected_sources = {
        label: os.path.join(repo_root, TARGETS[label][2]) for label in requested
    }
    for message in messages:
        if message.get("reason") != "compiler-artifact":
            continue
        if not message.get("profile", {}).get("test"):
            continue
        target = message.get("target", {})
        for label in requested:
            name, kind, _ = TARGETS[label]
            if target.get("name") != name or target.get("kind") != kind:
                continue
            if os.path.realpath(target.get("src_path", "")) != expected_sources[label]:
                continue
            executable = message.get("executable")
            if executable is not None:
                matches[label].add(executable)
    selected = {}
    for label, executables in matches.items():
        if len(executables) != 1:
            raise ValueError(
                f"expected exactly one {label} test executable from Cargo, got {sorted(executables)}"
            )
        selected[label] = executables.pop()
    return selected


def read_messages(path):
    with open(path, encoding="utf-8") as messages:
        for line_number, line in enumerate(messages, 1):
            if not line.strip():
                continue
            try:
                yield json.loads(line)
            except json.JSONDecodeError as error:
                raise ValueError(
                    f"invalid Cargo JSON message on line {line_number}: {error}"
                ) from error


def main():
    if len(sys.argv) < 4:
        raise ValueError(
            f"usage: {sys.argv[0]} <cargo-json-messages> <repository-root> <lib|integration> ..."
        )
    selected = select_executables(
        read_messages(sys.argv[1]), os.path.realpath(sys.argv[2]), sys.argv[3:]
    )
    for label, executable in selected.items():
        if not os.path.isfile(executable) or not os.access(executable, os.X_OK):
            raise ValueError(
                f"Cargo emitted a missing or non-executable {label} test artifact: {executable}"
            )
        print(f"{label}\t{executable}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
