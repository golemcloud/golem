#!/usr/bin/env python3
"""
Populate Golem Cloud with test data for MCP server demonstration
Creates components, workers, and agent types to test MCP tools
"""

import subprocess
import sys
import os
import time

GOLEM_CLI = r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"

def run_golem_cmd(args, check=True):
    """Run golem-cli command"""
    cmd = [GOLEM_CLI] + args
    print(f"\n> {' '.join(cmd)}")
    result = subprocess.run(cmd, capture_output=True, text=True)
    
    if result.stdout:
        print(result.stdout)
    if result.stderr:
        print(result.stderr, file=sys.stderr)
    
    if check and result.returncode != 0:
        print(f"Command failed with exit code {result.returncode}")
        return False
    
    return True

def check_authentication():
    """Check if user is authenticated"""
    print("=" * 70)
    print("Checking authentication...")
    print("=" * 70)
    
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "cloud", "account", "get"],
        capture_output=True,
        text=True
    )
    
    if "error" in result.stdout.lower() or "error" in result.stderr.lower():
        print("Not authenticated. Please run:")
        print(f"  {GOLEM_CLI} --profile cloud cloud account get")
        print("  (This will open browser for OAuth login)")
        return False
    
    print("✓ Authenticated")
    return True

def deploy_example_component():
    """Deploy the rust-deploy example component"""
    print("\n" + "=" * 70)
    print("Deploying example component (deploy-counter)...")
    print("=" * 70)
    
    deploy_dir = r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\rust-deploy"
    
    if not os.path.exists(deploy_dir):
        print(f"Deploy directory not found: {deploy_dir}")
        return False
    
    # Build the component
    print("\nBuilding component...")
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "app", "build"],
        cwd=deploy_dir,
        capture_output=True,
        text=True
    )
    
    if result.returncode != 0:
        print("Build failed:")
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        return False
    
    print("✓ Build successful")
    
    # Deploy the component
    print("\nDeploying component...")
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "app", "deploy", "-E", "cloud"],
        cwd=deploy_dir,
        capture_output=True,
        text=True
    )
    
    if result.returncode != 0:
        print("Deploy failed:")
        print(result.stdout)
        print(result.stderr, file=sys.stderr)
        return False
    
    print("✓ Component deployed")
    print(result.stdout)
    return True

def create_test_workers():
    """Create test workers"""
    print("\n" + "=" * 70)
    print("Creating test workers...")
    print("=" * 70)
    
    # Get component list to find our deployed component
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "component", "list"],
        capture_output=True,
        text=True
    )
    
    if "deploy:counter" not in result.stdout and "deploy_counter" not in result.stdout:
        print("Component not found. Deploy failed or component has different name.")
        print("Available components:")
        print(result.stdout)
        return False
    
    # Create a few test workers
    worker_names = ["test-worker-1", "test-worker-2", "test-worker-3"]
    
    for worker_name in worker_names:
        print(f"\nCreating worker: {worker_name}")
        result = subprocess.run(
            [GOLEM_CLI, "--profile", "cloud", "worker", "add",
             "--component", "deploy:counter",
             "--worker-name", worker_name],
            capture_output=True,
            text=True
        )
        
        if result.returncode == 0:
            print(f"✓ Worker {worker_name} created")
        else:
            print(f"⚠ Worker {worker_name} creation failed (might already exist)")
            print(result.stdout)
    
    return True

def register_agent_types():
    """Register agent types"""
    print("\n" + "=" * 70)
    print("Registering agent types...")
    print("=" * 70)
    
    # Note: Agent type registration depends on Golem's agent system
    # This is a placeholder - actual implementation depends on Golem API
    
    print("\nℹ Agent types are typically registered through:")
    print("  1. Golem Cloud console")
    print("  2. Golem API calls")
    print("  3. Component metadata")
    
    print("\nTo register agent types, you may need to:")
    print("  - Use the Golem Cloud web interface")
    print("  - Check Golem documentation for agent registration")
    print("  - Use golem-cli agent commands (if available)")
    
    # Try to list agent types
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "app", "list-agent-types"],
        capture_output=True,
        text=True
    )
    
    if result.returncode == 0:
        print("\nCurrent agent types:")
        print(result.stdout)
    else:
        print("\n⚠ Could not list agent types")
        print(result.stdout)
        print(result.stderr)
    
    return True

