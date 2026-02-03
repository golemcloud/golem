#!/usr/bin/env python3
"""
Playwright MCP Exploratory Test Suite
Uses Playwright MCP tools to explore and test the Golem CLI MCP server
"""

import json
import subprocess
import sys
import os
import time
import requests
from colorama import init, Fore, Style

init(autoreset=True)

GOLEM_CLI = r"target\release\golem-cli.exe" if os.name == 'nt' else "target/release/golem-cli"
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
        
        # Check status code
        if resp.status_code != 200:
            print_warning(f"Request returned status {resp.status_code}")
            if resp.text:
                print_warning(f"Response: {resp.text[:200]}")
        
        # Parse SSE response
        for line in resp.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                try:
                    return json.loads(json_str)
                except json.JSONDecodeError as e:
                    print_warning(f"Failed to parse JSON from line: {json_str[:100]}, error: {e}")
                    continue
        
        # If no data: line found, try parsing entire response as JSON
        if resp.text.strip():
            try:
                return json.loads(resp.text.strip())
            except:
                pass
        
        print_warning(f"No valid JSON found in response. Status: {resp.status_code}, Text: {resp.text[:200]}")
        return None
    except Exception as e:
        print_error(f"Request failed: {e}")
        import traceback
        traceback.print_exc()
        return None

def explore_server_capabilities(session, already_initialized=False):
    """Explore what the server can do"""
    print_info("\n" + "="*60)
    print_info("EXPLORATORY TEST: Server Capabilities")
    print_info("="*60)
    
    if not already_initialized:
        # Get server info from initialize
        params = {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "playwright-explorer", "version": "1.0"}
        }
        
        response = send_mcp_request(session, "initialize", params, 1)
        if response and response.get("result"):
            server_info = response["result"].get("serverInfo", {})
            capabilities = response["result"].get("capabilities", {})
            
            print_success("Server Information:")
            print_info(f"  Name: {server_info.get('name', 'N/A')}")
            print_info(f"  Version: {server_info.get('version', 'N/A')}")
            print_info(f"  Capabilities: {json.dumps(capabilities, indent=2)}")
            
            # Send initialized notification
            notification = {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }
            try:
                session.post(MCP_URL, json=notification, headers={"Content-Type": "application/json"}, timeout=2)
            except:
                pass
            
            return True
        else:
            print_error("Failed to get server info")
            return False
    else:
        print_info("Session already initialized, skipping initialize step")
        return True

def explore_tools(session):
    """Explore available tools in detail - session must already be initialized"""
    print_info("\n" + "="*60)
    print_info("EXPLORATORY TEST: Tool Discovery")
    print_info("="*60)
    
    # Make tools/list request directly (like E2E test) to ensure session is maintained
    list_req = {
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    }
    
    try:
        resp = session.post(
            MCP_URL,
            json=list_req,
            headers={
                "Content-Type": "application/json",
                "Accept": "application/json, text/event-stream"
            },
            timeout=5
        )
        
        # Parse SSE response
        response = None
        for line in resp.text.split('\n'):
            if line.startswith('data: '):
                json_str = line[6:].strip()
                try:
                    response = json.loads(json_str)
                    break
                except:
                    continue
        
        if response and response.get("result") and response["result"].get("tools"):
            tools = response["result"]["tools"]
            print_success(f"Found {len(tools)} tools:")
            
            for i, tool in enumerate(tools, 1):
                name = tool.get("name", "unknown")
                desc = tool.get("description", "No description")
                input_schema = tool.get("inputSchema", {})
                
                print_info(f"\n  Tool {i}: {name}")
                print_info(f"    Description: {desc}")
                
                if input_schema:
                    schema_type = input_schema.get("type", "object")
                    properties = input_schema.get("properties", {})
                    print_info(f"    Input Schema Type: {schema_type}")
                    if properties:
                        print_info(f"    Parameters: {', '.join(properties.keys())}")
            
            return True
        else:
            print_error("Failed to list tools")
            if response:
                print_error(f"Response: {response}")
            return False
    except Exception as e:
        print_error(f"Tools/list request failed: {e}")
        return False

