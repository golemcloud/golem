#!/usr/bin/env python3
"""Run MCP integration tests (simple version)."""

import os
import subprocess
import sys
from pathlib import Path

project_dir = Path(r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem")
os.chdir(project_dir)

cargo_path = Path(r"C:\Users\matias.magni2\.cargo\bin\cargo.exe")

env = os.environ.copy()
env["RUST_BACKTRACE"] = "1"

result = subprocess.run(
    [
        str(cargo_path),
        "test",
        "--package", "golem-cli",
        "--test", "mcp_integration",
        "--",
        "--nocapture",
        "--test-threads=1"
    ],
    env=env,
    cwd=project_dir
)

sys.exit(result.returncode)
