import subprocess
import json
import os
import sys

# Path to golem-cli executable
GOLEM_CLI = r"target\debug\golem-cli.exe"

def verify_stdio():
    print("--- Starting Stdio Verification ---")
    
    # Start the process
    process = subprocess.Popen(
        [GOLEM_CLI, "mcp-server", "start", "--transport", "stdio"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=sys.stderr,
        text=True,
        bufsize=0  # Unbuffered
    )

    # 1. Initialize
    init_req = json.dumps({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "test-client", "version": "1.0"}
        }
    })
    
    print(f"Sending: {init_req}")
    process.stdin.write(init_req + "\n")
    process.stdin.flush()
    
    # Read response (blocking)
    print("Waiting for response...")
    response_line = process.stdout.readline()
    print(f"Received: {response_line}")
    
    if not response_line:
        print("FAIL: No response received")
        return False
        
    resp = json.loads(response_line)
    if "result" in resp and "protocolVersion" in resp["result"]:
        print("✅ Initialize success")
    else:
        print("FAIL: Invalid initialize response")
        return False

    # 2. Initialized Notification
    notify_req = json.dumps({
        "jsonrpc": "2.0",
        "method": "notifications/initialized",
        "params": {}
    })
    print(f"Sending: {notify_req}")
    process.stdin.write(notify_req + "\n")
    process.stdin.flush()
    
    # 3. List Tools
    list_req = json.dumps({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/list",
        "params": {}
    })
    print(f"Sending: {list_req}")
    process.stdin.write(list_req + "\n")
    process.stdin.flush()
    
    response_line = process.stdout.readline()
    print(f"Received: {response_line}")
    
    if "list_agent_types" in response_line:
        print("✅ List tools success")
    else:
        print("FAIL: Tools not found")
        return False
        
    process.terminate()
    return True

if __name__ == "__main__":
    if not os.path.exists(GOLEM_CLI):
        # Try finding it in path or default location
        if os.path.exists(r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\debug\golem-cli.exe"):
             GOLEM_CLI = r"c:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\debug\golem-cli.exe"
    
    try:
        if verify_stdio():
            print("\n✅ Stdio Verification Successful")
            sys.exit(0)
        else:
            print("\n❌ Stdio Verification Failed")
            sys.exit(1)
    except Exception as e:
        print(f"\n❌ Error: {e}")
        sys.exit(1)
