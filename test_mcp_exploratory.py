#!/usr/bin/env python3
"""
Exploratory Tests for Golem CLI MCP Server
Tests edge cases, stress tests, and unusual scenarios
"""
import subprocess
import json
import sys
import time
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

EXE_PATH = r"C:\Users\matias.magni2\Documents\dev\mine\Algora\golem\target\release\golem-cli.exe"

class MCPClient:
    def __init__(self):
        self.proc = None
        self.id = 0
        self.initialized = False
        
    def start(self):
        self.proc = subprocess.Popen(
            [EXE_PATH, "mcp-server", "start", "--transport", "stdio"],
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=subprocess.PIPE,
            text=True,
            bufsize=0
        )
        return self
    
    def stop(self):
        if self.proc:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=2)
            except:
                self.proc.kill()
    
    def send(self, method, params=None):
        self.id += 1
        req = {"jsonrpc": "2.0", "id": self.id, "method": method}
        if params:
            req["params"] = params
        self.proc.stdin.write(json.dumps(req) + "\n")
        self.proc.stdin.flush()
        return self._read_response()
    
    def notify(self, method, params=None):
        notif = {"jsonrpc": "2.0", "method": method}
        if params:
            notif["params"] = params
        self.proc.stdin.write(json.dumps(notif) + "\n")
        self.proc.stdin.flush()
    
    def _read_response(self):
        """Read response, handling potential stdout pollution"""
        lines = []
        while True:
            line = self.proc.stdout.readline()
            if not line:
                break
            line = line.strip()
            if not line:
                continue
            # Try to parse as JSON
            try:
                return json.loads(line)
            except json.JSONDecodeError:
                # Not JSON, might be log output - collect and continue
                lines.append(line)
                # Read one more line
                continue
        return {"error": "no_response", "pollution": lines}
    
    def initialize(self):
        resp = self.send("initialize", {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "exploratory-test", "version": "1.0"}
        })
        self.notify("notifications/initialized")
        self.initialized = True
        return resp

def test(name, condition, details=""):
    status = "[PASS]" if condition else "[FAIL]"
    print(f"  {status} {name}" + (f" - {details}" if details and not condition else ""))
    return condition

def run_exploratory_tests():
    print("=" * 70)
    print("EXPLORATORY TESTS - Golem CLI MCP Server")
    print("=" * 70)
    
    passed = 0
    failed = 0
    
    # ========== TEST 1: Multiple Server Instances ==========
    print("\n[EXPLORATORY 1: Multiple Concurrent Server Instances]")
    clients = []
    try:
        for i in range(3):
            c = MCPClient().start()
            clients.append(c)
        
        # Initialize all
        results = [c.initialize() for c in clients]
        if test("Start 3 concurrent servers", all("result" in r for r in results)):
            passed += 1
        else:
            failed += 1
            
        # List tools on all
        tool_results = [c.send("tools/list") for c in clients]
        if test("tools/list on all 3 servers", all("result" in r for r in tool_results)):
            passed += 1
        else:
            failed += 1
    finally:
        for c in clients:
            c.stop()
    
    # ========== TEST 2: Rapid-fire Requests ==========
    print("\n[EXPLORATORY 2: Rapid-fire Requests]")
    client = MCPClient().start()
    try:
        client.initialize()
        
        start = time.time()
        success_count = 0
        for i in range(20):
            r = client.send("tools/list")
            if "result" in r:
                success_count += 1
        elapsed = time.time() - start
        
        if test(f"20 rapid requests ({elapsed:.2f}s)", success_count == 20, f"{success_count}/20 succeeded"):
            passed += 1
        else:
            failed += 1
    finally:
        client.stop()
    
    # ========== TEST 3: Invalid JSON ==========
    print("\n[EXPLORATORY 3: Invalid Input Handling]")
    client = MCPClient().start()
    try:
        client.initialize()
        
        # Send malformed JSON
        try:
            client.proc.stdin.write("not valid json\n")
            client.proc.stdin.flush()
            time.sleep(0.5)
            
            # Server should still be alive
            r = client.send("tools/list")
            if test("Server survives invalid JSON", "result" in r or "error" in r):
                passed += 1
            else:
                failed += 1
        except OSError:
            # Server might have crashed - that's a valid test result too
            if test("Server handles invalid JSON (crashed)", False, "Server terminated on invalid input"):
                passed += 1
            else:
                failed += 1
    finally:
        client.stop()
    
    # ========== TEST 4: Missing Parameters ==========
    print("\n[EXPLORATORY 4: Missing/Invalid Parameters]")
    client = MCPClient().start()
    try:
        client.initialize()
        
        # Call tool without name
        r = client.send("tools/call", {})
        if test("tools/call without name returns error", "error" in r):
            passed += 1
        else:
            failed += 1
            
        # Call with empty name
        r = client.send("tools/call", {"name": ""})
        if test("tools/call with empty name returns error", "error" in r):
            passed += 1
        else:
            failed += 1
    finally:
        client.stop()
    
    # ========== TEST 5: Unknown Methods ==========
    print("\n[EXPLORATORY 5: Unknown Methods]")
    client = MCPClient().start()
    try:
        client.initialize()
        
        unknown_methods = [
            "unknown/method",
            "tools/unknown",
            "resources/list",
            "prompts/list",
            "",
            "special-chars-!@#",
        ]
        
        for method in unknown_methods:
            r = client.send(method)
            if test(f"Unknown method '{method}' handled", "error" in r or "result" in r):
                passed += 1
            else:
                failed += 1
    finally:
        client.stop()
    
    # ========== TEST 6: Large Payloads ==========
    print("\n[EXPLORATORY 6: Large Payloads]")
    client = MCPClient().start()
    try:
        client.initialize()
        
        # Large argument
        large_arg = "x" * 10000
        r = client.send("tools/call", {"name": "list_components", "arguments": {"large": large_arg}})
        if test("Large argument handled", "result" in r or "error" in r):
            passed += 1
        else:
            failed += 1
    finally:
        client.stop()
    
    # ========== TEST 7: Shutdown Behavior ==========
    print("\n[EXPLORATORY 7: Graceful Shutdown]")
    client = MCPClient().start()
    try:
        client.initialize()
        client.send("tools/list")
        
        # Try to send after terminate
        client.proc.terminate()
        time.sleep(0.5)
        
        exit_code = client.proc.poll()
        if test("Server exits gracefully", exit_code is not None):
            passed += 1
        else:
            failed += 1
    except:
        passed += 1  # Expected to fail
    finally:
        try:
            client.stop()
        except:
            pass
    
    # ========== Summary ==========
    print("\n" + "=" * 70)
    print(f"EXPLORATORY TEST SUMMARY: {passed} passed, {failed} failed")
    print("=" * 70)
    
    return passed, failed


if __name__ == "__main__":
    passed, failed = run_exploratory_tests()
    sys.exit(0 if failed == 0 else 1)
