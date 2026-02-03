# Golem CLI MCP Server - Comprehensive Test Report

**Date:** 2026-01-31
**Tester:** Automated Test Suite + Manual Testing

---

## Executive Summary

| Test Category | Passed | Failed | Notes |
|---------------|--------|--------|-------|
| Unit Tests | N/A | N/A | Windows long path issue prevents compilation |
| E2E Tests | 14 | 3 | Tool execution has stdout pollution bug |
| Manual Protocol Tests | 4 | 2 | Same bug as E2E |
| Integration Tests | N/A | N/A | Blocked by unit test compilation |
| Exploratory Tests | 13 | 1 | Server crashes on invalid JSON |

---

## Test Results

### 1. Unit Tests (cargo test)

**Status:** ❌ BLOCKED

**Issue:** Windows long path limitation prevents cargo from cloning wasmtime dependency:
```
error: path too long: 'C:/Users/.../wasmtime-ea44c988131055b2/23787cf/tests/disas/load-store/aarch64/load_store_dynamic_kind_i32_index_0_guard_no_spectre_i32_access_0x1000_offset.wat'
```

**Workaround:** Enable Windows long paths or run tests on Linux/macOS.

---

### 2. E2E Tests (Python)

**Test Script:** `test_mcp_e2e_full.py`

| Test | Status | Details |
|------|--------|---------|
| Server process starts | ✅ PASS | Process spawns correctly |
| Initialize returns result | ✅ PASS | JSON-RPC response received |
| Protocol version in response | ✅ PASS | "2024-11-05" |
| Server info present | ✅ PASS | {"name":"rmcp","version":"0.12.0"} |
| Initialized notification sent | ✅ PASS | No errors |
| tools/list returns result | ✅ PASS | Valid JSON-RPC response |
| Tools array present | ✅ PASS | Array of 3 tools |
| At least 1 tool available | ✅ PASS | 3 tools found |
| list_components tool exists | ✅ PASS | Tool registered |
| list_agent_types tool exists | ✅ PASS | Tool registered |
| list_workers tool exists | ✅ PASS | Tool registered |
| Tool 'list_components' has schema | ✅ PASS | inputSchema present |
| Tool 'list_agent_types' has schema | ✅ PASS | inputSchema present |
| Tool 'list_workers' has schema | ✅ PASS | inputSchema present |
| Tool execution | ❌ FAIL | **BUG: stdout pollution** |
| Error handling | ❌ FAIL | Same bug |
| Sequential requests | ❌ FAIL | Same bug |

---

### 3. Manual Protocol Tests

**Test Script:** `test_mcp_manual.py`

#### ✅ Initialize Connection
```json
REQUEST: {"jsonrpc":"2.0","id":1,"method":"initialize",...}
RESPONSE: {"jsonrpc":"2.0","id":1,"result":{"protocolVersion":"2024-11-05","capabilities":{},"serverInfo":{"name":"rmcp","version":"0.12.0"}}}
```

#### ✅ List Available Tools
```json
REQUEST: {"jsonrpc":"2.0","id":2,"method":"tools/list"}
RESPONSE: {"jsonrpc":"2.0","id":2,"result":{"tools":[
  {"name":"list_components","description":"List all available components","inputSchema":{"properties":{},"type":"object"}},
  {"name":"list_workers","description":"List all workers across all components","inputSchema":{"properties":{},"type":"object"}},
  {"name":"list_agent_types","description":"List all available agent types","inputSchema":{"properties":{},"type":"object"}}
]}}
```

#### ❌ Call list_components Tool
```
REQUEST: {"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"list_components"}}
RESPONSE: Selected app: -, env: -, server: local - builtin (http://localhost:9881), profile: local
ERROR: Not valid JSON-RPC - stdout pollution from log_action()
```

#### ❌ Call list_agent_types Tool
```
REQUEST: {"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"list_agent_types"}}
RESPONSE: error: The requested command requires an environment from an application manifest...
ERROR: CLI error message instead of JSON-RPC error
```

---

---

### 4. Exploratory Tests (Python)

**Test Script:** `test_mcp_exploratory.py`

| Test | Status | Details |
|------|--------|---------|
| Start 3 concurrent servers | PASS | Multiple instances work |
| tools/list on all 3 servers | PASS | No resource conflicts |
| 20 rapid requests | PASS | 0.00s - very fast |
| Server survives invalid JSON | FAIL | **Server crashes on malformed input** |
| tools/call without name returns error | PASS | Proper error handling |
| tools/call with empty name returns error | PASS | Proper error handling |
| Unknown method 'unknown/method' handled | PASS | Returns error |
| Unknown method 'tools/unknown' handled | PASS | Returns error |
| Unknown method 'resources/list' handled | PASS | Returns error |
| Unknown method 'prompts/list' handled | PASS | Returns error |
| Unknown method '' handled | PASS | Returns error |
| Unknown method 'special-chars' handled | PASS | Returns error |
| Large argument handled | PASS | 10KB payload OK |
| Server exits gracefully | PASS | Clean shutdown |

---

## Critical Bugs Found

### BUG 1: Server Crashes on Invalid JSON Input

**Severity:** Medium  
**Impact:** Server terminates when receiving malformed JSON, requiring restart

---

## Critical Bug Found (BUG 2)

### BUG: Stdout Pollution During Tool Execution

**Location:** `cli/golem-cli/src/context.rs` line 681-689

**Description:** When MCP tools execute, they trigger lazy context initialization which calls `log_selection()`. This function uses `log_action()` which outputs to stdout, polluting the JSON-RPC response stream.

**Code Path:**
1. `tools/call` → `list_components()`
2. → `self.ctx.component_handler().cmd_list_components()`
3. → Context lazy initialization
4. → `log_selection()` → `log_action("Selected", ...)`
5. → Outputs to stdout (should go to stderr in MCP stdio mode)

**Root Cause:** The `set_log_output(Output::Stderr)` is called in `mcp_server.rs` line 44, but context initialization happens lazily during tool execution, and the log output setting may not be properly propagated or the context does its own stdout writes.

**Impact:** 
- MCP clients receive invalid JSON-RPC responses
- Tool calls appear to fail even when they succeed
- Breaks integration with Claude Desktop, Cursor, and other MCP clients

**FIX APPLIED (2026-01-31):**
- In `command_handler/mod.rs`: Changed `set_log_output(Output::Stderr)` to `set_log_output(Output::None)` when `is_mcp_stdio` is true
- This completely suppresses all CLI logging during MCP stdio mode, preventing stdout pollution

---

## Available MCP Tools

| Tool Name | Description | Status |
|-----------|-------------|--------|
| `list_components` | List all available components | Works (with bug) |
| `list_agent_types` | List all available agent types | Works (with bug) |
| `list_workers` | List all workers across all components | Works (with bug) |

---

## Recommendations

1. **HIGH PRIORITY:** Fix stdout pollution bug - this blocks all MCP client integration
2. **MEDIUM:** Enable Windows long paths for development or provide Linux/WSL instructions
3. **LOW:** Add more comprehensive error handling for missing configuration
4. **LOW:** Add parameterized tools (get_worker, get_component, etc.)

---

## Test Environment

- **OS:** Windows 10
- **Rust:** 1.93.0 (stable)
- **golem-cli:** v1.4.0-rc5-466-g69c2e6c3c
- **MCP Protocol:** 2024-11-05
- **RMCP Library:** 0.12.0
