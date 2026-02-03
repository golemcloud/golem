#!/usr/bin/env python3
"""
Comprehensive MCP Test Runner
Runs all MCP tests: unit, integration, E2E, and exploratory
"""

import subprocess
import sys
import os
from colorama import init, Fore, Style

init(autoreset=True)

def print_success(msg):
    print(f"{Fore.GREEN}[PASS] {msg}{Style.RESET_ALL}")

def print_error(msg):
    print(f"{Fore.RED}[FAIL] {msg}{Style.RESET_ALL}")

def print_info(msg):
    print(f"{Fore.CYAN}[INFO] {msg}{Style.RESET_ALL}")

def print_section(msg):
    print(f"\n{Fore.MAGENTA}{'='*60}{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}{msg}{Style.RESET_ALL}")
    print(f"{Fore.MAGENTA}{'='*60}{Style.RESET_ALL}\n")

def run_command(cmd, description):
    """Run a command and return success status"""
    print_info(f"Running: {description}")
    print_info(f"Command: {' '.join(cmd)}")
    
    try:
        result = subprocess.run(
            cmd,
            capture_output=True,
            text=True,
            timeout=300  # 5 minute timeout
        )
        
        if result.returncode == 0:
            print_success(f"{description} - PASSED")
            if result.stdout:
                print(result.stdout)
            return True
        else:
            print_error(f"{description} - FAILED (exit code: {result.returncode})")
            if result.stderr:
                print(result.stderr)
            if result.stdout:
                print(result.stdout)
            return False
    except subprocess.TimeoutExpired:
        print_error(f"{description} - TIMEOUT")
        return False
    except Exception as e:
        print_error(f"{description} - ERROR: {e}")
        return False

def main():
    print_section("GOLEM CLI MCP SERVER - COMPREHENSIVE TEST SUITE")
    
    results = {
        "unit": False,
        "integration_http": False,
        "integration_stdio": False,
        "e2e": False,
        "stdio_manual": False,
        "playwright": False,
    }
    
    # Check if we're in the right directory
    if not os.path.exists("cli/golem-cli"):
        print_error("Must run from project root directory")
        sys.exit(1)
    
    # 1. Unit Tests
    print_section("1. UNIT TESTS")
    print_info("Running Rust unit tests for MCP server (mcp_server_unit)...")
    results["unit"] = run_command(
        ["cargo", "test", "--package", "golem-cli", "--test", "mcp_server_unit", "--", "--nocapture"],
        "Unit Tests"
    )
    
    # 2. Integration Tests - HTTP
    print_section("2. INTEGRATION TESTS - HTTP MODE")
    print_info("Running Rust integration tests for HTTP transport...")
    results["integration_http"] = run_command(
        ["cargo", "test", "--package", "golem-cli", "--test", "mcp_integration_test", "--", "--test-threads=1"],
        "HTTP Integration Tests"
    )
    
    # 3. Integration Tests - Stdio
    print_section("3. INTEGRATION TESTS - STDIO MODE")
    print_info("Running Rust integration tests for stdio transport...")
    results["integration_stdio"] = run_command(
        ["cargo", "test", "--package", "golem-cli", "--test", "mcp_stdio_integration", "--", "--test-threads=1"],
        "Stdio Integration Tests"
    )
    
    # 4. E2E Tests
    print_section("4. END-TO-END TESTS")
    print_info("Running Python E2E tests for both transport modes...")
    if os.path.exists("test_mcp_e2e.py"):
        results["e2e"] = run_command(
            [sys.executable, "test_mcp_e2e.py"],
            "E2E Tests"
        )
    else:
        print_error("test_mcp_e2e.py not found")
    
    # 5. Stdio Manual Test
    print_section("5. STDIO MANUAL TEST")
    print_info("Running Python stdio transport test...")
    if os.path.exists("test_mcp_stdio.py"):
        results["stdio_manual"] = run_command(
            [sys.executable, "test_mcp_stdio.py"],
            "Stdio Manual Test"
        )
    else:
        print_error("test_mcp_stdio.py not found")
    
    # 6. Playwright Exploratory Test
    print_section("6. PLAYWRIGHT EXPLORATORY TEST")
    print_info("Running Playwright MCP exploratory test...")
    if os.path.exists("test_mcp_playwright.py"):
        results["playwright"] = run_command(
            [sys.executable, "test_mcp_playwright.py"],
            "Playwright Exploratory Test"
        )
    else:
        print_error("test_mcp_playwright.py not found")
    
    # Summary
    print_section("TEST SUMMARY")
    
    total = len(results)
    passed = sum(1 for v in results.values() if v)
    failed = total - passed
    
    for test_name, result in results.items():
        if result:
            print_success(f"{test_name}: PASSED")
        else:
            print_error(f"{test_name}: FAILED")
    
    print(f"\n{Fore.CYAN}Total: {total} | Passed: {passed} | Failed: {failed}{Style.RESET_ALL}")
    
    if failed == 0:
        print_success("\nALL TESTS PASSED!")
        sys.exit(0)
    else:
        print_error(f"\n{failed} TEST(S) FAILED")
        sys.exit(1)

if __name__ == "__main__":
    main()
