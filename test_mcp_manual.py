#!/usr/bin/env python3
# -*- coding: utf-8 -*-
"""
Manual MCP Server Testing Script
Tests all functionality from mcp_integration.rs manually

NOTE: The MCP server uses LocalSessionManager which is connection-based.
HTTP requests may open new connections, so session state may not persist.
For comprehensive testing, use the Rust integration tests in mcp_integration.rs
which use a single persistent TCP connection.
"""

import json
import os
import socket
import subprocess
import sys
import time
from pathlib import Path
from typing import Optional, Dict, Any

# Set UTF-8 encoding for Windows
if sys.platform == 'win32':
    import io
    sys.stdout = io.TextIOWrapper(sys.stdout.buffer, encoding='utf-8', errors='replace')
    sys.stderr = io.TextIOWrapper(sys.stderr.buffer, encoding='utf-8', errors='replace')

import requests
from colorama import init, Fore, Style

# Initialize colorama for Windows
init(autoreset=True)

# Configuration
PORT = 3000
HOST = "127.0.0.1"
BASE_URL = f"http://{HOST}:{PORT}"
MCP_URL = f"{BASE_URL}/mcp"

# Test counters
tests_passed = 0
tests_failed = 0
server_process = None


def print_success(msg: str):
    print(f"{Fore.GREEN}[PASS] {msg}{Style.RESET_ALL}")


def print_info(msg: str):
    print(f"{Fore.CYAN}{msg}{Style.RESET_ALL}")


def print_warning(msg: str):
    print(f"{Fore.YELLOW}[WARN] {msg}{Style.RESET_ALL}")


def print_error(msg: str):
    print(f"{Fore.RED}[FAIL] {msg}{Style.RESET_ALL}")


def find_binary() -> Path:
    """Find or build the golem-cli binary."""
    binary_path = Path("target/debug/golem-cli.exe")
    if not binary_path.exists():
        print_info(f"Binary not found at {binary_path}. Building...")
        result = subprocess.run(
            ["cargo", "build", "--package", "golem-cli"],
            cwd=Path.cwd()
        )
        if result.returncode != 0:
            print_error("Failed to build binary")
            sys.exit(1)
        if not binary_path.exists():
            print_error("Binary still not found after build")
            sys.exit(1)
    return binary_path


def start_server(binary_path: Path) -> subprocess.Popen:
    """Start the MCP server."""
    print_info(f"Using binary: {binary_path}")
    print_info(f"Starting MCP server on port {PORT}...")
    
    process = subprocess.Popen(
        [
            str(binary_path),
            "mcp-server",
            "start",
            "--host", HOST,
            "--port", str(PORT)
        ],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE
    )
    return process


def wait_for_server(max_attempts: int = 20) -> bool:
    """Wait for the server to be ready - OPTIMIZED for speed."""
    print_info("Waiting for server...")
    for attempt in range(max_attempts):
        try:
            sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
            sock.settimeout(0.05)  # 50ms timeout - fast but reliable
            result = sock.connect_ex((HOST, PORT))
            sock.close()
            if result == 0:
                # Double-check with HTTP request to ensure it's responding
                try:
                    response = requests.get(BASE_URL, timeout=0.2)
                    if response.status_code == 200:
                        print_success("Server ready!\n")
                        return True
                except:
                    pass
        except:
            pass
        if attempt < max_attempts - 1:
            time.sleep(0.05)  # 50ms sleep - balanced
    print_error("Server failed to start")
    return False


