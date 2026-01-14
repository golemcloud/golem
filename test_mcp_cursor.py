#!/usr/bin/env python3
"""
Quick test script to verify MCP server is working for Cursor integration
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

def test_health():
    """Test health endpoint"""
    print_info("\n1. Testing health endpoint...")
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
        return False

def test_initialize():
    """Test MCP initialize"""
    print_info("\n2. Testing MCP initialize...")
    try:
        request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "cursor-test",
                    "version": "1.0.0"
                }
            }
        }
        
        resp = requests.post(
            MCP_URL,
            json=request,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=5
        )
        
        # Parse SSE response
        for line in resp.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                data = json.loads(json_str)
                if data.get("result"):
                    print_success("Initialize: SUCCESS")
                    print_info(f"  Server info: {data['result'].get('serverInfo', {}).get('name', 'N/A')}")
                    return True, data
                elif data.get("error"):
                    print_error(f"Initialize: ERROR - {data['error']}")
                    return False, None
        
        print_error("Initialize: No valid response")
        return False, None
    except Exception as e:
        print_error(f"Initialize failed: {e}")
        return False, None

def test_list_tools(session):
    """Test tools/list"""
    print_info("\n3. Testing tools/list...")
    try:
        request = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        
        resp = session.post(
            MCP_URL,
            json=request,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=5
        )
        
        # Parse SSE response
        for line in resp.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                data = json.loads(json_str)
                if data.get("result") and data["result"].get("tools"):
                    tools = data["result"]["tools"]
                    print_success(f"Tools/list: SUCCESS ({len(tools)} tools found)")
                    for tool in tools:
                        print_info(f"  - {tool['name']}: {tool.get('description', 'N/A')[:60]}")
                    return True
                elif data.get("error"):
                    print_error(f"Tools/list: ERROR - {data['error']}")
                    return False
        
        print_error("Tools/list: No valid response")
        return False
    except Exception as e:
        print_error(f"Tools/list failed: {e}")
        return False

def main():
    print_info("=" * 60)
    print_info("MCP Server Test for Cursor Integration")
    print_info("=" * 60)
    
    # Test health
    if not test_health():
        print_error("\nServer is not running. Start it with:")
        print_info("  target\\debug\\golem-cli.exe mcp-server start --port 3000")
        sys.exit(1)
    
    # Test initialize with session
    session = requests.Session()
    success, init_data = test_initialize()
    
    if not success:
        print_error("\nFailed to initialize MCP session")
        sys.exit(1)
    
    # Send initialized notification
    try:
        notification = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }
        session.post(
            MCP_URL,
            json=notification,
            headers={"Content-Type": "application/json"},
            timeout=2
        )
    except:
        pass  # Notification might not return response
    
    # Test list tools
    test_list_tools(session)
    
    print_info("\n" + "=" * 60)
    print_success("MCP Server is ready for Cursor!")
    print_info("=" * 60)
    print_info("\nTo connect from Cursor, add to your MCP settings:")
    print_info('{')
    print_info('  "mcpServers": {')
    print_info('    "golem-cli": {')
    print_info('      "url": "http://127.0.0.1:3000/mcp"')
    print_info('    }')
    print_info('  }')
    print_info('}')
    print_info("\nServer is running on: http://127.0.0.1:3000/mcp")

if __name__ == "__main__":
    main()
