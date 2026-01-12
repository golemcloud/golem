#!/usr/bin/env python3
"""Run MCP integration tests."""

import os
import subprocess
import sys
from pathlib import Path
from colorama import init, Fore, Style

init(autoreset=True)

# Change to project directory
project_dir = Path(r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem")
os.chdir(project_dir)

print(f"{Fore.CYAN}{'=' * 40}")
print(f"{Fore.CYAN}Running MCP Integration Tests")
print(f"{Fore.CYAN}{'=' * 40}")
print()

# Kill any existing processes
print(f"{Fore.YELLOW}Cleaning up existing processes...")
try:
    subprocess.run(
        ["taskkill", "/F", "/IM", "golem-cli.exe"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False
    )
except:
    pass
import time
time.sleep(2)

print(f"{Fore.YELLOW}Starting tests...")
print()

# Set environment variable
env = os.environ.copy()
env["RUST_BACKTRACE"] = "1"

# Find cargo
cargo_path = Path(r"C:\Users\matias.magni2\.cargo\bin\cargo.exe")

# Run tests
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

print()
print(f"{Fore.YELLOW}Cleaning up...")
try:
    subprocess.run(
        ["taskkill", "/F", "/IM", "golem-cli.exe"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False
    )
except:
    pass

print()
if result.returncode == 0:
    print(f"{Fore.GREEN}{'=' * 40}")
    print(f"{Fore.GREEN}[PASS] ALL TESTS PASSED!")
    print(f"{Fore.GREEN}{'=' * 40}")
    sys.exit(0)
else:
    print(f"{Fore.RED}{'=' * 40}")
    print(f"{Fore.RED}[FAIL] TESTS FAILED")
    print(f"{Fore.RED}{'=' * 40}")
    sys.exit(result.returncode)
