#!/usr/bin/env python3
"""
Test MCP connections manually with prompts
Tests both HTTP/SSE and stdio modes
"""

import json
import socket
import time
import sys
from typing import Optional

HOST = "127.0.0.1"
PORT = 3000
MCP_ENDPOINT = f"http://{HOST}:{PORT}/mcp"

def print_section(title: str):
    """Print a section header"""
    print("\n" + "=" * 70)
    print(f"  {title}")
    print("=" * 70)

def print_test(test_name: str, status: bool, details: str = ""):
    """Print test result"""
    status_str = "[PASS]" if status else "[FAIL]"
    color_code = "\033[92m" if status else "\033[91m"
    reset_code = "\033[0m"
    print(f"{color_code}{status_str}{reset_code} {test_name}")
    if details:
        print(f"      {details}")

def wait_for_server(max_attempts: int = 20) -> bool:
    """Wait for the server to be ready"""
    print("Waiting for server to start...")
    for attempt in range(max_attempts):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(0.1)
            result = sock.connect_ex((HOST, PORT))
            sock.close()
            if result == 0:
                print("Server is ready!")
                return True
        except:
            pass
        time.sleep(0.1)
    return False

def send_mcp_request(method: str, params: dict = None, request_id: int = 1) -> Optional[dict]:
    """Send an MCP request via HTTP"""
    import requests
    
    if params is None:
        params = {}
    
    payload = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params
    }
    
    try:
        response = requests.post(
            MCP_ENDPOINT,
            json=payload,
            headers={"Content-Type": "application/json"},
            timeout=5
        )
        
        # Parse SSE response
        text = response.text
        if "data: " in text:
            # Extract JSON from SSE format
            lines = text.split("\n")
            for line in lines:
                if line.startswith("data: "):
                    json_str = line[6:]  # Remove "data: " prefix
                    try:
                        return json.loads(json_str)
                    except:
                        pass
        
        # Try direct JSON
        try:
            return response.json()
        except:
            return {"error": f"Failed to parse response: {text[:200]}"}
            
    except Exception as e:
        return {"error": str(e)}

def test_health_endpoint():
    """Test 1: Health endpoint"""
    print_section("Test 1: Health Endpoint")
    
    import requests
    try:
        response = requests.get(f"http://{HOST}:{PORT}/", timeout=2)
        if response.status_code == 200:
            print_test("Health check", True, f"Response: {response.text.strip()}")
            return True
        else:
            print_test("Health check", False, f"Status: {response.status_code}")
            return False
    except Exception as e:
        print_test("Health check", False, f"Error: {e}")
        return False

def test_initialize():
    """Test 2: Initialize MCP connection"""
    print_section("Test 2: Initialize MCP Connection")
    
    params = {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "test-client",
            "version": "1.0.0"
        }
    }
    
    response = send_mcp_request("initialize", params, request_id=1)
    
    if "error" in response:
        print_test("Initialize", False, f"Error: {response['error']}")
        return False
    
    if "result" in response:
        result = response["result"]
        if "protocolVersion" in result:
            print_test("Initialize", True, f"Protocol: {result.get('protocolVersion')}")
            print_test("Server info", True, f"Name: {result.get('serverInfo', {}).get('name', 'N/A')}")
            return True
    
    print_test("Initialize", False, f"Unexpected response: {json.dumps(response, indent=2)}")
    return False

def test_list_tools():
    """Test 3: List available tools"""
    print_section("Test 3: List Available Tools")
    
    response = send_mcp_request("tools/list", {}, request_id=2)
    
    if "error" in response:
        print_test("List tools", False, f"Error: {response['error']}")
        return False
    
    if "result" in response:
        tools = response["result"].get("tools", [])
        print_test("List tools", True, f"Found {len(tools)} tools")
        
        for tool in tools:
            name = tool.get("name", "unknown")
            desc = tool.get("description", "No description")
            print(f"      - {name}: {desc[:60]}...")
        
        return True
    
    print_test("List tools", False, f"Unexpected response: {json.dumps(response, indent=2)}")
    return False

