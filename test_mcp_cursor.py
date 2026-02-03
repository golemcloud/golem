#!/usr/bin/env python3
"""Test Golem CLI MCP server for Cursor debugging"""
import subprocess
import json
import sys

def test_mcp_server():
    # Start the MCP server
    proc = subprocess.Popen(
        [r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe",
         "mcp-server", "start", "--transport", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1
    )
    
    # Send initialize request
    init_request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test", "version": "1.0"}
        }
    }
    
    print("Sending initialize request...")
    proc.stdin.write(json.dumps(init_request) + "\n")
    proc.stdin.flush()
    
    # Read response
    print("Reading response...")
    response = proc.stdout.readline()
    print(f"Response: {response}")
    
    if response:
        resp_obj = json.loads(response)
        print(json.dumps(resp_obj, indent=2))
        
        # Send initialized notification
        initialized = {
            "jsonrpc": "2.0",
            "method": "notifications/initialized"
        }
        proc.stdin.write(json.dumps(initialized) + "\n")
        proc.stdin.flush()
        
        # List tools
        list_tools = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/list"
        }
        proc.stdin.write(json.dumps(list_tools) + "\n")
        proc.stdin.flush()
        
        tools_response = proc.stdout.readline()
        print(f"\nTools response: {tools_response}")
        if tools_response:
            tools_obj = json.loads(tools_response)
            print(json.dumps(tools_obj, indent=2))
    
    proc.terminate()
    proc.wait()

if __name__ == "__main__":
    test_mcp_server()