def verify_data():
    """Verify the populated data"""
    print("\n" + "=" * 70)
    print("Verifying populated data...")
    print("=" * 70)
    
    # List components
    print("\n1. Components:")
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "component", "list"],
        capture_output=True,
        text=True
    )
    print(result.stdout)
    
    # List workers
    print("\n2. Workers:")
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "worker", "list"],
        capture_output=True,
        text=True
    )
    print(result.stdout)
    
    # List agent types
    print("\n3. Agent types:")
    result = subprocess.run(
        [GOLEM_CLI, "--profile", "cloud", "app", "list-agent-types"],
        capture_output=True,
        text=True
    )
    print(result.stdout)
    
    return True

def test_mcp_tools():
    """Test MCP tools with populated data"""
    print("\n" + "=" * 70)
    print("Testing MCP tools with populated data...")
    print("=" * 70)
    
    import json
    
    # Start MCP server and test
    proc = subprocess.Popen(
        [GOLEM_CLI, "mcp-server", "start", "--transport", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True
    )
    
    try:
        # Initialize
        init_req = json.dumps({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": {
                "protocolVersion": "2024-11-05",
                "capabilities": {},
                "clientInfo": {"name": "test", "version": "1.0"}
            }
        })
        proc.stdin.write(init_req + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        print(f"Initialize: {response[:100]}...")
        
        # Send initialized notification
        proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": "notifications/initialized"}) + "\n")
        proc.stdin.flush()
        
        # Test list_components
        print("\nTesting list_components...")
        req = json.dumps({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": "list_components", "arguments": {}}
        })
        proc.stdin.write(req + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        print(f"Components: {response}")
        
        # Test list_workers
        print("\nTesting list_workers...")
        req = json.dumps({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {"name": "list_workers", "arguments": {}}
        })
        proc.stdin.write(req + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        print(f"Workers: {response}")
        
        # Test list_agent_types
        print("\nTesting list_agent_types...")
        req = json.dumps({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {"name": "list_agent_types", "arguments": {}}
        })
        proc.stdin.write(req + "\n")
        proc.stdin.flush()
        response = proc.stdout.readline()
        print(f"Agent types: {response}")
        
    finally:
        proc.terminate()
        proc.wait(timeout=5)
    
    return True

def main():
    print("=" * 70)
    print("GOLEM MCP TEST DATA POPULATION")
    print("=" * 70)
    
    # Check authentication
    if not check_authentication():
        print("\n❌ Please authenticate first")
        return 1
    
    # Deploy example component
    if not deploy_example_component():
        print("\n⚠ Component deployment failed, but continuing...")
    
    # Create test workers
    if not create_test_workers():
        print("\n⚠ Worker creation had issues, but continuing...")
    
    # Register agent types
    register_agent_types()
    
    # Verify data
    verify_data()
    
    # Test MCP tools
    print("\n" + "=" * 70)
    print("Would you like to test MCP tools now? (y/n)")
    response = input("> ").strip().lower()
    if response == 'y':
        test_mcp_tools()
    
    print("\n" + "=" * 70)
    print("✓ Test data population complete!")
    print("=" * 70)
    print("\nYou can now test the MCP server with:")
    print(f"  {GOLEM_CLI} mcp-server start --transport stdio")
    print("\nOr run the test suite:")
    print("  python test_mcp_e2e_full.py")
    
    return 0

if __name__ == "__main__":
    sys.exit(main())
