#!/usr/bin/env python3

import json
import os
import sys


TARGETS = {
    "lib": ("golem_worker_executor", ["lib"], "golem-worker-executor/src/lib.rs"),
    "integration": ("integration", ["test"], "golem-worker-executor-tests/tests/lib.rs"),
}


def select_executables(messages, repo_root, requested):
    if not requested or len(set(requested)) != len(requested):
        raise ValueError("request each test target exactly once")
    if any(label not in TARGETS for label in requested):
        raise ValueError(f"unknown test target; expected one of {sorted(TARGETS)}")
    expected_sources = {
        label: os.path.join(repo_root, TARGETS[label][2]) for label in requested
    }
    artifacts = [
        (message.get("target", {}), message.get("executable"))
        for message in messages
        if message.get("reason") == "compiler-artifact"
        and message.get("profile", {}).get("test")
    ]
    matches = {
        label: {
            executable
            for target, executable in artifacts
            if executable is not None
            and target.get("name") == TARGETS[label][0]
            and target.get("kind") == TARGETS[label][1]
            and os.path.realpath(target.get("src_path", "")) == expected_sources[label]
        }
        for label in requested
    }
    unmatched = next(
        (
            (label, executables)
            for label, executables in matches.items()
            if len(executables) != 1
        ),
        None,
    )
    if unmatched is not None:
        label, executables = unmatched
        raise ValueError(
            f"expected exactly one {label} test executable from Cargo, got {sorted(executables)}"
        )
    return {label: next(iter(executables)) for label, executables in matches.items()}


def parse_message(line_number, line):
    try:
        return json.loads(line)
    except json.JSONDecodeError as error:
        raise ValueError(
            f"invalid Cargo JSON message on line {line_number}: {error}"
        ) from error


def read_messages(path):
    with open(path, encoding="utf-8") as messages:
        yield from (
            parse_message(line_number, line)
            for line_number, line in enumerate(messages, 1)
            if line.strip()
        )


def main():
    if len(sys.argv) < 4:
        raise ValueError(
            f"usage: {sys.argv[0]} <cargo-json-messages> <repository-root> <lib|integration> ..."
        )
    selected = select_executables(
        read_messages(sys.argv[1]), os.path.realpath(sys.argv[2]), sys.argv[3:]
    )
    unusable = next(
        (
            (label, executable)
            for label, executable in selected.items()
            if not os.path.isfile(executable) or not os.access(executable, os.X_OK)
        ),
        None,
    )
    if unusable is not None:
        label, executable = unusable
        raise ValueError(
            f"Cargo emitted a missing or non-executable {label} test artifact: {executable}"
        )
    print("\n".join(f"{label}\t{executable}" for label, executable in selected.items()))


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError) as error:
        print(error, file=sys.stderr)
        raise SystemExit(1)
