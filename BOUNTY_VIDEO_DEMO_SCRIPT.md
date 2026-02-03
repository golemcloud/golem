# MCP Server Demo Video Script - Bounty #1926

## Video Overview
**Duration:** 3-5 minutes  
**Purpose:** Demonstrate MCP server integration in Golem CLI  
**Audience:** Algora bounty reviewers, Golem community

---

## Scene 1: Introduction (30 seconds)

**Visual:** Terminal with Golem CLI logo/banner

**Script:**
> "Hi! Today I'm demonstrating the Model Context Protocol server integration for Golem CLI, completing bounty #1926. This allows AI assistants like Claude and Cursor to interact with Golem Cloud through a standardized protocol."

**Show:**
```bash
golem-cli --version
golem-cli mcp-server --help
```

---

## Scene 2: MCP Server Startup (30 seconds)

**Visual:** Terminal showing server startup in both modes

**Script:**
> "Golem CLI now supports MCP server mode in two transports: stdio for local AI assistants, and HTTP for web-based clients."

**Show:**
```bash
# Stdio mode (for Claude Desktop, Cursor)
golem-cli mcp-server start --transport stdio

# In another terminal - HTTP mode
golem-cli mcp-server start --transport http --port 3000
```

---

## Scene 3: Tool Discovery (45 seconds)

**Visual:** Python script showing JSON-RPC communication

**Script:**
> "The server exposes Golem CLI functionality as MCP tools. Let me show the protocol handshake and tool discovery."

**Show:**
```bash
python -c "
import subprocess, json, sys

# Start server
proc = subprocess.Popen(
    ['golem-cli', 'mcp-server', 'start', '--transport', 'stdio'],
    stdin=subprocess.PIPE, stdout=subprocess.PIPE, 
    stderr=subprocess.PIPE, text=True
)

# Initialize
init = {'jsonrpc':'2.0','id':1,'method':'initialize',
        'params':{'protocolVersion':'2024-11-05',
                  'capabilities':{},
                  'clientInfo':{'name':'demo','version':'1.0'}}}
proc.stdin.write(json.dumps(init) + '\n')
proc.stdin.flush()
print('INIT RESPONSE:', proc.stdout.readline())

# Send initialized notification
proc.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n')
proc.stdin.flush()

# List tools
tools = {'jsonrpc':'2.0','id':2,'method':'tools/list'}
proc.stdin.write(json.dumps(tools) + '\n')
proc.stdin.flush()
print('TOOLS:', json.dumps(json.loads(proc.stdout.readline()), indent=2))

proc.terminate()
"
```

**Expected Output:**
```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "result": {
    "tools": [
      {"name": "list_components", "description": "List all available components"},
      {"name": "list_agent_types", "description": "List all available agent types"},
      {"name": "list_workers", "description": "List all workers"}
    ]
  }
}
```

---

## Scene 4: Tool Execution (45 seconds)

**Visual:** Continue Python demo showing tool execution

**Script:**
> "Each tool executes actual Golem CLI commands and returns results in MCP format."

**Show:**
```bash
# Call list_components tool
python -c "
import subprocess, json

proc = subprocess.Popen(['golem-cli', 'mcp-server', 'start', '--transport', 'stdio'],
                       stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)

# Initialize (abbreviated for video)
proc.stdin.write(json.dumps({'jsonrpc':'2.0','id':1,'method':'initialize','params':{'protocolVersion':'2024-11-05','capabilities':{},'clientInfo':{'name':'d','version':'1'}}})+'\n')
proc.stdin.flush()
proc.stdout.readline()

proc.stdin.write(json.dumps({'jsonrpc':'2.0','method':'notifications/initialized'})+'\n')
proc.stdin.flush()

# Execute tool
call = {'jsonrpc':'2.0','id':3,'method':'tools/call',
        'params':{'name':'list_components','arguments':{}}}
proc.stdin.write(json.dumps(call)+'\n')
proc.stdin.flush()
print('RESULT:', json.dumps(json.loads(proc.stdout.readline()), indent=2))

proc.terminate()
"
```

---

## Scene 5: Cursor Integration (60 seconds)

**Visual:** Cursor IDE with MCP configuration and live interaction

**Script:**
> "Let me show this working in Cursor. I've configured the MCP server in my mcp.json file."

**Show:**
1. Open `~/.cursor/mcp.json`:
```json
{
  "mcpServers": {
    "golem-cli": {
      "command": "C:\\path\\to\\golem-cli.exe",
      "args": ["mcp-server", "start", "--transport", "stdio"]
    }
  }
}
```

2. Reload Cursor window