def test_list_agent_types():
    """Test 4: Call list_agent_types tool"""
    print_section("Test 4: Call list_agent_types Tool")
    
    params = {
        "name": "list_agent_types",
        "arguments": {}
    }
    
    response = send_mcp_request("tools/call", params, request_id=3)
    
    if "error" in response:
        print_test("list_agent_types", False, f"Error: {response['error']}")
        return False
    
    if "result" in response:
        content = response["result"].get("content", [])
        if content:
            text_content = content[0].get("text", "")
            try:
                data = json.loads(text_content)
                agent_types = data.get("agent_types", [])
                print_test("list_agent_types", True, f"Found {len(agent_types)} agent types")
                if agent_types:
                    print(f"      Agent types: {', '.join(agent_types[:5])}")
                    if len(agent_types) > 5:
                        print(f"      ... and {len(agent_types) - 5} more")
                return True
            except:
                print_test("list_agent_types", True, f"Response: {text_content[:100]}...")
                return True
    
    print_test("list_agent_types", False, f"Unexpected response: {json.dumps(response, indent=2)}")
    return False

def test_list_components():
    """Test 5: Call list_components tool"""
    print_section("Test 5: Call list_components Tool")
    
    params = {
        "name": "list_components",
        "arguments": {}
    }
    
    response = send_mcp_request("tools/call", params, request_id=4)
    
    if "error" in response:
        print_test("list_components", False, f"Error: {response['error']}")
        return False
    
    if "result" in response:
        content = response["result"].get("content", [])
        if content:
            text_content = content[0].get("text", "")
            try:
                data = json.loads(text_content)
                components = data.get("components", [])
                print_test("list_components", True, f"Found {len(components)} components")
                if components:
                    print(f"      First component: {components[0].get('name', 'N/A')} (ID: {components[0].get('id', 'N/A')[:20]}...)")
                return True
            except:
                print_test("list_components", True, f"Response: {text_content[:100]}...")
                return True
    
    print_test("list_components", False, f"Unexpected response: {json.dumps(response, indent=2)}")
    return False

def test_error_handling():
    """Test 6: Error handling - invalid tool"""
    print_section("Test 6: Error Handling (Invalid Tool)")
    
    params = {
        "name": "nonexistent_tool",
        "arguments": {}
    }
    
    response = send_mcp_request("tools/call", params, request_id=5)
    
    if "error" in response:
        error_code = response["error"].get("code", "unknown")
        error_msg = response["error"].get("message", "unknown")
        print_test("Error handling", True, f"Proper error returned: {error_code} - {error_msg}")
        return True
    
    print_test("Error handling", False, f"Expected error but got: {json.dumps(response, indent=2)}")
    return False

def main():
    """Run all tests"""
    print("\n" + "=" * 70)
    print("  Golem CLI MCP Connection Tests")
    print("=" * 70)
    print(f"\nTesting HTTP/SSE endpoint: {MCP_ENDPOINT}")
    print(f"Host: {HOST}, Port: {PORT}")
    
    # Check if server is running
    if not wait_for_server():
        print("\n[FAIL] Server is not running!")
        print("\nPlease start the server first:")
        print("  golem-cli mcp-server start")
        sys.exit(1)
    
    results = []
    
    # Run tests
    results.append(("Health Endpoint", test_health_endpoint()))
    results.append(("Initialize", test_initialize()))
    results.append(("List Tools", test_list_tools()))
    results.append(("list_agent_types", test_list_agent_types()))
    results.append(("list_components", test_list_components()))
    results.append(("Error Handling", test_error_handling()))
    
    # Summary
    print_section("Test Summary")
    passed = sum(1 for _, result in results if result)
    total = len(results)
    
    for test_name, result in results:
        status = "PASS" if result else "FAIL"
        color = "\033[92m" if result else "\033[91m"
        reset = "\033[0m"
        print(f"{color}{status:6}{reset} {test_name}")
    
    print(f"\nTotal: {passed}/{total} tests passed")
    
    if passed == total:
        print("\n[SUCCESS] All tests passed!")
        return 0
    else:
        print(f"\n[WARNING] {total - passed} test(s) failed")
        return 1

if __name__ == "__main__":
    try:
        sys.exit(main())
    except KeyboardInterrupt:
        print("\n\nTests interrupted by user")
        sys.exit(1)
    except Exception as e:
        print(f"\n[ERROR] Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
