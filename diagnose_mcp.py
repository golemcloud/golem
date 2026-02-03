#!/usr/bin/env python3
"""
Diagnostic script to verify MCP server configuration and connectivity
"""
import sys
import io
import json
import socket
import requests
from pathlib import Path

# Fix Windows console encoding
if sys.platform == 'win32':
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')

def check_server_running():
    """Check if MCP server is running on port 3000"""
    print("=" * 60)
    print("1. Checking if MCP server is running...")
    print("=" * 60)
    
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2)
        result = sock.connect_ex(('127.0.0.1', 3000))
        sock.close()
        
        if result == 0:
            print("✓ Server is running on port 3000")
            return True
        else:
            print("✗ Server is NOT running on port 3000")
            return False
    except Exception as e:
        print(f"✗ Error checking server: {e}")
        return False

def check_health_endpoint():
    """Check health endpoint"""
    print("\n" + "=" * 60)
    print("2. Checking health endpoint...")
    print("=" * 60)
    
    try:
        response = requests.get("http://127.0.0.1:3000/", timeout=2)
        if response.status_code == 200:
            print(f"✓ Health endpoint OK: {response.text.strip()}")
            return True
        else:
            print(f"✗ Health endpoint returned status {response.status_code}")
            return False
    except Exception as e:
        print(f"✗ Health endpoint failed: {e}")
        return False


def test_mcp_connection():
    """Test MCP connection and list tools"""
    print("\n" + "=" * 60)
    print("4. Testing MCP connection and listing tools...")
    print("=" * 60)
    
    try:
        # Use requests directly to test MCP
        import time
        
        # Create a persistent connection like the test script does
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.connect(('127.0.0.1', 3000))
        
        # Send initialize request
        init_request = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {
                    "name": "diagnostic-script",
                    "version": "1.0.0"
                }
            }
        }
        
        request_str = json.dumps(init_request) + "\n"
        sock.sendall(request_str.encode('utf-8'))
        
        # Wait for response
        time.sleep(0.5)
        response_data = sock.recv(4096).decode('utf-8')
        
        if '"result"' in response_data:
            print("✓ MCP connection initialized")
        else:
            print(f"✗ Failed to initialize: {response_data[:200]}")
            sock.close()
            return False
        
        # Send tools/list request
        tools_request = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list",
            "params": {}
        }
        
        request_str = json.dumps(tools_request) + "\n"
        sock.sendall(request_str.encode('utf-8'))
        
        # Wait for response
        time.sleep(0.5)
        response_data = sock.recv(4096).decode('utf-8')
        sock.close()
        
        # Parse response
        try:
            # Extract JSON from SSE format if needed
            if "data:" in response_data:
                lines = response_data.split('\n')
                for line in lines:
                    if line.startswith('data:'):
                        json_str = line[5:].strip()
                        if json_str:
                            response = json.loads(json_str)
                            break
            else:
                # Try to find JSON in response
                start = response_data.find('{')
                end = response_data.rfind('}') + 1
                if start >= 0 and end > start:
                    response = json.loads(response_data[start:end])
                else:
                    response = json.loads(response_data)
            
            if "result" in response and "tools" in response["result"]:
                tools = response["result"]["tools"]
                if tools:
                    print(f"✓ Found {len(tools)} tool(s):")
                    for i, tool in enumerate(tools, 1):
                        print(f"  {i}. {tool.get('name', 'unknown')}")
                        print(f"     Description: {tool.get('description', 'N/A')}")
                    return True
                else:
                    print("✗ No tools found in response")
                    return False
            else:
                print(f"✗ Unexpected response format: {response_data[:300]}")
                return False
                
        except json.JSONDecodeError as e:
            print(f"✗ Failed to parse response: {e}")
            print(f"Response: {response_data[:300]}")
            return False
        
    except Exception as e:
        print(f"✗ Error testing MCP connection: {e}")
        import traceback
        traceback.print_exc()
        return False

def main():
    print("\n" + "=" * 60)
    print("  Golem CLI MCP Server Diagnostic")
    print("=" * 60 + "\n")
    
    results = []
    
    results.append(("Server Running", check_server_running()))
    results.append(("Health Endpoint", check_health_endpoint()))
    results.append(("MCP Connection", test_mcp_connection()))
    
    print("\n" + "=" * 60)
    print("  Diagnostic Summary")
    print("=" * 60)
    
    all_passed = True
    for name, passed in results:
        status = "✓ PASS" if passed else "✗ FAIL"
        print(f"{status}: {name}")
        if not passed:
            all_passed = False
    
    print("\n" + "=" * 60)
    if all_passed:
        print("✓ All checks passed! MCP server should be working.")
    else:
        print("✗ Some checks failed. Please fix the issues above.")
        print("\nCommon fixes:")
        print("  1. Start the server: golem-cli mcp-server start")
        print("  2. Verify the server is still running")
    print("=" * 60 + "\n")

if __name__ == "__main__":
    main()