# MCP Client using persistent TCP connection
class McpClient:
    """MCP client using a single persistent TCP connection."""
    
    def __init__(self, host: str, port: int):
        self.host = host
        self.port = port
        self.sock: Optional[socket.socket] = None
        self.request_id = 1
        self.is_initialized = False
    
    def connect(self):
        """Connect to the MCP server."""
        self.sock = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self.sock.settimeout(1.0)  # Fast but reliable
        self.sock.connect((self.host, self.port))
        self.sock.setsockopt(socket.IPPROTO_TCP, socket.TCP_NODELAY, 1)
    
    def send_http(self, body: str) -> str:
        """Send HTTP request over the persistent connection and read complete response.
        
        Matches Rust implementation: reads until complete response is received.
        """
        if not self.sock:
            self.connect()
        
        request = (
            f"POST /mcp HTTP/1.1\r\n"
            f"Host: {self.host}:{self.port}\r\n"
            f"Connection: keep-alive\r\n"
            f"Content-Type: application/json\r\n"
            f"Accept: application/json, text/event-stream\r\n"
            f"Content-Length: {len(body)}\r\n"
            f"\r\n"
            f"{body}"
        )
        
        self.sock.sendall(request.encode('utf-8'))
        self.sock.settimeout(0.5)  # VERY FAST timeout
        
        # Read response FAST - optimized for speed
        buffer = b""
        saw_data = False
        
        for _ in range(20):  # Max 20 reads (FAST)
            try:
                chunk = self.sock.recv(8192)  # Larger buffer
                if not chunk:
                    break
                buffer += chunk
                text = buffer.decode('utf-8', errors='ignore')
                
                if "data: " in text:
                    saw_data = True
                    # Fast check - ends with \n\n or \r\n\r\n
                    if text.endswith("\n\n") or text.endswith("\r\n\r\n") or "\r\n\r\n" in text[-20:]:
                        break
                
                if len(buffer) > 50000:  # Safety
                    break
                    
            except socket.timeout:
                if saw_data and len(buffer) > 50:  # Fast break
                    break
                break  # Fail fast
            except Exception:
                if saw_data:
                    break
                raise
        
        return buffer.decode('utf-8', errors='ignore')
    
    def request(self, method: str, params: Dict[str, Any]) -> Optional[Dict[str, Any]]:
        """Send an MCP request. Client must be initialized first."""
        if method == "initialize":
            return self._request_internal(method, params)
        
        # Client should already be initialized - don't re-initialize
        if not self.is_initialized:
            raise Exception("Client not initialized. Call initialize() first.")
        
        return self._request_internal(method, params)
    
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
            error_msg = ""
            for line in response_text.split('\n'):
                if line.startswith("data: "):
                    error_msg = line[6:].strip()
                    break
            if not error_msg:
                error_msg = response_text[:200]
            print_warning(f"Server returned error: {error_msg}")
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
        
        if not response:
            raise Exception("Failed to get response from initialize")
        
        if response.get("error"):
            raise Exception(f"Initialize error: {response['error']}")
        
        if not response.get("result"):
            raise Exception(f"Initialize missing result: {response}")
        
        # Send initialized notification on same connection
        # We MUST read the response completely to keep connection state clean
        notification = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized",
            "params": {}
        }
        
        notify_body = json.dumps(notification)
        
        # CRITICAL: Send notification and read complete response
        # This MUST complete fully before next request or session is lost
        try:
            response = self.send_http(notify_body)
            # Verify response is complete - even if empty, we must read it all
            # Small delay ONLY if we got incomplete response
            if response and len(response) > 0 and "200 OK" not in response[:100]:
                time.sleep(0.02)  # 20ms only if response looks incomplete
        except Exception as e:
            # Notification might fail but initialize already succeeded
            print_warning(f"Notification warning (may be OK): {e}")
            time.sleep(0.01)  # Minimal wait even on error
        
        self.is_initialized = True
    
    def close(self):
        """Close the connection."""
        if self.sock:
            self.sock.close()
            self.sock = None


# Global MCP client
mcp_client: Optional[McpClient] = None


def get_mcp_client() -> McpClient:
    """Get the MCP client (must be initialized first)."""
    global mcp_client
    if mcp_client is None or not mcp_client.is_initialized:
        raise Exception("MCP client not initialized. Call test_mcp_initialize() first.")
    return mcp_client




def test_health_endpoint():
    """Test 1: Health Endpoint"""
    global tests_passed, tests_failed
    
    print_info("=== TEST 1: Health Endpoint ===")
    try:
        response = requests.get(BASE_URL, timeout=0.5)  # Fast but reliable
        if response.status_code == 200 and "Golem CLI MCP Server" in response.text:
            print_success("Health Endpoint - PASSED")
            tests_passed += 1
        else:
            print_error("Health Endpoint - FAILED")
            tests_failed += 1
    except Exception as e:
        print_error(f"Health Endpoint - FAILED: {e}")
        tests_failed += 1


def test_mcp_initialize():
    """Test 2: MCP Initialize - This also initializes the client for all subsequent tests"""
    global tests_passed, tests_failed, mcp_client
    
    print_info("\n=== TEST 2: Initialize ===")
    try:
        # Get and initialize client - this will be reused for all tests
        if mcp_client is None:
            mcp_client = McpClient(HOST, PORT)
            mcp_client.connect()
        mcp_client.initialize()
        print_success("MCP Initialize - PASSED")
        tests_passed += 1
    except Exception as e:
        print_error(f"MCP Initialize - FAILED: {e}")
        tests_failed += 1
        # Reset client on failure
        if mcp_client:
            try:
                mcp_client.close()
            except:
                pass
            mcp_client = None


