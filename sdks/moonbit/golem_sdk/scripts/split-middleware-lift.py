#!/usr/bin/env python3
"""Validate the pinned generator's middleware argument lift.

This checked pass only accepts the exact shape emitted by the
repository-pinned wit-bindgen revision.
"""

import hashlib
import sys
from pathlib import Path


EXPECTED_INVOKE_LIFT_SHA256 = (
    "0fa6709353a45af0ff2a14c0307b28b33ab7dce47dcaba929bb344ea169ac57f"
)


def main() -> None:
    if len(sys.argv) != 2:
        raise SystemExit("usage: split-middleware-lift.py <ffi.mbt>")
    path = Path(sys.argv[1])
    lines = path.read_text().splitlines()
    signature = "pub fn wasmExportInvokeToolMiddleware(p0 : Int) -> Int {"
    if lines.count(signature) != 1:
        raise SystemExit("expected exactly one invoke-tool-middleware binding")
    signature_index = lines.index(signature)
    raw_start = signature_index - 1
    task_return_prefix = "fn wasmExportAsyncInvokeToolMiddlewareTaskReturn("
    task_return_indices = [
        index for index, line in enumerate(lines) if line.startswith(task_return_prefix)
    ]
    if len(task_return_indices) != 1:
        raise SystemExit("expected exactly one invoke-tool-middleware task return")
    task_return_index = task_return_indices[0]
    raw_end = task_return_index - 1
    if raw_start < 0 or lines[raw_start] != "#doc(hidden)":
        raise SystemExit("unexpected truncated invoke-tool-middleware binding")
    if raw_end <= raw_start or lines[raw_end] != "///|":
        raise SystemExit("unexpected invoke-tool-middleware binding end")
    raw_lift = "\n".join(lines[raw_start:raw_end]) + "\n"
    digest = hashlib.sha256(raw_lift.encode()).hexdigest()
    if digest != EXPECTED_INVOKE_LIFT_SHA256:
        raise SystemExit(
            "unexpected invoke-tool-middleware lift digest: "
            f"expected {EXPECTED_INVOKE_LIFT_SHA256}, got {digest}"
        )
    path.write_text("\n".join(lines) + "\n")


if __name__ == "__main__":
    main()
