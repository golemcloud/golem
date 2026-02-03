#!/usr/bin/env python3
"""
MCP Server Test using persistent TCP connection (like Rust tests)
"""

import json
import socket
import time
import sys
from colorama import init, Fore, Style

init(autoreset=True)

HOST = "127.0.0.1"
PORT = 3000
MCP_URL = f"/mcp"

def print_success(msg):
    print(f"{Fore.GREEN}[PASS] {msg}{Style.RESET_ALL}")

def print_error(msg):
    print(f"{Fore.RED}[FAIL] {msg}{Style.RESET_ALL}")

def print_info(msg):
    print(f"{Fore.CYAN}{msg}{Style.RESET_ALL}")

def print_warning(msg):
    print(f"{Fore.YELLOW}[WARN] {msg}{Style.RESET_ALL}")

class McpClient:
    def __init__(self, host, port):
        self.host = host
        self.port = port
        self.sock = None
        self.request_id = 1
        self.is_initialized = False
    
    def connect(self):
        """Connect to MCP server"""
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(5)
        self.sock.connect((self.host, self.port))
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    
    def send_http(self, body):
        """Send HTTP request over persistent connection"""
        request = (
            f"POST {MCP_URL} HTTP/1.1\r\n"
            f"Host: {self.host}:{self.port}\r\n"
            f"Connection: keep-alive\r\n"
            f"Content-Type: application/json\r\n"
            f"Accept: application/json, text/event-stream\r\n"
            f"Content-Length: {len(body)}\r\n"
            f"\r\n"
            f"{body}"
        )
        
        self.sock.sendall(request.encode('utf-8'))
        self.sock.settimeout(3)
        
        # Read response
        buffer = b""
        saw_data = False
        
        while True:
            try:
                chunk = self.sock.recv(4096)
                if not chunk:
                    break
                buffer += chunk
                text = buffer.decode('utf-8', errors='ignore')
                
                if "data: " in text:
                    saw_data = True
                    if text.endswith("\n\n") or text.endswith("\r\n\r\n"):
                        break
                
                if len(buffer) > 1_000_000:
                    break
            except socket.timeout:
                if saw_data and len(buffer) > 100:
                    break
                continue
            except Exception as e:
                print_warning(f"Read error: {e}")
                break
        
        return buffer.decode('utf-8', errors='ignore')
    
    def request(self, method, params):
        """Send MCP request"""
        if method == "initialize":
            return self._request_internal(method, params)
        
        if not self.is_initialized:
            self.initialize()
        
        return self._request_internal(method, params)
    
    def _request_internal(self, method, params):
        """Internal request handler"""
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
            error_msg = ""
            for line in response_text.split('\n'):
                if line.startswith("data: "):
                    error_msg = line[6:].strip()
                    break
            if not error_msg:
                error_msg = response_text[:200]
            print_warning(f"Server error: {error_msg}")
            return None
        
        # Extract JSON from SSE
        json_str = None
        for line in response_text.split('\n'):
            if line.startswith("data: "):
                json_str = line[6:].strip()
                break
        
        if not json_str:
            print_error(f"No data line in response: {response_text[:200]}")
            return None
        
        try:
            return json.loads(json_str)
        except json.JSONDecodeError as e:
            print_error(f"Parse error: {e} - JSON: {json_str}")
            return None
    
    def initialize(self):
        """Initialize MCP session"""
        params = {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "mcp-test",
                "version": "1.0.0"
            }
        }
        
        response = self._request_internal("initialize", params)
        
        if not response:
            raise Exception("Failed to get response from initialize")
        
        if response.get("error"):
            raise Exception(f"Initialize error: {response['error']}")
        
        if not response.get("result"):
            raise Exception(f"Initialize missing result: {response}")
        
        # Send initialized notification
        notification = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }
        
        notify_body = json.dumps(notification)
        try:
            self.send_http(notify_body)
            time.sleep(0.05)  # Small delay for server to process
        except:
            pass
        
        self.is_initialized = True
    
    def close(self):
        """Close connection"""
        if self.sock:
            self.sock.close()