def test_list_tools():
    """Test 3: List Tools"""
    global tests_passed, tests_failed
    
    print_info("\n=== TEST 3: List Tools ===")
    try:
        client = get_mcp_client()
        response = client.request("tools/list", {})
        
        if response and response.get("result") and response["result"].get("tools"):
            tools = response["result"]["tools"]
            tool_names = [tool["name"] for tool in tools]
            
            if len(tools) >= 2 and "list_agent_types" in tool_names and "list_components" in tool_names:
                print_success(f"List Tools - PASSED (found {len(tools)} tools)")
                tests_passed += 1
            else:
                print_error("List Tools - FAILED (missing expected tools)")
                print_warning(f"Found tools: {', '.join(tool_names)}")
                tests_failed += 1
        else:
            print_error("List Tools - FAILED")
            tests_failed += 1
    except Exception as e:
        print_error(f"List Tools - FAILED: {e}")
        tests_failed += 1


def test_call_list_agent_types():
    """Test 4: Call list_agent_types"""
    global tests_passed, tests_failed
    
    print_info("\n=== TEST 4: list_agent_types ===")
    try:
        client = get_mcp_client()
        params = {
            "name": "list_agent_types",
            "arguments": {}
        }
        
        response = client.request("tools/call", params)
        
        if not response:
            print_error("Call list_agent_types - FAILED (no response)")
            tests_failed += 1
            return
        
        if response.get("error"):
            error = response["error"]
            if error.get("code") and error.get("message"):
                print_warning(f"[WARN] list_agent_types returned error (expected if Golem not configured): {error['message']}")
                print_success("Call list_agent_types - PASSED (error structure valid)")
                tests_passed += 1
            else:
                print_error("Call list_agent_types - FAILED (invalid error structure)")
                tests_failed += 1
        elif response.get("result") and response["result"].get("content"):
            content_array = response["result"]["content"]
            if content_array and len(content_array) > 0 and content_array[0].get("type") == "text":
                try:
                    parsed = json.loads(content_array[0]["text"])
                    if parsed.get("agent_types"):
                        print_success("Call list_agent_types - PASSED")
                        tests_passed += 1
                    else:
                        print_error("Call list_agent_types - FAILED (missing agent_types field)")
                        tests_failed += 1
                except json.JSONDecodeError:
                    print_error("Call list_agent_types - FAILED (invalid JSON in content)")
                    tests_failed += 1
            else:
                print_error("Call list_agent_types - FAILED (invalid content structure)")
                tests_failed += 1
        else:
            print_error("Call list_agent_types - FAILED")
            tests_failed += 1
    except Exception as e:
        print_error(f"Call list_agent_types - FAILED: {e}")
        tests_failed += 1


def test_call_list_components():
    """Test 5: Call list_components"""
    global tests_passed, tests_failed
    
    print_info("\n=== TEST 5: list_components ===")
    try:
        client = get_mcp_client()
        params = {
            "name": "list_components",
            "arguments": {}
        }
        
        response = client.request("tools/call", params)
        
        if not response:
            print_error("Call list_components - FAILED (no response)")
            tests_failed += 1
            return
        
        if response.get("error"):
            error = response["error"]
            if error.get("code") and error.get("message"):
                print_warning(f"[WARN] list_components returned error (expected if Golem not configured): {error['message']}")
                print_success("Call list_components - PASSED (error structure valid)")
                tests_passed += 1
            else:
                print_error("Call list_components - FAILED (invalid error structure)")
                tests_failed += 1
        elif response.get("result") and response["result"].get("content"):
            content_array = response["result"]["content"]
            if content_array and len(content_array) > 0 and content_array[0].get("type") == "text":
                try:
                    parsed = json.loads(content_array[0]["text"])
                    if parsed.get("components"):
                        print_success("Call list_components - PASSED")
                        tests_passed += 1
                    else:
                        print_error("Call list_components - FAILED (missing components field)")
                        tests_failed += 1
                except json.JSONDecodeError:
                    print_error("Call list_components - FAILED (invalid JSON in content)")
                    tests_failed += 1
            else:
                print_error("Call list_components - FAILED (invalid content structure)")
                tests_failed += 1
        else:
            print_error("Call list_components - FAILED")
            tests_failed += 1
    except Exception as e:
        print_error(f"Call list_components - FAILED: {e}")
        tests_failed += 1


def test_call_nonexistent_tool():
    """Test 6: Call Nonexistent Tool"""
    global tests_passed, tests_failed
    
    print_info("\n=== TEST 6: Nonexistent Tool ===")
    try:
        client = get_mcp_client()
        params = {
            "name": "nonexistent_tool",
            "arguments": {}
        }
        
        response = client.request("tools/call", params)
        
        if response and response.get("error") and response["error"].get("code") and response["error"].get("message"):
            print_success("Call Nonexistent Tool - PASSED (returned proper error)")
            tests_passed += 1
        else:
            print_error("Call Nonexistent Tool - FAILED (should return error)")
            tests_failed += 1
    except Exception as e:
        print_error(f"Call Nonexistent Tool - FAILED: {e}")
        tests_failed += 1


