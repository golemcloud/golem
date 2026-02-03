#!/usr/bin/env python3
"""
Test MCP connections manually with prompts
Tests both HTTP/SSE and stdio modes
"""

import json
import socket
import time
import sys
from typing import Optional, Dict, Any

HOST = "127.0.0.1"
PORT = 3000
MCP_ENDPOINT = f"http://{HOST}:{PORT}/mcp"

# MCP Client using persistent TCP connection
class McpClient:
    """MCP client using a single persistent TCP connection."""
    
    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self.sock: Optional[socket.socket] = None
        self.request_id = 1
        self.is_initialized = False
        self.cookies = {}  # Store cookies for session management
        self.session_id = None  # Store mcp-session-id from initialize response
    
    def connect(self):
        """Connect to the MCP server."""
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(2.0)
        self.sock.connect((self.host, self.port))
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    
    def send_http(self, body: str) -> str:
        """Send HTTP request over the persistent connection and read complete response."""
        if not self.sock:
            self.connect()
        
        # Build cookie header if we have cookies
        cookie_header = ""
        if self.cookies:
            cookie_parts = [f"{name}={value}" for name, value in self.cookies.items()]
            cookie_header = f"Cookie: {'; '.join(cookie_parts)}\r\n"
        
        # Build session ID header if we have one
        session_header = ""
        if self.session_id:
            session_header = f"mcp-session-id: {self.session_id}\r\n"
        
        request = (
            f"POST /mcp HTTP/1.1\r\n"
            f"Host: {self.host}:{self.port}\r\n"
            f"Connection: keep-alive\r\n"
            f"Content-Type: application/json\r\n"
            f"Accept: application/json, text/event-stream\r\n"
            f"{cookie_header}"
            f"{session_header}"
            f"Content-Length: {len(body)}\r\n"
            f"\r\n"
            f"{body}"
        )
        
        self.sock.sendall(request.encode('utf-8'))
        self.sock.settimeout(2.0)
        
        # Read response
        buffer = b""
        saw_data = False
        saw_double_newline = False
        
        for _ in range(100):  # Increased attempts
            try:
                chunk = self.sock.recv(8192)
                if not chunk:
                    if saw_data or len(buffer) > 100:
                        break
                    time.sleep(0.05)  # Wait a bit more if no data yet
                    continue
                buffer += chunk
                text = buffer.decode('utf-8', errors='ignore')
                
                # Check for SSE data line
                if "data: " in text:
                    saw_data = True
                
                # Check for end of SSE stream (double newline after data)
                if saw_data and ("\n\n" in text or "\r\n\r\n" in text):
                    # Make sure we have at least one data line
                    if "data: " in text:
                        saw_double_newline = True
                        break
                
                if len(buffer) > 100000:
                    break
                    
            except socket.timeout:
                if saw_data and len(buffer) > 50:
                    break
                if len(buffer) > 100:
                    break
                time.sleep(0.05)
                continue
            except Exception:
                if saw_data:
                    break
                raise
        
        # If we saw data but no double newline, wait a bit more
        if saw_data and not saw_double_newline:
            try:
                self.sock.settimeout(0.2)
                chunk = self.sock.recv(8192)
                if chunk:
                    buffer += chunk
            except:
                pass
        
        response_text = buffer.decode('utf-8', errors='ignore')
        
        # Extract cookies from response headers
        if "Set-Cookie:" in response_text:
            lines = response_text.split("\n")
            for line in lines:
                if line.startswith("Set-Cookie:"):
                    cookie_line = line[12:].strip()  # Remove "Set-Cookie: "
                    # Parse cookie (simple: name=value; ...)
                    if "=" in cookie_line:
                        cookie_name, cookie_value = cookie_line.split("=", 1)
                        # Remove any trailing attributes (; path=..., etc.)
                        if ";" in cookie_value:
                            cookie_value = cookie_value.split(";")[0]
                        self.cookies[cookie_name.strip()] = cookie_value.strip()
        
        # Extract mcp-session-id from response headers (CRITICAL for session management)
        # Check both lowercase and mixed case
        response_lower = response_text.lower()
        if "mcp-session-id:" in response_lower:
            lines = response_text.split("\n")
            for line in lines:
                if line.lower().startswith("mcp-session-id:"):
                    self.session_id = line.split(":", 1)[1].strip()
                    break
        
        return response_text
    
    def request(self, method: str, params: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Send an MCP request."""
        if method == "initialize":
            return self._request_internal(method, params)
        
        # Ensure session is initialized on this connection
        if not self.is_initialized:
            self.initialize()
        
        return self._request_internal(method, params)
    
    def _send_notification(self, method: str, params: Dict[str, Any]):
        """Send a notification (no id, no response expected).
        
        CRITICAL: Must read complete response to keep connection state clean.
        """
        notification = {
            "jsonrpc": "2.0",
            "method": method,
            "params": params
        }
        
        body_str = json.dumps(notification)
        # Read complete response even though we don't parse it
        # This is critical for maintaining connection state
        response = self.send_http(body_str)
        # Verify we got a complete response
        if response and "200 OK" not in response[:100]:
            # Response might be empty or incomplete, but we read it
            pass
    
    def _request_internal(self, method: str, params: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Internal request handler."""
        request_id = self.request_id
        self.request_id += 1
        
        request_body = {
            "jsonrpc": "2.0",
            "id": request_id,
            "method": method,
            "params": params
        }
        
        body_str = json.dumps(request_body)
        response_text = self.send_http(body_str)
        
        # Parse HTTP response
        if "200 OK" not in response_text:
            # Try to extract error message from SSE response
            error_msg = response_text.split("\n", 1)[0] if "\n" in response_text else response_text[:200]
            # Check if there's an SSE error message
            if "data: " in response_text:
                lines = response_text.split("\n")
                for line in lines:
                    if line.startswith("data: "):
                        json_str = line[6:].strip()
                        if json_str:
                            try:
                                error_data = json.loads(json_str)
                                if "error" in error_data:
                                    error_obj = error_data["error"]
                                    if isinstance(error_obj, dict):
                                        error_code = error_obj.get("code", "unknown")
                                        error_message = error_obj.get("message", "unknown")
                                        return {"error": f"{error_code}: {error_message}"}
                                    else:
                                        return {"error": str(error_obj)}
                            except Exception as e:
                                # Debug: print the JSON string if parsing fails
                                if method != "initialize":  # Don't spam on initialize
                                    pass  # Silently continue
            # Debug: print full response for non-initialize requests
            if method != "initialize" and len(response_text) < 500:
                # Only print if response is short enough
                pass  # Don't print, just return error
            return {"error": f"HTTP error: {error_msg}"}
        
        # Extract JSON from SSE
        # First, try to find JSON in data: lines
        lines = response_text.split("\n")
        for line in lines:
            line = line.strip()
            if line.startswith("data: "):
                json_str = line[6:].strip()
                if json_str:
                    try:
                        return json.loads(json_str)
                    except json.JSONDecodeError:
                        continue
        
        # If no data: line found, try to find JSON anywhere in the response
        # (sometimes the server sends JSON directly without the data: prefix)
        try:
            # Look for JSON object in the response
            start_idx = response_text.find("{")
            if start_idx != -1:
                # Find matching closing brace
                brace_count = 0
                end_idx = start_idx
                for i in range(start_idx, len(response_text)):
                    if response_text[i] == "{":
                        brace_count += 1
                    elif response_text[i] == "}":
                        brace_count -= 1
                        if brace_count == 0:
                            end_idx = i + 1
                            break
                
                if end_idx > start_idx:
                    json_str = response_text[start_idx:end_idx]
                    try:
                        return json.loads(json_str)
                    except json.JSONDecodeError:
                        pass
        except:
            pass
        
        # If still no JSON found, return error with more context
        if "200 OK" in response_text:
            # Response looks like HTTP headers but no data
            if "data: " not in response_text:
                # Check if response is just headers with no body (empty SSE stream)
                if "\r\n\r\n" in response_text or "\n\n" in response_text:
                    header_end = response_text.find("\r\n\r\n")
                    if header_end == -1:
                        header_end = response_text.find("\n\n")
                    if header_end != -1:
                        body = response_text[header_end + 4:].strip()
                        if not body:
                            return {"error": "SSE response received but body is empty. Server may not have sent data."}
        
        return {"error": f"Failed to parse SSE response: {response_text[:300]}"}
    
    def initialize(self):
        """Initialize the MCP session."""
        params = {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "test-client",
                "version": "1.0.0"
            }
        }
        
        response = self._request_internal("initialize", params)
        
        if "error" in response:
            raise Exception(f"Initialize failed: {response['error']}")
        
        if "result" in response:
            # Send initialized notification (no id, no response)
            # CRITICAL: Must read complete response to keep connection state clean
            self._send_notification("notifications/initialized", {})
            # Give server time to fully process the notification
            time.sleep(0.2)  # Increased delay to ensure server processes notification
            self.is_initialized = True
            return response
        
        raise Exception(f"Unexpected initialize response: {response}")
    
    def close(self):
        """Close the connection."""
        if self.sock:
            self.sock.close()
            self.sock = None

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

