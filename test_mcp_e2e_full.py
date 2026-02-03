#!/usr/bin/env python3
"""
Comprehensive E2E Tests for Golem CLI MCP Server
Tests: Protocol handshake, tool discovery, tool execution, error handling
"""
import subprocess
import json
import sys
import time
from typing import Optional, Dict, Any

class MCPTestClient:
    def __init__(self, exe_path: str):
        self.exe_path = exe_path
        self.proc: Optional[subprocess.Popen] = None
        self.request_id = 0
        
    def start(self):
        """Start the MCP server process"""
        self.proc = subprocess.Popen(
            [self.exe_path, "mcp-server", "start", "--transport", "stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=1
        )
        return self
    
    def stop(self):
        """Stop the MCP server process"""
        if self.proc:
            self.proc.terminate()
            self.proc.wait(timeout=5)
            
    def send_request(self, method: str, params: Dict[str, Any] = None) -> Dict:
        """Send a JSON-RPC request and get response"""
        self.request_id += 1
        request = {
            "jsonrpc": "2.0",
            "id": self.request_id,
            "method": method,
        }
        if params:
            request["params"] = params
            
        self.proc.stdin.write(json.dumps(request) + "\n")
        self.proc.stdin.flush()
        
        response = self.proc.stdout.readline()
        if response:
            return json.loads(response)
        return {}
    
    def send_notification(self, method: str, params: Dict[str, Any] = None):
        """Send a JSON-RPC notification (no response expected)"""
        notification = {
            "jsonrpc": "2.0",
            "method": method,
        }
        if params:
            notification["params"] = params
        self.proc.stdin.write(json.dumps(notification) + "\n")
        self.proc.stdin.flush()
    
    def call_tool(self, tool_name: str, arguments: Dict[str, Any] = None) -> Dict:
        """Call an MCP tool"""
        params = {"name": tool_name}
        if arguments:
            params["arguments"] = arguments
        return self.send_request("tools/call", params)


def run_tests():
    exe_path = r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"
    
    results = {
        "passed": 0,
        "failed": 0,
        "tests": []
    }
    
    def test(name: str, condition: bool, details: str = ""):
        status = "PASS" if condition else "FAIL"
        results["tests"].append({"name": name, "status": status, "details": details})
        if condition:
            results["passed"] += 1
            print(f"  [PASS] {name}")
        else:
            results["failed"] += 1
            print(f"  [FAIL] {name}: {details}")
    
    print("=" * 60)
    print("GOLEM CLI MCP SERVER - E2E TEST SUITE")
    print("=" * 60)
    
    # ========== TEST 1: Server Startup ==========
    print("\n[TEST GROUP 1: Server Startup]")
    client = MCPTestClient(exe_path)
    try:
        client.start()
        test("Server process starts", client.proc.poll() is None)
    except Exception as e:
        test("Server process starts", False, str(e))
        return results
    
    # ========== TEST 2: Protocol Initialization ==========
    print("\n[TEST GROUP 2: Protocol Initialization]")
    try:
        init_response = client.send_request("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "e2e-test", "version": "1.0"}
        })
        
        test("Initialize returns result", "result" in init_response)
        test("Protocol version in response", 
             init_response.get("result", {}).get("protocolVersion") == "2024-11-05")
        test("Server info present", 
             "serverInfo" in init_response.get("result", {}))
        
        # Send initialized notification
        client.send_notification("notifications/initialized")
        test("Initialized notification sent", True)
        
    except Exception as e:
        test("Protocol initialization", False, str(e))
    
    # ========== TEST 3: Tool Discovery ==========
    print("\n[TEST GROUP 3: Tool Discovery]")
    try:
        tools_response = client.send_request("tools/list")
        
        test("tools/list returns result", "result" in tools_response)
        
        tools = tools_response.get("result", {}).get("tools", [])
        test("Tools array present", isinstance(tools, list))
        test("At least 1 tool available", len(tools) >= 1)
        
        tool_names = [t.get("name") for t in tools]
        test("list_components tool exists", "list_components" in tool_names)
        test("list_agent_types tool exists", "list_agent_types" in tool_names)
        test("list_workers tool exists", "list_workers" in tool_names)
        
        # Validate tool schema
        for tool in tools:
            has_schema = "inputSchema" in tool and "description" in tool
            test(f"Tool '{tool.get('name')}' has schema", has_schema)
            
    except Exception as e:
        test("Tool discovery", False, str(e))
    
    # ========== TEST 4: Tool Execution ==========
    print("\n[TEST GROUP 4: Tool Execution]")
    try:
        # Test list_components
        components_result = client.call_tool("list_components")
        test("list_components executes", "result" in components_result or "error" not in components_result)
        
        # Test list_agent_types
        agents_result = client.call_tool("list_agent_types")
        test("list_agent_types executes", "result" in agents_result or "error" not in agents_result)
        
        # Test list_workers
        workers_result = client.call_tool("list_workers")
        test("list_workers executes", "result" in workers_result or "error" not in workers_result)
        
    except Exception as e:
        test("Tool execution", False, str(e))
    
    # ========== TEST 5: Error Handling ==========
    print("\n[TEST GROUP 5: Error Handling]")
    try:
        # Test invalid tool
        invalid_tool = client.call_tool("nonexistent_tool")
        test("Invalid tool returns error", "error" in invalid_tool)
        
        # Test invalid method
        invalid_method = client.send_request("invalid/method")
        test("Invalid method handled", "error" in invalid_method or "result" in invalid_method)
        
    except Exception as e:
        test("Error handling", False, str(e))
    
    # ========== TEST 6: Multiple Sequential Requests ==========
    print("\n[TEST GROUP 6: Sequential Requests]")
    try:
        for i in range(5):
            resp = client.send_request("tools/list")
            test(f"Sequential request {i+1}", "result" in resp)
    except Exception as e:
        test("Sequential requests", False, str(e))
    
    # ========== Cleanup ==========
    client.stop()
    
    # ========== Summary ==========
    print("\n" + "=" * 60)
    print(f"TEST SUMMARY: {results['passed']} passed, {results['failed']} failed")
    print("=" * 60)
    
    return results


if __name__ == "__main__":
    results = run_tests()
    sys.exit(0 if results["failed"] == 0 else 1)