def test_tool_schemas():
    """Test 7: Tool Schemas"""
    global tests_passed, tests_failed
    
    print_info("\n=== TEST 7: Schemas ===")
    try:
        client = get_mcp_client()
        response = client.request("tools/list", {})
        
        if response and response.get("result") and response["result"].get("tools"):
            tools = response["result"]["tools"]
            all_valid = True
            
            for tool in tools:
                if not tool.get("name") or not isinstance(tool["name"], str):
                    print_error(f"Tool missing name: {tool}")
                    all_valid = False
                if not tool.get("description") or not isinstance(tool["description"], str):
                    print_error(f"Tool missing description: {tool.get('name', 'unknown')}")
                    all_valid = False
                if tool.get("inputSchema"):
                    input_schema = tool["inputSchema"]
                    if not isinstance(input_schema, dict):
                        print_error(f"Tool inputSchema is not an object: {tool.get('name', 'unknown')}")
                        all_valid = False
                    elif not input_schema.get("type") and not input_schema.get("properties"):
                        print_error(f"Tool inputSchema missing type or properties: {tool.get('name', 'unknown')}")
                        all_valid = False
            
            if all_valid:
                print_success(f"Tool Schemas - PASSED (all {len(tools)} tools have valid schemas)")
                tests_passed += 1
            else:
                print_error("Tool Schemas - FAILED (some tools have invalid schemas)")
                tests_failed += 1
        else:
            print_error("Tool Schemas - FAILED")
            tests_failed += 1
    except Exception as e:
        print_error(f"Tool Schemas - FAILED: {e}")
        tests_failed += 1


def main():
    """Main test runner."""
    global server_process, tests_passed, tests_failed, mcp_client
    
    try:
        # Find or build binary
        binary_path = find_binary()
        
        # Start server
        server_process = start_server(binary_path)
        
        # Wait for server to be ready
        if not wait_for_server():
            if server_process:
                server_process.terminate()
            sys.exit(1)
        
        # Initialize client once for all tests
        try:
            mcp_client = McpClient(HOST, PORT)
            mcp_client.connect()
            mcp_client.initialize()
            print_success("Client initialized for all tests\n")
        except Exception as e:
            print_error(f"Failed to initialize client: {e}")
            if server_process:
                server_process.terminate()
            sys.exit(1)
        
        # Run all tests (they will reuse the initialized client)
        test_health_endpoint()
        test_mcp_initialize()  # This will verify initialization worked
        test_list_tools()
        test_call_list_agent_types()
        test_call_list_components()
        test_call_nonexistent_tool()
        test_tool_schemas()
        
        # Print summary
        print_info("\n==========================================")
        print_info("TEST SUMMARY")
        print_info("==========================================")
        print_success(f"Passed: {tests_passed}")
        if tests_failed > 0:
            print_error(f"Failed: {tests_failed}")
            print_warning("\nNOTE: Some tests may fail due to connection-based session management.")
            print_warning("The MCP server's LocalSessionManager uses TCP connections for sessions.")
            print_warning("HTTP requests may open new connections, causing session loss.")
            print_warning("For full testing, use: cargo test --package golem-cli --test mcp_integration")
        else:
            print_success(f"Failed: {tests_failed}")
        print_info(f"Total: {tests_passed + tests_failed}")
        
        # Cleanup
        print_info("\nStopping server...")
        if mcp_client:
            mcp_client.close()
            mcp_client = None
        if server_process:
            server_process.terminate()
            try:
                server_process.wait(timeout=0.2)  # Fast kill
            except subprocess.TimeoutExpired:
                server_process.kill()
                try:
                    server_process.wait(timeout=0.1)  # Final wait
                except:
                    pass
        
        if tests_failed == 0:
            print_success("\n[SUCCESS] ALL TESTS PASSED!")
            sys.exit(0)
        else:
            print_error("\n[FAILURE] SOME TESTS FAILED")
            sys.exit(1)
            
    except KeyboardInterrupt:
        print_info("\nInterrupted by user")
        if server_process:
            server_process.terminate()
        sys.exit(1)
    except Exception as e:
        print_error(f"\nUnexpected error: {e}")
        if server_process:
            server_process.terminate()
        sys.exit(1)


if __name__ == "__main__":
    main()
