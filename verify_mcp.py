import subprocess
import json
import time
import os
import sys

# Configuration
BASE_URL = "http://127.0.0.1:13337/mcp"
CURL_PATH = r"C:\Windows\System32\curl.exe"
COOKIE_JAR = "mcp_cookies.txt"

def run_curl(args, data=None):
    cmd = [CURL_PATH, "-s", "-i"] + args
    if data:
        # Write data to temp file to avoid shell quoting hell
        with open("temp_payload.json", "w") as f:
            f.write(data)
        cmd.extend(["-d", "@temp_payload.json"])
        
    result = subprocess.run(cmd, capture_output=True, text=True)
    if os.path.exists("temp_payload.json"):
        os.remove("temp_payload.json")
        
    return result.stdout

def parse_header(response, header_name):
    for line in response.splitlines():
        if line.lower().startswith(header_name.lower() + ":"):
            return line.split(":", 1)[1].strip()
    return None

def verify():
    print("--- 1. Initialize ---")
    data = json.dumps({
        "jsonrpc": "2.0", 
        "id": 1, 
        "method": "initialize", 
        "params": {
            "protocolVersion": "2024-11-05", 
            "capabilities": {}, 
            "clientInfo": {"name": "test", "version": "1.0"}
        }
    })
    
    resp = run_curl([
        "-X", "POST", BASE_URL,
        "-H", "Content-Type: application/json",
        "-H", "Accept: application/json, text/event-stream",
        "-c", COOKIE_JAR
    ], data)
    
    session_id = parse_header(resp, "mcp-session-id")
    print(f"Session ID: {session_id}")
    
    if not session_id:
        # Fallback to checking if cookie jar was written
        if os.path.exists(COOKIE_JAR):
            print("Cookie jar created.")
        else:
            print("WARNING: No session ID header and no cookie jar.")
            
    header_args = [
        "-H", "Content-Type: application/json",
        "-H", "Accept: application/json, text/event-stream",
        "-b", COOKIE_JAR,
        "-c", COOKIE_JAR
    ]
    
    if session_id:
        header_args.extend(["-H", f"mcp-session-id: {session_id}"])

    print("\n--- 2. Initialized Notification ---")
    data = json.dumps({
        "jsonrpc": "2.0", 
        "method": "notifications/initialized", 
        "params": {}
    })
    resp = run_curl(["-X", "POST", BASE_URL] + header_args, data)
    if "Unexpected message" in resp:
        print("FAIL: Handshake failed")
        return False
    print("OK")

    print("\n--- 3. List Tools ---")
    data = json.dumps({
        "jsonrpc": "2.0", 
        "id": 2, 
        "method": "tools/list", 
        "params": {}
    })
    resp = run_curl(["-X", "POST", BASE_URL] + header_args, data)
    if "list_agent_types" in resp:
        print("OK: Found list_agent_types")
    else:
        print(f"FAIL: Tools not found. Response:\n{resp}")
        return False

    print("\n--- 4. Call list_components ---")
    data = json.dumps({
        "jsonrpc": "2.0", 
        "id": 3, 
        "method": "tools/call", 
        "params": {
            "name": "list_components", 
            "arguments": {}
        }
    })
    resp = run_curl(["-X", "POST", BASE_URL] + header_args, data)
    if "components" in resp:
        print("OK: Call successful")
    else:
        print(f"FAIL: Call failed. Response:\n{resp}")
        return False
        
    return True

if __name__ == "__main__":
    if not os.path.exists(CURL_PATH):
        print(f"Error: curl not found at {CURL_PATH}")
        sys.exit(1)
        
    try:
        success = verify()
        if success:
            print("\n✅ Verification Successful")
            sys.exit(0)
        else:
            print("\n❌ Verification Failed")
            sys.exit(1)
    finally:
        if os.path.exists(COOKIE_JAR):
            os.remove(COOKIE_JAR)

