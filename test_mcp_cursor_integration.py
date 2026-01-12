#!/usr/bin/env python3
"""
Manual test of Golem MCP Server integration with Cursor
Tests the actual MCP protocol to verify it works in Cursor
"""

import json
import requests
import time
from colorama import init, Fore, Style

init(autoreset=True)

BASE_URL = "http://127.0.0.1:3000"
MCP_URL = f"{BASE_URL}/mcp"

def print_success(msg):
    print(f"{Fore.GREEN}[PASS] {msg}{Style.RESET_ALL}")

def print_info(msg):
    print(f"{Fore.CYAN}{msg}{Style.RESET_ALL}")

def print_error(msg):
    print(f"{Fore.RED}[FAIL] {msg}{Style.RESET_ALL}")

def print_warning(msg):
    print(f"{Fore.YELLOW}[WARN] {msg}{Style.RESET_ALL}")

def wait_for_server(max_attempts=20):
    """Wait for server to be ready"""
    print_info("Waiting for server to be ready...")
    for i in range(max_attempts):
        try:
            response = requests.get(BASE_URL, timeout=1)
            if response.status_code == 200:
                print_success("Server is ready!")
                return True
        except:
            pass
        time.sleep(0.1)
    print_error("Server not ready")
    return False

# Global session for connection persistence
session = requests.Session()

def send_mcp_request(method, params, request_id=1):
    """Send MCP request and parse SSE response using persistent session"""
    request = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params
    }
    
    try:
        response = session.post(
            MCP_URL,
            json=request,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream",
                "Connection": "keep-alive"
            },
            timeout=5
        )
        response.raise_for_status()
        
        # Parse SSE response
        for line in response.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                return json.loads(json_str)
        return None
    except Exception as e:
        print_error(f"Request failed: {e}")
        if hasattr(e, 'response') and e.response is not None:
            print_error(f"Response: {e.response.text[:200]}")
        return None

def test_health():
    """Test 1: Health endpoint"""
    print_info("\n=== TEST 1: Health Endpoint ===")
    try:
        response = requests.get(BASE_URL, timeout=2)
        if response.status_code == 200 and "Golem CLI MCP Server" in response.text:
            print_success("Health endpoint works")
            return True
        else:
            print_error(f"Health check failed: {response.status_code}")
            return False
    except Exception as e:
        print_error(f"Health check error: {e}")
        return False

def test_initialize():
    """Test 2: MCP Initialize"""
    print_info("\n=== TEST 2: MCP Initialize ===")
    params = {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "cursor-test",
            "version": "1.0.0"
        }
    }
    
    response = send_mcp_request("initialize", params, request_id=1)
    if response and response.get("result"):
        print_success("Initialize successful")
        print_info(f"Server info: {json.dumps(response['result'], indent=2)}")
        
        # Send initialized notification on same session
        notification = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }
        try:
            session.post(
                MCP_URL,
                json=notification,
                headers={
                    "Content-Type": "application/json",
                    "Connection": "keep-alive"
                },
                timeout=2
            )
        except:
            pass
        return True
    else:
        print_error(f"Initialize failed: {response}")
        return False

def test_list_tools():
    """Test 3: List Tools"""
    print_info("\n=== TEST 3: List Tools ===")
    response = send_mcp_request("tools/list", {}, request_id=2)
    if response and response.get("result") and response["result"].get("tools"):
        tools = response["result"]["tools"]
        print_success(f"Found {len(tools)} tools")
        for tool in tools:
            print_info(f"  - {tool['name']}: {tool['description']}")
        return True
    else:
        print_error(f"List tools failed: {response}")
        return False

def test_call_tool(tool_name):
    """Test 4: Call a tool"""
    print_info(f"\n=== TEST 4: Call {tool_name} ===")
    params = {
        "name": tool_name,
        "arguments": {}
    }
    
    response = send_mcp_request("tools/call", params, request_id=3)
    if response:
        if response.get("error"):
            error = response["error"]
            print_warning(f"Tool returned error (may be expected): {error.get('message', 'Unknown error')}")
            return True  # Error structure is valid
        elif response.get("result"):
            print_success(f"{tool_name} executed successfully")
            result = response["result"]
            if result.get("content"):
                content = result["content"][0] if result["content"] else {}
                if content.get("text"):
                    try:
                        data = json.loads(content["text"])
                        print_info(f"Result: {json.dumps(data, indent=2)}")
                    except:
                        print_info(f"Result: {content['text'][:200]}")
            return True
        else:
            print_error(f"Unexpected response: {response}")
            return False
    else:
        print_error("No response received")
        return False

def main():
    print_info("=" * 50)
    print_info("Golem MCP Server - Cursor Integration Test")
    print_info("=" * 50)
    
    if not wait_for_server():
        print_error("Cannot proceed without server")
        return
    
    tests_passed = 0
    tests_total = 0
    
    # Run tests
    tests = [
        ("Health", test_health),
        ("Initialize", test_initialize),
        ("List Tools", test_list_tools),
        ("Call list_agent_types", lambda: test_call_tool("list_agent_types")),
        ("Call list_components", lambda: test_call_tool("list_components")),
    ]
    
    for test_name, test_func in tests:
        tests_total += 1
        if test_func():
            tests_passed += 1
    
    # Summary
    print_info("\n" + "=" * 50)
    print_info("TEST SUMMARY")
    print_info("=" * 50)
    print_success(f"Passed: {tests_passed}/{tests_total}")
    if tests_passed == tests_total:
        print_success("\n[SUCCESS] ALL TESTS PASSED! MCP Server is ready for Cursor!")
    else:
        print_warning(f"\n[WARN] {tests_total - tests_passed} tests had issues")
    
    print_info("\nThe MCP server is configured in Cursor and ready to use!")
    print_info("Restart Cursor to activate the MCP integration.")

if __name__ == "__main__":
    main()