3. In Cursor chat, type: "@golem-cli list all components"

4. Show Cursor making MCP tool calls in the background

5. Show results displayed in chat

**Script continues:**
> "Notice how Cursor automatically discovered the golem-cli MCP server and can now interact with Golem Cloud through natural language."

---

## Scene 6: Test Suite Demo (45 seconds)

**Visual:** Terminal running test suite

**Script:**
> "The implementation includes comprehensive testing - E2E tests, manual protocol tests, and exploratory tests."

**Show:**
```bash
# Run E2E tests
python test_mcp_e2e_full.py
```

**Expected Output (abbreviated):**
```
============================================================
GOLEM CLI MCP SERVER - E2E TEST SUITE
============================================================

[TEST GROUP 1: Server Startup]
  [PASS] Server process starts

[TEST GROUP 2: Protocol Initialization]
  [PASS] Initialize returns result
  [PASS] Protocol version in response
  [PASS] Server info present

[TEST GROUP 3: Tool Discovery]
  [PASS] tools/list returns result
  [PASS] All tools present with schemas

[TEST GROUP 4: Tool Execution]
  [PASS] list_components executes
  [PASS] list_agent_types executes
  [PASS] list_workers executes

TEST SUMMARY: 24 passed, 0 failed ✅
```

---

## Scene 7: Documentation Showcase (20 seconds)

**Visual:** Quick scroll through documentation files

**Script:**
> "Full documentation is included covering setup, usage, testing, and troubleshooting."

**Show files:**
- `cli/golem-cli/MCP_SERVER.md`
- `MCP_TOOLS_DOCUMENTATION.md`
- `MCP_TESTING_GUIDE.md`
- `MCP_CLIENT_CONFIGURATION.md`

---

## Scene 8: Conclusion (20 seconds)

**Visual:** Terminal with summary statistics

**Script:**
> "To recap: MCP server fully integrated, 3 tools exposed, both stdio and HTTP transports working, 24 out of 24 E2E tests passing, and works with Claude and Cursor. Thanks for watching!"

**Show:**
```
✅ MCP Server: Implemented
✅ Transport Modes: Stdio & HTTP
✅ Tools Exposed: 3 (list_components, list_agent_types, list_workers)
✅ E2E Tests: 24/24 passed
✅ Manual Tests: All passed
✅ Exploratory Tests: 13/14 passed
✅ AI Assistants: Claude Desktop ✓, Cursor ✓
✅ Documentation: Complete
```

---

## Recording Tips

1. **Preparation:**
   - Clean terminal with no history
   - Fresh Cursor window
   - Close unnecessary apps
   - Test all commands beforehand

2. **Video Settings:**
   - 1920x1080 resolution
   - Clear terminal font (16-18pt)
   - Dark theme for readability
   - Screen recording at 30fps

3. **Audio:**
   - Clear microphone
   - Quiet environment
   - Speak slowly and clearly
   - Pause between scenes

4. **Editing:**
   - Add timestamps for navigation
   - Highlight important output
   - Add captions for key terms
   - Include links in description

---

## Video Description (for upload)

```
MCP Server Integration for Golem CLI - Bounty #1926 Complete

This video demonstrates the Model Context Protocol (MCP) server integration into Golem CLI, 
enabling AI assistants like Claude and Cursor to interact with Golem Cloud.

🔧 Features:
- Stdio transport for local AI assistants
- HTTP/SSE transport for web clients  
- 3 MCP tools exposing Golem functionality
- Full MCP protocol compliance
- Comprehensive test suite (97.4% pass rate)

📊 Test Results:
- E2E: 24/24 passed ✅
- Manual: All passed ✅
- Exploratory: 13/14 passed ⚠️

🔗 Links:
- Issue: https://github.com/golemcloud/golem/issues/1926
- Branch: feature/1926-mcp-server-mode
- Documentation: See BOUNTY_FINAL_REPORT.md

⏱️ Timestamps:
0:00 - Introduction
0:30 - Server Startup
1:00 - Tool Discovery
1:45 - Tool Execution
2:30 - Cursor Integration
3:30 - Test Suite
3:50 - Documentation
4:10 - Conclusion
```

---

## Alternative: Silent Demo Video

If you prefer a silent demo with text overlays instead of narration:

**Format:**
- Screen recording only
- Text overlays explaining each step
- Highlight commands before execution
- Show output with annotations
- Background music (optional, subtle)
- 3-4 minutes total

**Tools:**
- asciinema for terminal recording
- Video editor for overlays (iMovie, Premiere, DaVinci)
- Export as MP4, 1080p, 30fps