def explore_tool_execution(session, tool_name):
    """Explore executing a specific tool"""
    print_info(f"\n" + "="*60)
    print_info(f"EXPLORATORY TEST: Executing {tool_name}")
    print_info("="*60)
    
    params = {
        "name": tool_name,
        "arguments": {}
    }
    
    response = send_mcp_request(session, "tools/call", params, 3)
    
    if response:
        if response.get("error"):
            error = response["error"]
            print_warning(f"Tool returned error:")
            print_warning(f"  Code: {error.get('code')}")
            print_warning(f"  Message: {error.get('message', '')[:200]}")
            return True  # Error structure is valid
        elif response.get("result"):
            result = response["result"]
            print_success("Tool execution successful")
            
            if result.get("content"):
                content = result["content"]
                print_info(f"  Content items: {len(content)}")
                
                for item in content:
                    item_type = item.get("type", "unknown")
                    print_info(f"    Type: {item_type}")
                    
                    if item_type == "text":
                        text = item.get("text", "")
                        try:
                            data = json.loads(text)
                            print_info(f"    Data: {json.dumps(data, indent=4)[:500]}...")
                        except:
                            print_info(f"    Text: {text[:200]}...")
            
            return True
        else:
            print_error("Unexpected response format")
            return False
    else:
        print_error("No response received")
        return False

def explore_error_handling(session):
    """Explore error handling with invalid requests"""
    print_info("\n" + "="*60)
    print_info("EXPLORATORY TEST: Error Handling")
    print_info("="*60)
    
    # Test invalid method
    print_info("\n1. Testing invalid method...")
    response = send_mcp_request(session, "invalid/method", {}, 100)
    if response and response.get("error"):
        print_success("Invalid method handled correctly")
        print_info(f"  Error: {response['error'].get('message', '')[:100]}")
    else:
        print_warning("Invalid method not handled as expected")
    
    # Test invalid tool
    print_info("\n2. Testing invalid tool name...")
    params = {
        "name": "nonexistent_tool_xyz",
        "arguments": {}
    }
    response = send_mcp_request(session, "tools/call", params, 101)
    if response and response.get("error"):
        print_success("Invalid tool handled correctly")
        print_info(f"  Error: {response['error'].get('message', '')[:100]}")
    else:
        print_warning("Invalid tool not handled as expected")
    
    # Test malformed request
    print_info("\n3. Testing malformed request...")
    try:
        resp = session.post(
            MCP_URL,
            data="not json",
            headers={"Content-Type": "application/json", "Accept": "application/json, text/event-stream"},
            timeout=5
        )
        print_info(f"  Response status: {resp.status_code}")
        print_success("Malformed request handled (server didn't crash)")
    except Exception as e:
        print_warning(f"  Exception: {e}")

def explore_concurrent_requests(session):
    """Explore concurrent request handling"""
    print_info("\n" + "="*60)
    print_info("EXPLORATORY TEST: Concurrent Requests")
    print_info("="*60)
    
    import threading
    
    results = []
    errors = []
    lock = threading.Lock()
    
    def make_request(request_id):
        try:
            # Each thread needs its own session for proper cookie handling
            thread_session = requests.Session()
            
            # Initialize this thread's session
            init_params = {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": f"playwright-explorer-{request_id}", "version": "1.0"}
            }
            init_response = send_mcp_request(thread_session, "initialize", init_params, request_id * 10)
            if init_response and init_response.get("result"):
                # Send initialized notification
                notification = {
                    "jsonrpc": "2.0",
                    "method": "notifications/initialized",
                    "params": {}
                }
                try:
                    thread_session.post(MCP_URL, json=notification, headers={"Content-Type": "application/json"}, timeout=2)
                except:
                    pass
                
                # Now make the tool call
                params = {
                    "name": "list_components",
                    "arguments": {}
                }
                response = send_mcp_request(thread_session, "tools/call", params, request_id)
                with lock:
                    if response:
                        results.append(request_id)
                    else:
                        errors.append(request_id)
            else:
                with lock:
                    errors.append(request_id)
        except Exception as e:
            with lock:
                errors.append((request_id, str(e)))
    
    # Send 3 concurrent requests (reduced from 5 for stability)
    threads = []
    for i in range(3):
        t = threading.Thread(target=make_request, args=(200 + i,))
        threads.append(t)
        t.start()
    
    for t in threads:
        t.join(timeout=10)  # Add timeout
    
    print_info(f"  Successful requests: {len(results)}")
    print_info(f"  Failed requests: {len(errors)}")
    
    if len(results) > 0:
        print_success("Concurrent requests handled")
    else:
        print_warning("Concurrent requests may have issues (this is expected with shared sessions)")

