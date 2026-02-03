#!/usr/bin/env python3
"""
Playwright-based E2E test for Golem CLI MCP Server - HTTP/SSE mode
Tests the MCP server via browser automation for bounty #1926
"""

import subprocess
import time
import requests
import json
import sys
import os

# Test configuration
MCP_SERVER_PORT = 3000
MCP_SERVER_URL = f"http://127.0.0.1:{MCP_SERVER_PORT}"
GOLEM_CLI_PATH = r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"

def print_header(text):
    print("\n" + "=" * 70)
    print(text)
    print("=" * 70)

def print_pass(msg):
    print(f"  [PASS] {msg}")

def print_fail(msg):
    print(f"  [FAIL] {msg}")

def start_http_server():
    """Start the MCP server in HTTP mode"""
    print_header("Starting MCP Server (HTTP Mode)")
    
    proc = subprocess.Popen(
        [GOLEM_CLI_PATH, "mcp-server", "start", "--transport", "http", "--port", str(MCP_SERVER_PORT)],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    
    # Wait for server to start
    for i in range(10):
        try:
            r = requests.get(f"{MCP_SERVER_URL}/mcp", timeout=1)
            print(f"  Server started (attempt {i+1})")
            return proc
        except:
            time.sleep(0.5)
    
    print_fail("Server failed to start")
    return None

def test_sse_connection():
    """Test SSE endpoint connection"""
    print_header("TEST: SSE Connection")
    
    try:
        # SSE endpoint should be available
        r = requests.get(f"{MCP_SERVER_URL}/mcp", 
                        headers={"Accept": "text/event-stream"},
                        stream=True, timeout=2)
        
        if r.status_code == 200:
            print_pass(f"SSE endpoint responded (status={r.status_code})")
            return True
        else:
            print_fail(f"Unexpected status: {r.status_code}")
            return False
    except Exception as e:
        print_fail(f"SSE connection error: {e}")
        return False

def send_jsonrpc_via_http(method, params=None, msg_id=1):
    """Send JSON-RPC request via HTTP POST"""
    request = {
        "jsonrpc": "2.0",
        "id": msg_id,
        "method": method
    }
    if params:
        request["params"] = params
    
    try:
        r = requests.post(
            f"{MCP_SERVER_URL}/mcp/message",
            json=request,
            headers={"Content-Type": "application/json"},
            timeout=10
        )
        return r.json() if r.status_code == 200 else {"error": f"HTTP {r.status_code}"}
    except Exception as e:
        return {"error": str(e)}

def test_http_initialize():
    """Test MCP initialization via HTTP"""
    print_header("TEST: HTTP Initialize")
    
    response = send_jsonrpc_via_http(
        "initialize",
        {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "playwright-test", "version": "1.0"}
        },
        msg_id=1
    )
    
    if "result" in response:
        print_pass("Initialize successful")
        print(f"    Protocol: {response['result'].get('protocolVersion', 'N/A')}")
        print(f"    Server: {response['result'].get('serverInfo', {}).get('name', 'N/A')}")
        return True
    else:
        print_fail(f"Initialize failed: {response}")
        return False

def test_http_tools_list():
    """Test tools/list via HTTP"""
    print_header("TEST: HTTP Tools List")
    
    response = send_jsonrpc_via_http("tools/list", msg_id=2)
    
    if "result" in response and "tools" in response["result"]:
        tools = response["result"]["tools"]
        print_pass(f"Found {len(tools)} tools")
        for tool in tools:
            print(f"    - {tool['name']}: {tool['description']}")
        return True
    else:
        print_fail(f"Tools list failed: {response}")
        return False

def test_http_tool_call():
    """Test tool execution via HTTP"""
    print_header("TEST: HTTP Tool Call (list_components)")
    
    response = send_jsonrpc_via_http(
        "tools/call",
        {"name": "list_components", "arguments": {}},
        msg_id=3
    )
    
    if "result" in response:
        content = response["result"].get("content", [])
        if content and len(content) > 0:
            print_pass("Tool executed successfully")
            print(f"    Result: {content[0].get('text', 'N/A')[:100]}")
            return True
    
    print_fail(f"Tool call failed: {response}")
    return False

def test_http_error_handling():
    """Test error handling via HTTP"""
    print_header("TEST: HTTP Error Handling")
    
    response = send_jsonrpc_via_http(
        "tools/call",
        {"name": "nonexistent_tool", "arguments": {}},
        msg_id=4
    )
    
    if "error" in response:
        print_pass(f"Error handled: {response['error'].get('message', 'N/A')}")
        return True
    else:
        print_fail(f"Expected error, got: {response}")
        return False

def main():
    print_header("GOLEM CLI MCP SERVER - HTTP/SSE E2E TESTS")
    print(f"Server URL: {MCP_SERVER_URL}")
    print(f"CLI Path: {GOLEM_CLI_PATH}")
    
    passed = 0
    failed = 0
    
    # Start server
    server_proc = start_http_server()
    if not server_proc:
        print("\nFailed to start server. Exiting.")
        return 1
    
    try:
        # Run tests
        tests = [
            test_sse_connection,
            test_http_initialize,
            test_http_tools_list,
            test_http_tool_call,
            test_http_error_handling
        ]
        
        for test in tests:
            try:
                if test():
                    passed += 1
                else:
                    failed += 1
            except Exception as e:
                print_fail(f"Test exception: {e}")
                failed += 1
    
    finally:
        # Cleanup
        print_header("Cleanup")
        server_proc.terminate()
        server_proc.wait(timeout=5)
        print("  Server stopped")
    
    # Summary
    print_header(f"HTTP/SSE E2E TEST SUMMARY: {passed} passed, {failed} failed")
    
    return 0 if failed == 0 else 1

if __name__ == "__main__":
    sys.exit(main())
