#!/usr/bin/env python3
"""Comprehensive MCP Server Integration Test Suite."""

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
print(f"{Fore.CYAN}MCP Server Integration Test Suite")
print(f"{Fore.CYAN}{'=' * 40}")
print()

# Step 1: Kill existing processes
print(f"{Fore.YELLOW}[1/4] Killing any existing golem-cli processes...")
try:
    subprocess.run(
        ["taskkill", "/F", "/IM", "golem-cli.exe"],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        check=False
    )
except:
    pass
time.sleep(2)

# Step 2: Run integration tests
print()
print(f"{Fore.YELLOW}[2/4] Running all MCP integration tests...")
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

if result.returncode != 0:
    print()
    print(f"{Fore.RED}{'=' * 40}")
    print(f"{Fore.RED}[FAIL] TESTS FAILED - Check output above")
    print(f"{Fore.RED}{'=' * 40}")
    sys.exit(result.returncode)

print()
print(f"{Fore.GREEN}{'=' * 40}")
print(f"{Fore.GREEN}[PASS] ALL TESTS PASSED!")
print(f"{Fore.GREEN}{'=' * 40}")

# Step 3: Start MCP server for manual testing
print()
print(f"{Fore.YELLOW}[3/4] Starting MCP server for manual testing...")
binary_path = project_dir / "target" / "debug" / "golem-cli.exe"

if binary_path.exists():
    subprocess.Popen(
        [
            str(binary_path),
            "mcp-server",
            "start",
            "--host", "127.0.0.1",
            "--port", "13337"
        ],
        cwd=project_dir,
        creationflags=subprocess.CREATE_NEW_CONSOLE
    )
    time.sleep(3)
    
    # Step 4: Test endpoints
    print()
    print(f"{Fore.YELLOW}[4/4] Testing MCP server endpoints...")
    print()
    
    import requests
    
    print("Testing health endpoint...")
    try:
        response = requests.get("http://127.0.0.1:13337", timeout=5)
        print(response.text)
    except Exception as e:
        print(f"Error: {e}")
    
    print()
    print("Testing MCP initialize endpoint...")
    try:
        response = requests.post(
            "http://127.0.0.1:13337/mcp",
            json={
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2024-11-05",
                    "capabilities": {},
                    "clientInfo": {
                        "name": "test-client",
                        "version": "1.0.0"
                    }
                }
            },
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=5
        )
        print(response.text[:500])  # Print first 500 chars
    except Exception as e:
        print(f"Error: {e}")
    
    print()
    print("Testing tools/list endpoint...")
    try:
        response = requests.post(
            "http://127.0.0.1:13337/mcp",
            json={
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            },
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=5
        )
        print(response.text[:500])  # Print first 500 chars
    except Exception as e:
        print(f"Error: {e}")
    
    print()
    print(f"{Fore.CYAN}{'=' * 40}")
    print(f"{Fore.CYAN}Manual Testing Complete")
    print(f"{Fore.CYAN}MCP Server is running in background")
    print(f"{Fore.CYAN}Press Ctrl+C to stop the server...")
    print(f"{Fore.CYAN}{'=' * 40}")
    
    try:
        input()  # Wait for user input
    except KeyboardInterrupt:
        pass
    
    print()
    print("Stopping MCP server...")
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
print("Done!")
