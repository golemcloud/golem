#!/usr/bin/env python3
"""Run all MCP integration tests."""

import os
import subprocess
import sys
from pathlib import Path
from colorama import init, Fore, Style

init(autoreset=True)

project_dir = Path(r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem")
os.chdir(project_dir)

cargo_path = Path(r"C:\Users\matias.magni2\.cargo\bin\cargo.exe")

print(f"{Fore.GREEN}=== Running MCP Integration Tests ===")

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
    cwd=project_dir
)

exit_code = result.returncode
color = Fore.GREEN if exit_code == 0 else Fore.RED
print(f"\n=== Tests completed with exit code: {exit_code} ===")
sys.exit(exit_code)
