#!/usr/bin/env python3
"""
End-to-End MCP Server Test Suite
Tests both HTTP and stdio transport modes comprehensively
"""

import json
import subprocess
import sys
import os
import time
import requests
import threading
from colorama import init, Fore, Style

init(autoreset=True)

GOLEM_CLI = r"target\release\golem-cli.exe" if os.name == 'nt' else "target/release/golem-cli"
MCP_URL = "http://127.0.0.1:3000/mcp"
BASE_URL = "http://127.0.0.1:3000"

def print_success(msg):
    print(f"{Fore.GREEN}[PASS] {msg}{Style.RESET_ALL}")

def print_error(msg):
    print(f"{Fore.RED}[FAIL] {msg}{Style.RESET_ALL}")

def print_info(msg):
    print(f"{Fore.CYAN}{msg}{Style.RESET_ALL}")

def print_warning(msg):
    print(f"{Fore.YELLOW}[WARN] {msg}{Style.RESET_ALL}")

class TestResult:
    def __init__(self):
        self.passed = 0
        self.failed = 0
        self.warnings = 0
        
    def add_pass(self):
        self.passed += 1
        
    def add_fail(self):
        self.failed += 1
        
    def add_warning(self):
        self.warnings += 1
        
    def summary(self):
        total = self.passed + self.failed
        print_info("\n" + "="*60)
        print_info("TEST SUMMARY")
        print_info("="*60)
        print_success(f"Passed: {self.passed}")
        if self.failed > 0:
            print_error(f"Failed: {self.failed}")
        if self.warnings > 0:
            print_warning(f"Warnings: {self.warnings}")
        print_info(f"Total: {total}")
        print_info("="*60)
        return self.failed == 0

# HTTP Transport Tests
def test_http_transport():
    """Test HTTP transport mode"""
    result = TestResult()
    print_info("\n" + "="*60)
    print_info("E2E TEST: HTTP Transport Mode")
    print_info("="*60)
    
    # Start HTTP server
    print_info("\nStarting HTTP MCP server...")
    server_process = subprocess.Popen(
        [GOLEM_CLI, "mcp-server", "start", "--host", "127.0.0.1", "--port", "3000"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    
    try:
        # Wait for server to start
        for _ in range(20):
            try:
                resp = requests.get(BASE_URL, timeout=0.1)
                if resp.status_code == 200:
                    break
            except:
                time.sleep(0.1)
        else:
            print_error("Server did not start")
            result.add_fail()
            return result
        
        print_success("Server started")
        result.add_pass()
        
        # Test health endpoint
        print_info("\n1. Testing health endpoint...")
        try:
            resp = requests.get(BASE_URL, timeout=2)
            if resp.status_code == 200:
                print_success("Health endpoint: OK")
                result.add_pass()
            else:
                print_error(f"Health endpoint: Failed with status {resp.status_code}")
                result.add_fail()
        except Exception as e:
            print_error(f"Health endpoint: Exception - {e}")
            result.add_fail()
        
        # Test MCP protocol
        session = requests.Session()
        
        # Initialize
        print_info("\n2. Testing initialize...")
        init_req = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "e2e-test", "version": "1.0"}
            }
        }
        
        try:
            resp = session.post(
                MCP_URL,
                json=init_req,
                headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
                timeout=5
            )
            
            # Parse SSE response
            for line in resp.text.split('\n'):
                if line.startswith('data: '):
                    json_str = line[6:].strip()
                    resp_data = json.loads(json_str)
                    if "result" in resp_data:
                        print_success("Initialize: SUCCESS")
                        result.add_pass()
                    else:
                        print_error(f"Initialize: FAILED - {resp_data}")
                        result.add_fail()
                    break
        except Exception as e:
            print_error(f"Initialize: Exception - {e}")
            result.add_fail()
        
        # Send initialized notification
        notify_req = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }
        try:
            session.post(MCP_URL, json=notify_req, headers={"Content-Type": "application/json"}, timeout=2)
        except:
            pass
        
        # List tools
        print_info("\n3. Testing tools/list...")
        list_req = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        
        try:
            resp = session.post(
                MCP_URL,
                json=list_req,
                headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
                timeout=5
            )
            
            for line in resp.text.split('\n'):
                if line.startswith('data: '):
                    json_str = line[6:].strip()
                    resp_data = json.loads(json_str)
                    if "result" in resp_data and "tools" in resp_data["result"]:
                        tools = resp_data["result"]["tools"]
                        print_success(f"Tools/list: SUCCESS - {len(tools)} tools")
                        result.add_pass()
                    else:
                        print_error(f"Tools/list: FAILED - {resp_data}")
                        result.add_fail()
                    break
        except Exception as e:
            print_error(f"Tools/list: Exception - {e}")
            result.add_fail()
        
    finally:
        server_process.terminate()
        server_process.wait()
    
    return result

# Stdio Transport Tests
def test_stdio_transport():
    """Test stdio transport mode"""
    result = TestResult()
    print_info("\n" + "="*60)
    print_info("E2E TEST: Stdio Transport Mode")
    print_info("="*60)
    
    print_info("\nStarting stdio MCP server...")
    process = subprocess.Popen(
        [GOLEM_CLI, "mcp-server", "start", "--transport", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=0
    )
    
    try:
        # Initialize
        print_info("\n1. Testing initialize...")
        init_req = json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "e2e-test", "version": "1.0"}
            }
        })
        
        process.stdin.write(init_req + "\n")
        process.stdin.flush()
        
        response_line = process.stdout.readline()
        if response_line:
            resp = json.loads(response_line.strip())
            if "result" in resp:
                print_success("Initialize: SUCCESS")
                result.add_pass()
            else:
                print_error(f"Initialize: FAILED - {resp}")
                result.add_fail()
        else:
            print_error("Initialize: No response")
            result.add_fail()
        
        # Send initialized notification
        notify_req = json.dumps({
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        })
        process.stdin.write(notify_req + "\n")
        process.stdin.flush()
        
        # List tools
        print_info("\n2. Testing tools/list...")
        list_req = json.dumps({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        })
        
        process.stdin.write(list_req + "\n")
        process.stdin.flush()
        
        response_line = process.stdout.readline()
        if response_line:
            resp = json.loads(response_line.strip())
            if "result" in resp and "tools" in resp["result"]:
                tools = resp["result"]["tools"]
                print_success(f"Tools/list: SUCCESS - {len(tools)} tools")
                result.add_pass()
            else:
                print_error(f"Tools/list: FAILED - {resp}")
                result.add_fail()
        else:
            print_error("Tools/list: No response")
            result.add_fail()
        
    except Exception as e:
        print_error(f"Stdio test failed: {e}")
        result.add_fail()
    finally:
        process.terminate()
        process.wait()
    
    return result

def main():
    print_info("="*60)
    print_info("GOLEM CLI MCP SERVER - E2E TEST SUITE")
    print_info("="*60)
    
    if not os.path.exists(GOLEM_CLI):
        print_error(f"golem-cli not found at {GOLEM_CLI}")
        print_warning("Please build the project first: cargo build --package golem-cli")
        sys.exit(1)
    
    # Run HTTP tests
    http_result = test_http_transport()
    
    # Run stdio tests
    stdio_result = test_stdio_transport()
    
    # Combined summary
    total_result = TestResult()
    total_result.passed = http_result.passed + stdio_result.passed
    total_result.failed = http_result.failed + stdio_result.failed
    total_result.warnings = http_result.warnings + stdio_result.warnings
    
    success = total_result.summary()
    sys.exit(0 if success else 1)

if __name__ == "__main__":
    main()
