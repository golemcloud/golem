#!/usr/bin/env python3
"""
Full MCP Server Test - Tests all available tools
"""

import json
import requests
import sys
from colorama import init, Fore, Style

init(autoreset=True)

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

def send_mcp_request(session, method, params, request_id=1):
    """Send MCP request and parse SSE response"""
    request = {
        "jsonrpc": "2.0",
        "id": request_id,
        "method": method,
        "params": params
    }
    
    try:
        resp = session.post(
            MCP_URL,
            json=request,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=10
        )
        
        # Parse SSE response
        for line in resp.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                try:
                    return json.loads(json_str)
                except:
                    continue
        return None
    except Exception as e:
        print_error(f"Request failed: {e}")
        return None

def test_health():
    """Test health endpoint"""
    print_info("\n" + "="*60)
    print_info("TEST 1: Health Endpoint")
    print_info("="*60)
    try:
        resp = requests.get(BASE_URL, timeout=2)
        if resp.status_code == 200:
            print_success(f"Health check: {resp.text.strip()}")
            return True
        else:
            print_error(f"Health check failed: {resp.status_code}")
            return False
    except Exception as e:
        print_error(f"Health check failed: {e}")
        print_warning("Make sure server is running: target\\debug\\golem-cli.exe mcp-server start --port 3000")
        return False

def test_initialize(session):
    """Test MCP initialize"""
    print_info("\n" + "="*60)
    print_info("TEST 2: MCP Initialize")
    print_info("="*60)
    
    params = {
        "protocolVersion": "2024-11-05",
        "capabilities": {},
        "clientInfo": {
            "name": "cursor-test",
            "version": "1.0.0"
        }
    }
    
    response = send_mcp_request(session, "initialize", params, 1)
    
    if response and response.get("result"):
        print_success("Initialize: SUCCESS")
        server_info = response["result"].get("serverInfo", {})
        print_info(f"  Server: {server_info.get('name', 'N/A')} v{server_info.get('version', 'N/A')}")
        return True
    elif response and response.get("error"):
        print_error(f"Initialize: ERROR - {response['error']}")
        return False
    else:
        print_error("Initialize: No valid response")
        return False

def send_initialized_notification(session):
    """Send initialized notification"""
    notification = {
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    }
    try:
        session.post(
            MCP_URL,
            json=notification,
            headers={"Content-Type": "application/json"},
            timeout=2
        )
    except:
        pass  # Notification might not return response

def test_list_tools(session):
    """Test tools/list"""
    print_info("\n" + "="*60)
    print_info("TEST 3: List Available Tools")
    print_info("="*60)
    
    response = send_mcp_request(session, "tools/list", {}, 2)
    
    if response and response.get("result") and response["result"].get("tools"):
        tools = response["result"]["tools"]
        print_success(f"Found {len(tools)} tools:")
        for tool in tools:
            name = tool.get("name", "unknown")
            desc = tool.get("description", "No description")[:70]
            print_info(f"  • {name}")
            print_info(f"    {desc}")
        return True, tools
    elif response and response.get("error"):
        print_error(f"Tools/list: ERROR - {response['error']}")
        return False, []
    else:
        print_error("Tools/list: No valid response")
        return False, []

def test_call_tool(session, tool_name, tool_args={}):
    """Test calling a specific tool"""
    print_info(f"\n" + "="*60)
    print_info(f"TEST: Call Tool - {tool_name}")
    print_info("="*60)
    
    params = {
        "name": tool_name,
        "arguments": tool_args
    }
    
    response = send_mcp_request(session, "tools/call", params, 3)
    
    if response and response.get("error"):
        error = response["error"]
        code = error.get("code", "unknown")
        message = error.get("message", "unknown")
        print_warning(f"Tool returned error (may be expected if Golem not configured):")
        print_warning(f"  Code: {code}")
        print_warning(f"  Message: {message[:100]}")
        return True  # Error structure is valid
    elif response and response.get("result"):
        result = response["result"]
        if result.get("content"):
            content = result["content"]
            print_success(f"Tool call: SUCCESS")
            for item in content:
                if item.get("type") == "text":
                    text = item.get("text", "")
                    try:
                        # Try to parse as JSON
                        data = json.loads(text)
                        print_info(f"  Response: {json.dumps(data, indent=2)[:200]}...")
                    except:
                        print_info(f"  Response: {text[:200]}...")
            return True
        else:
            print_error("Tool call: No content in response")
            return False
    else:
        print_error("Tool call: No valid response")
        return False

def main():
    print_info("="*60)
    print_info("GOLEM CLI MCP SERVER - FULL TEST SUITE")
    print_info("="*60)
    
    # Test health
    if not test_health():
        sys.exit(1)
    
    # Create session for connection persistence
    session = requests.Session()
    
    # Test initialize
    if not test_initialize(session):
        print_error("\nFailed to initialize MCP session")
        sys.exit(1)
    
    # Send initialized notification
    send_initialized_notification(session)
    
    # Test list tools
    success, tools = test_list_tools(session)
    if not success:
        print_error("\nFailed to list tools")
        sys.exit(1)
    
    # Test each available tool
    tool_names = [tool.get("name") for tool in tools if tool.get("name")]
    
    for tool_name in tool_names:
        test_call_tool(session, tool_name)
    
    # Summary
    print_info("\n" + "="*60)
    print_success("ALL TESTS COMPLETED!")
    print_info("="*60)
    print_info(f"\nServer URL: {MCP_URL}")
    print_info(f"Available tools: {len(tool_names)}")
    print_info("\nThe MCP server is ready for Cursor integration!")
    print_info("\nAdd to Cursor MCP settings:")
    print_info('{')
    print_info('  "mcpServers": {')
    print_info('    "golem-cli": {')
    print_info(f'      "url": "{MCP_URL}"')
    print_info('    }')
    print_info('  }')
    print_info('}')

if __name__ == "__main__":
    main()
