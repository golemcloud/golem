#!/usr/bin/env python3
"""
Manual MCP Protocol Tests - Test each tool individually with detailed output
"""
import subprocess
import json
import sys
import time

def main():
    exe_path = r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"
    
    print("=" * 70)
    print("MANUAL MCP PROTOCOL TESTS")
    print("=" * 70)
    
    # Start server
    proc = subprocess.Popen(
        [exe_path, "mcp-server", "start", "--transport", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=0  # Unbuffered
    )
    
    def send_and_receive(request: dict, description: str):
        print(f"\n{'='*70}")
        print(f"TEST: {description}")
        print(f"{'='*70}")
        print(f"REQUEST:\n{json.dumps(request, indent=2)}")
        
        proc.stdin.write(json.dumps(request) + "\n")
        proc.stdin.flush()
        
        # Read response with timeout handling
        import select
        import os
        
        # On Windows, just read the line
        response_line = proc.stdout.readline()
        
        if response_line:
            print(f"\nRESPONSE:\n{response_line.strip()}")
            try:
                parsed = json.loads(response_line)
                print(f"\nPARSED:\n{json.dumps(parsed, indent=2)}")
                return parsed
            except json.JSONDecodeError as e:
                print(f"\nJSON PARSE ERROR: {e}")
                return None
        else:
            print("\nNO RESPONSE RECEIVED")
            return None
    
    # Test 1: Initialize
    init_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "manual-test", "version": "1.0"}
        }
    }, "Initialize Connection")
    
    # Send initialized notification
    proc.stdin.write(json.dumps({
        "jsonrpc": "2.0",
        "method": "notifications/initialized"
    }) + "\n")
    proc.stdin.flush()
    print("\n[Sent initialized notification]")
    
    # Test 2: List Tools
    tools_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list"
    }, "List Available Tools")
    
    # Test 3: Call list_components
    components_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "tools/call",
        "params": {
            "name": "list_components",
            "arguments": {}
        }
    }, "Call list_components Tool")
    
    # Test 4: Call list_agent_types
    agents_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/call",
        "params": {
            "name": "list_agent_types",
            "arguments": {}
        }
    }, "Call list_agent_types Tool")
    
    # Test 5: Call list_workers
    workers_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {
            "name": "list_workers",
            "arguments": {}
        }
    }, "Call list_workers Tool")
    
    # Test 6: Invalid tool (error handling)
    error_resp = send_and_receive({
        "jsonrpc": "2.0",
        "id": 6,
        "method": "tools/call",
        "params": {
            "name": "invalid_tool_that_doesnt_exist",
            "arguments": {}
        }
    }, "Call Invalid Tool (Error Handling)")
    
    # Cleanup
    proc.terminate()
    proc.wait(timeout=5)
    
    print("\n" + "=" * 70)
    print("MANUAL TESTS COMPLETE")
    print("=" * 70)


if __name__ == "__main__":
    main()
