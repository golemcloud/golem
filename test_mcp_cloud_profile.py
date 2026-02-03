#!/usr/bin/env python3
"""
Test MCP tools with cloud profile explicitly set
"""

import subprocess
import json
import sys
import os

GOLEM_CLI = r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"
RUST_DEPLOY_DIR = r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\rust-deploy"

def test_mcp_cloud():
    print("=" * 70)
    print("TESTING MCP TOOLS WITH CLOUD PROFILE")
    print("=" * 70)
    
    # Start MCP server with cloud profile and environment
    cmd = [GOLEM_CLI, "--profile", "cloud", "-E", "cloud", "mcp-server", "start", "--transport", "stdio"]
    print(f"\nStarting: {' '.join(cmd)}")
    print(f"CWD: {RUST_DEPLOY_DIR}")
    
    proc = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        cwd=RUST_DEPLOY_DIR
    )
    
    try:
        # Initialize
        print("\n1. Initializing MCP connection...")
        init_req = {
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "cloud-test", "version": "1.0"}
            }
        }
        proc.stdin.write(json.dumps(init_req) + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        data = json.loads(response)
        print(f"   [PASS] Server: {data['result']['serverInfo']['name']} v{data['result']['serverInfo']['version']}")
        
        # Send initialized notification
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        
        # Test list_components
        print("\n2. Calling list_components...")
        req = {
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "list_components", "arguments": {}}
        }
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        data = json.loads(response)
        
        if "result" in data:
            content = json.loads(data["result"]["content"][0]["text"])
            components = content.get("components", [])
            print(f"   [RESULT] {len(components)} component(s)")
            for comp in components:
                print(f"      Component: {comp['name']}")
                print(f"        ID: {comp['id']}")
                print(f"        Revision: {comp['revision']}")
                print(f"        Size: {comp['size']} bytes")
        else:
            print(f"   [ERROR] {data}")
        
        # Test list_agent_types
        print("\n3. Calling list_agent_types...")
        req = {
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "list_agent_types", "arguments": {}}
        }
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        data = json.loads(response)
        
        if "result" in data:
            content = json.loads(data["result"]["content"][0]["text"])
            agent_types = content.get("agent_types", [])
            print(f"   [RESULT] {len(agent_types)} agent type(s)")
            for agent_type in agent_types:
                print(f"      - {agent_type}")
        else:
            print(f"   [ERROR] {data}")
        
        # Test list_workers
        print("\n4. Calling list_workers...")
        req = {
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "list_workers", "arguments": {}}
        }
        proc.stdin.write(json.dumps(req) + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        data = json.loads(response)
        
        if "result" in data:
            content = json.loads(data["result"]["content"][0]["text"])
            workers = content.get("workers", [])
            print(f"   [RESULT] {len(workers)} worker(s)")
            for i, worker in enumerate(workers, 1):
                print(f"      Worker {i}:")
                print(f"        Component: {worker['component_name']}")
                print(f"        Worker ID: {worker['worker_id'][:60]}...")
                print(f"        Status: {worker['status']}")
                print(f"        Created: {worker['created_at']}")
        else:
            print(f"   [ERROR] {data}")
        
        print("\n" + "=" * 70)
        if components or agent_types or workers:
            print("SUCCESS! MCP TOOLS RETURNING ACTUAL DATA")
        else:
            print("WARNING: All tools returned empty arrays")
            print("This means the MCP server is working but not using cloud profile")
        print("=" * 70)
        
    except Exception as e:
        print(f"\n[ERROR] {e}")
        import traceback
        traceback.print_exc()
    finally:
        proc.terminate()
        proc.wait(timeout=5)

if __name__ == "__main__":
    test_mcp_cloud()