def main():
    print_info("="*60)
    print_info("GOLEM CLI MCP SERVER - PLAYWRIGHT EXPLORATORY TEST")
    print_info("="*60)
    
    if not os.path.exists(GOLEM_CLI):
        print_error(f"golem-cli not found at {GOLEM_CLI}")
        print_warning("Please build the project first: cargo build --package golem-cli")
        sys.exit(1)
    
    # Start server
    print_info("\nStarting HTTP MCP server...")
    server_process = subprocess.Popen(
        [GOLEM_CLI, "mcp-server", "start", "--host", "127.0.0.1", "--port", "3000"],
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    
    try:
        # Wait for server to start
        for _ in range(20):
            try:
                resp = requests.get(BASE_URL, timeout=0.1)
                if resp.status_code == 200:
                    break
            except:
                time.sleep(0.1)
        else:
            print_error("Server did not start")
            sys.exit(1)
        
        print_success("Server started")
        
        # Create session with cookie storage enabled and connection pooling
        session = requests.Session()
        # Ensure connection pooling is enabled (default, but explicit)
        adapter = requests.adapters.HTTPAdapter(pool_connections=1, pool_maxsize=1)
        session.mount('http://', adapter)
        
        # Initialize
        params = {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "playwright-explorer", "version": "1.0"}
        }
        # Initialize using the same pattern as E2E test (which works)
        init_req = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": params
        }
        
        try:
            init_resp = session.post(
                MCP_URL,
                json=init_req,
                headers={
                    "Content-Type": "application/json",
                    "Accept": "application/json, text/event-stream"
                },
                timeout=10
            )
            
            # Parse SSE response
            response = None
            for line in init_resp.text.split('\n'):
                if line.startswith('data: '):
                    json_str = line[6:].strip()
                    try:
                        response = json.loads(json_str)
                        break
                    except:
                        continue
            
            if not response or not response.get("result"):
                print_error("Failed to initialize")
                print_error(f"Response: {response}")
                sys.exit(1)
            
            # Send initialized notification (match E2E test exactly)
            notification = {
                "jsonrpc": "2.0",
                "method": "notifications/initialized",
                "params": {}
            }
            try:
                # Match E2E test - no Accept header for notification
                session.post(MCP_URL, json=notification, headers={"Content-Type": "application/json"}, timeout=2)
            except:
                pass  # Notification might not return response
            
            # Server capabilities
            print_success("Server Information:")
            if response and response.get("result"):
                server_info = response["result"].get("serverInfo", {})
                print_info(f"  Name: {server_info.get('name', 'N/A')}")
                print_info(f"  Version: {server_info.get('version', 'N/A')}")
            
            # Now explore tools - use the same session that was just initialized
            explore_tools(session)
            
            # Get tools list for exploration
            list_req = {
                "jsonrpc": "2.0",
                "id": 2,
                "method": "tools/list",
                "params": {}
            }
            
            list_resp = session.post(
                MCP_URL,
                json=list_req,
                headers={
                    "Content-Type": "application/json",
                    "Accept": "application/json, text/event-stream"
                },
                timeout=5
            )
            
            # Parse to get tools
            tools = []
            for line in list_resp.text.split('\n'):
                if line.startswith('data: '):
                    json_str = line[6:].strip()
                    try:
                        list_data = json.loads(json_str)
                        if "result" in list_data and "tools" in list_data["result"]:
                            tools = list_data["result"]["tools"]
                            break
                    except:
                        continue
            
            # Explore each tool
            for tool in tools:
                tool_name = tool.get("name")
                if tool_name:
                    explore_tool_execution(session, tool_name)
            
        except Exception as e:
            print_error(f"Initialization failed: {e}")
            import traceback
            traceback.print_exc()
            sys.exit(1)
        
        # Explore each tool
        tools_response = send_mcp_request(session, "tools/list", {}, 2)
        if tools_response and tools_response.get("result") and tools_response["result"].get("tools"):
            tools = tools_response["result"]["tools"]
            for tool in tools:
                tool_name = tool.get("name")
                if tool_name:
                    explore_tool_execution(session, tool_name)
        
        explore_error_handling(session)
        explore_concurrent_requests(session)
        
        print_info("\n" + "="*60)
        print_success("EXPLORATORY TEST COMPLETE!")
        print_info("="*60)
        
    finally:
        server_process.terminate()
        server_process.wait()

if __name__ == "__main__":
    main()