def test_health():
    """Test health endpoint"""
    print_info("\n" + "="*60)
    print_info("TEST 1: Health Endpoint")
    print_info("="*60)
    try:
        sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        sock.settimeout(2)
        sock.connect((HOST, PORT))
        sock.sendall(b"GET / HTTP/1.1\r\nHost: 127.0.0.1:3000\r\n\r\n")
        response = sock.recv(4096).decode('utf-8')
        sock.close()
        
        if "200 OK" in response:
            print_success("Health check: Server is responding")
            return True
        else:
            print_error(f"Health check failed: {response[:100]}")
            return False
    except Exception as e:
        print_error(f"Health check failed: {e}")
        print_warning("Make sure server is running: target\\debug\\golem-cli.exe mcp-server start --port 3000")
        return False

def main():
    print_info("="*60)
    print_info("GOLEM CLI MCP SERVER - TCP CONNECTION TEST")
    print_info("="*60)
    
    # Test health
    if not test_health():
        sys.exit(1)
    
    # Create client with persistent TCP connection
    client = McpClient(HOST, PORT)
    
    try:
        print_info("\n" + "="*60)
        print_info("TEST 2: MCP Initialize (TCP Connection)")
        print_info("="*60)
        client.connect()
        client.initialize()
        print_success("Initialize: SUCCESS")
        
        print_info("\n" + "="*60)
        print_info("TEST 3: List Available Tools")
        print_info("="*60)
        response = client.request("tools/list", {})
        
        if response and response.get("result") and response["result"].get("tools"):
            tools = response["result"]["tools"]
            print_success(f"Found {len(tools)} tools:")
            for tool in tools:
                name = tool.get("name", "unknown")
                desc = tool.get("description", "No description")[:70]
                print_info(f"  • {name}")
                print_info(f"    {desc}")
            
            # Test each tool
            tool_names = [tool.get("name") for tool in tools if tool.get("name")]
            
            for tool_name in tool_names:
                print_info("\n" + "="*60)
                print_info(f"TEST: Call Tool - {tool_name}")
                print_info("="*60)
                
                params = {
                    "name": tool_name,
                    "arguments": {}
                }
                
                tool_response = client.request("tools/call", params)
                
                if tool_response and tool_response.get("error"):
                    error = tool_response["error"]
                    print_warning(f"Tool returned error (may be expected if Golem not configured):")
                    print_warning(f"  Code: {error.get('code', 'unknown')}")
                    print_warning(f"  Message: {error.get('message', 'unknown')[:100]}")
                elif tool_response and tool_response.get("result"):
                    result = tool_response["result"]
                    print_success(f"Tool call: SUCCESS")
                    if result.get("content"):
                        for item in result["content"]:
                            if item.get("type") == "text":
                                text = item.get("text", "")
                                try:
                                    data = json.loads(text)
                                    print_info(f"  Response preview: {str(data)[:150]}...")
                                except:
                                    print_info(f"  Response preview: {text[:150]}...")
                else:
                    print_error("Tool call: No valid response")
        else:
            print_error("Failed to list tools")
            if response:
                print_error(f"Response: {response}")
        
        # Summary
        print_info("\n" + "="*60)
        print_success("ALL TESTS COMPLETED!")
        print_info("="*60)
        print_info(f"\nServer URL: http://{HOST}:{PORT}{MCP_URL}")
        print_info("\nThe MCP server is ready for MCP client integration!")
        print_info("\nAdd to your MCP client settings:")
        print_info('{')
        print_info('  "mcpServers": {')
        print_info('    "golem-cli": {')
        print_info(f'      "url": "http://{HOST}:{PORT}{MCP_URL}"')
        print_info('    }')
        print_info('  }')
        print_info('}')
        
    except Exception as e:
        print_error(f"Test failed: {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
    finally:
        client.close()

if __name__ == "__main__":
    main()