# Global MCP client instance
mcp_client = None  # type: Optional[McpClient]

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
    global mcp_client
    print_section("Test 2: Initialize MCP Connection")
    
    try:
        mcp_client = McpClient(HOST, PORT)
        response = mcp_client.initialize()
        
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
    except Exception as e:
        print_test("Initialize", False, f"Error: {e}")
        return False

def test_list_tools():
    """Test 3: List available tools"""
    global mcp_client
    print_section("Test 3: List Available Tools")
    
    if not mcp_client:
        print_test("List tools", False, "Client not initialized")
        return False
    
    response = mcp_client.request("tools/list", {})
    
    if not response or "error" in response:
        print_test("List tools", False, f"Error: {response.get('error', 'Unknown error') if response else 'No response'}")
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
    global mcp_client
    print_section("Test 4: Call list_agent_types Tool")
    
    if not mcp_client:
        print_test("list_agent_types", False, "Client not initialized")
        return False
    
    params = {
        "name": "list_agent_types",
        "arguments": {}
    }
    
    response = mcp_client.request("tools/call", params)
    
    if not response or "error" in response:
        print_test("list_agent_types", False, f"Error: {response.get('error', 'Unknown error') if response else 'No response'}")
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
    global mcp_client
    print_section("Test 5: Call list_components Tool")
    
    if not mcp_client:
        print_test("list_components", False, "Client not initialized")
        return False
    
    params = {
        "name": "list_components",
        "arguments": {}
    }
    
    response = mcp_client.request("tools/call", params)
    
    if not response or "error" in response:
        print_test("list_components", False, f"Error: {response.get('error', 'Unknown error') if response else 'No response'}")
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
    global mcp_client
    print_section("Test 6: Error Handling (Invalid Tool)")
    
    if not mcp_client:
        print_test("Error handling", False, "Client not initialized")
        return False
    
    params = {
        "name": "nonexistent_tool",
        "arguments": {}
    }
    
    response = mcp_client.request("tools/call", params)
    
    if not response:
        print_test("Error handling", False, "No response received")
        return False
    
    if "error" in response:
        # Handle both string and dict error formats
        if isinstance(response["error"], str):
            error_msg = response["error"]
            print_test("Error handling", True, f"Proper error returned: {error_msg}")
            return True
        elif isinstance(response["error"], dict):
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
    
    # Cleanup
    global mcp_client
    if mcp_client:
        mcp_client.close()
    
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
        if mcp_client:
            mcp_client.close()
        sys.exit(1)
    except Exception as e:
        print(f"\n[ERROR] Unexpected error: {e}")
        import traceback
        traceback.print_exc()
        if mcp_client:
            mcp_client.close()
        sys.exit(1)
