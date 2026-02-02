import json
import subprocess
import time

def test_mcp_handshake():
    # Start golem-cli --serve
    # Replace with absolute path to target/debug/golem-cli
    cmd = ["/Users/youngwm/PycharmProjects/ai_scys/ai_scys_001_learn/bounties/001_Golem/target/debug/golem-cli", "--serve"]
    
    process = subprocess.Popen(
        cmd,
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1
    )

    # Prepare initialize request (with clientInfo instead of implementation to test fallback)
    init_request = {
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {
                "name": "MCP Inspector",
                "version": "1.0.0"
            }
        }
    }

    # Send request
    print(f"Sending request: {json.dumps(init_request)}")
    process.stdin.write(json.dumps(init_request) + "\n")
    process.stdin.flush()

    # Read response
    try:
        response_line = process.stdout.readline()
        if response_line:
            print(f"Received response: {response_line.strip()}")
            response_data = json.loads(response_line)
            if "result" in response_data:
                print("SUCCESS: Handshake completed!")
                
                # Send initialized notification
                initialized_notification = {
                    "jsonrpc": "2.0",
                    "method": "initialized"
                }
                print(f"Sending initialized notification: {json.dumps(initialized_notification)}")
                process.stdin.write(json.dumps(initialized_notification) + "\n")
                process.stdin.flush()
                time.sleep(0.1)

                # Now test tools/list
                list_request = {
                    "jsonrpc": "2.0",
                    "id": 2,
                    "method": "tools/list",
                    "params": {}
                }
                print(f"Sending tool list request: {json.dumps(list_request)}")
                process.stdin.write(json.dumps(list_request) + "\n")
                process.stdin.flush()
                
                list_response = process.stdout.readline()
                print(f"Received list response: {list_response.strip()}")
            else:
                print(f"FAILED: Unexpected response: {response_data}")
        else:
            stderr = process.stderr.read()
            print(f"FAILED: No response. Stderr: {stderr}")
    except Exception as e:
        print(f"ERROR: {e}")
    finally:
        process.terminate()

if __name__ == "__main__":
    test_mcp_handshake()
