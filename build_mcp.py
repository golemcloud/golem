#!/usr/bin/env python3
"""Build Golem CLI with MCP support."""

import os
import subprocess
import sys
from pathlib import Path
from colorama import init, Fore, Style

init(autoreset=True)

project_dir = Path(r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem")
os.chdir(project_dir)

cargo_path = Path(r"C:\Users\matias.magni2\.cargo\bin\cargo.exe")

print(f"{Fore.CYAN}{'=' * 40}")
print(f"{Fore.CYAN}Building Golem CLI with MCP support")
print(f"{Fore.CYAN}{'=' * 40}")

result = subprocess.run(
    [str(cargo_path), "build", "--package", "golem-cli"],
    cwd=project_dir
)

if result.returncode != 0:
    print(f"{Fore.RED}Build failed!")
    sys.exit(result.returncode)

print(f"{Fore.GREEN}Build completed successfully!")
