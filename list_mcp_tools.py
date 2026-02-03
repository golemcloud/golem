#!/usr/bin/env python3
"""
List all available MCP tools from the golem-cli server
"""

import json
import socket
import time
import sys
from typing import Optional, Dict, Any

HOST = "127.0.0.1"
PORT = 3000

class McpClient:
    """Simple MCP client to list tools."""
    
    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self.sock: Optional[socket.socket] = None
        self.request_id = 1
        self.session_id = None
    
    def connect(self):
        """Connect to the MCP server."""
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(2.0)
        self.sock.connect((self.host, self.port))
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    
    def send_http(self, body: str) -> str:
        """Send HTTP request and read response."""
        if not self.sock:
            self.connect()
        
        session_header = ""
        if self.session_id:
            session_header = f"mcp-session-id: {self.session_id}\r\n"
        
        request = (
            f"POST /mcp HTTP/1.1\r\n"
            f"Host: {self.host}:{self.port}\r\n"
            f"Connection: keep-alive\r\n"
            f"Content-Type: application/json\r\n"
            f"Accept: application/json, text/event-stream\r\n"
            f"{session_header}"
            f"Content-Length: {len(body)}\r\n"
            f"\r\n"
            f"{body}"
        )
        
        self.sock.sendall(request.encode('utf-8'))
        self.sock.settimeout(2.0)
        
        buffer = b""
        saw_data = False
        
        for _ in range(100):
            try:
                chunk = self.sock.recv(8192)
                if not chunk:
                    if saw_data or len(buffer) > 100:
                        break
                    time.sleep(0.05)
                    continue
                buffer += chunk
                text = buffer.decode('utf-8', errors='ignore')
                
                if "data: " in text:
                    saw_data = True
                
                if saw_data and ("\n\n" in text or "\r\n\r\n" in text):
                    if "data: " in text:
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
        
        response_text = buffer.decode('utf-8', errors='ignore')
        
        # Extract mcp-session-id
        if "mcp-session-id:" in response_text.lower():
            lines = response_text.split("\n")
            for line in lines:
                if line.lower().startswith("mcp-session-id:"):
                    self.session_id = line.split(":", 1)[1].strip()
                    break
        
        return response_text
    
    def request(self, method: str, params: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Send an MCP request."""
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
        
        # Parse SSE response
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
        
        # Try to find JSON anywhere
        try:
            start_idx = response_text.find("{")
            if start_idx != -1:
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
        
        return {"error": "Failed to parse response"}
    
    def initialize(self):
        """Initialize the MCP session."""
        params = {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "tool-lister",
                "version": "1.0.0"
            }
        }
        
        response = self.request("initialize", params)
        
        if "error" in response:
            raise Exception(f"Initialize failed: {response['error']}")
        
        if "result" in response:
            # Send initialized notification
            notification = {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }
            body_str = json.dumps(notification)
            self.send_http(body_str)
            time.sleep(0.2)
            return response
        
        raise Exception(f"Unexpected initialize response: {response}")
    
    def list_tools(self):
        """List all available tools."""
        response = self.request("tools/list", {})
        
        if "error" in response:
            raise Exception(f"Failed to list tools: {response['error']}")
        
        if "result" in response:
            return response["result"].get("tools", [])
        
        raise Exception(f"Unexpected response: {response}")
    
    def close(self):
        """Close the connection."""
        if self.sock:
            self.sock.close()
            self.sock = None

def wait_for_server(max_attempts: int = 20) -> bool:
    """Wait for the server to be ready"""
    for attempt in range(max_attempts):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(0.1)
            result = sock.connect_ex((HOST, PORT))
            sock.close()
            if result == 0:
                return True
        except:
            pass
        time.sleep(0.1)
    return False

def main():
    """List all MCP tools"""
    print("=" * 70)
    print("  Golem CLI MCP Tools List")
    print("=" * 70)
    print(f"\nConnecting to: http://{HOST}:{PORT}/mcp\n")
    
    # Check if server is running
    if not wait_for_server():
        print("[ERROR] Server is not running!")
        print("\nPlease start the server first:")
        print("  golem-cli mcp-server start --host 127.0.0.1 --port 3000")
        sys.exit(1)
    
    client = None
    try:
        client = McpClient(HOST, PORT)
        
        print("Initializing MCP connection...")
        client.initialize()
        print("[OK] Connected and initialized\n")
        
        print("Fetching available tools...")
        tools = client.list_tools()
        
        print("\n" + "=" * 70)
        print(f"  Found {len(tools)} tool(s)")
        print("=" * 70)
        print()
        
        if not tools:
            print("No tools available.")
        else:
            for i, tool in enumerate(tools, 1):
                name = tool.get("name", "unknown")
                description = tool.get("description", "No description")
                
                # Get input schema if available
                input_schema = tool.get("inputSchema", {})
                properties = input_schema.get("properties", {})
                required = input_schema.get("required", [])
                
                print(f"{i}. {name}")
                print(f"   Description: {description}")
                
                if properties:
                    print(f"   Parameters:")
                    for param_name, param_info in properties.items():
                        param_type = param_info.get("type", "unknown")
                        param_desc = param_info.get("description", "")
                        is_required = param_name in required
                        req_marker = " (required)" if is_required else " (optional)"
                        print(f"     - {param_name}: {param_type}{req_marker}")
                        if param_desc:
                            print(f"       {param_desc}")
                else:
                    print(f"   Parameters: None")
                
                print()
        
        print("=" * 70)
        
    except Exception as e:
        print(f"\n[ERROR] {e}")
        import traceback
        traceback.print_exc()
        sys.exit(1)
    finally:
        if client:
            client.close()

if __name__ == "__main__":
    main()
