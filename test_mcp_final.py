#!/usr/bin/env python3
"""Final MCP integration test runner."""

import os
import subprocess
import sys
import time
from pathlib import Path
from colorama import init, Fore, Style

init(autoreset=True)

project_dir = Path(r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem")
os.chdir(project_dir)

cargo_path = Path(r"C:\Users\matias.magni2\.cargo\bin\cargo.exe")

print(f"{Fore.CYAN}{'=' * 40}")
print(f"{Fore.CYAN}Running MCP Integration Tests")
print(f"{Fore.CYAN}{'=' * 40}")

# Kill any existing processes
print(f"{Fore.YELLOW}Killing any existing golem-cli processes...")
try:
    subprocess.run(
        ["taskkill", "/F", "/IM", "golem-cli.exe"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False
    )
except:
    pass
time.sleep(1)

print()
print(f"{Fore.YELLOW}Starting tests...")
print()

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

test_result = result.returncode

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
if test_result == 0:
    print(f"{Fore.GREEN}{'=' * 40}")
    print(f"{Fore.GREEN}[PASS] ALL TESTS PASSED!")
    print(f"{Fore.GREEN}{'=' * 40}")
    sys.exit(0)
else:
    print(f"{Fore.RED}{'=' * 40}")
    print(f"{Fore.RED}[FAIL] TESTS FAILED")
    print(f"{Fore.RED}{'=' * 40}")
    sys.exit(test_result)
