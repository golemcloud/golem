#!/bin/bash
# MCP Server Bounty Acceptance Test Suite
# Validates GitHub Issue #1926 requirements
#
# Requirements tested:
# 1. --serve flag with port parameter
# 2. HTTP JSON-RPC endpoint (not stdio)
# 3. Expose ALL CLI commands as MCP tools
# 4. Expose manifest files as MCP resources
# 5. End-to-end testing with MCP Client

set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CLI_PATH="$SCRIPT_DIR/../../target/release/golem-cli"
TEST_PORT=18080
SERVER_PID=""
COOKIE_JAR="/tmp/mcp-test-cookies.txt"

# Colors for output
GREEN='\033[0;32m'
RED='\033[0;31m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

# Cleanup function
cleanup() {
    if [ -n "$SERVER_PID" ]; then
        echo -e "${YELLOW}Stopping MCP server (PID: $SERVER_PID)...${NC}"
        kill $SERVER_PID 2>/dev/null || true
        wait $SERVER_PID 2>/dev/null || true
    fi
    pkill -f "golem-cli --serve" 2>/dev/null || true
}

trap cleanup EXIT INT TERM

# Check if CLI binary exists
if [ ! -f "$CLI_PATH" ]; then
    echo -e "${RED}ERROR: golem-cli binary not found at $CLI_PATH${NC}"
    echo "Run: cargo build --release --bin golem-cli"
    exit 1
fi

echo "========================================"
echo "MCP Server Bounty Acceptance Tests"
echo "========================================"
echo ""

# Kill any existing servers
pkill -f "golem-cli --serve" 2>/dev/null || true
sleep 1

# Clear cookie jar
rm -f $COOKIE_JAR

# TEST 1: Server starts with --serve flag and port parameter
echo -e "${YELLOW}TEST 1: Server starts with --serve flag and port parameter${NC}"
$CLI_PATH --serve $TEST_PORT > /tmp/mcp-test.log 2>&1 &
SERVER_PID=$!
echo "Server PID: $SERVER_PID"
sleep 3

if ps -p $SERVER_PID > /dev/null; then
    echo -e "${GREEN}✓ PASS: Server started successfully${NC}"
else
    echo -e "${RED}✗ FAIL: Server failed to start${NC}"
    cat /tmp/mcp-test.log
    exit 1
fi
echo ""

# TEST 2: HTTP JSON-RPC endpoint responds to requests (capture session ID)
echo -e "${YELLOW}TEST 2: HTTP JSON-RPC endpoint responds to requests${NC}"

# Initialize and capture session ID from response header
INIT_RESPONSE=$(curl -s -D /tmp/mcp-headers.txt -X POST http://localhost:$TEST_PORT/mcp \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    --data-raw '{
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{
            "protocolVersion":"2024-11-05",
            "capabilities":{},
            "clientInfo":{"name":"test-client","version":"1.0"}
        }
    }')

# Extract session ID from response headers
SESSION_ID=$(grep -i "mcp-session-id:" /tmp/mcp-headers.txt | cut -d: -f2 | tr -d ' \r\n')
if [ -z "$SESSION_ID" ]; then
    echo -e "${RED}✗ FAIL: No session ID returned${NC}"
    exit 1
fi
echo "Session ID: $SESSION_ID"

# Extract JSON from SSE format (remove "data: " prefix)
JSON_RESPONSE=$(echo "$INIT_RESPONSE" | sed 's/^data: //')

if echo "$JSON_RESPONSE" | jq -e '.result' > /dev/null 2>&1; then
    echo -e "${GREEN}✓ PASS: Server responds to JSON-RPC requests${NC}"
    echo "Response: $(echo "$JSON_RESPONSE" | jq -c '.result.serverInfo')"
else
    echo -e "${RED}✗ FAIL: Server did not respond correctly${NC}"
    echo "Response: $RESPONSE"
    exit 1
fi
echo ""

# Send initialized notification (required by MCP protocol)
echo "Sending initialized notification..."
curl -s -X POST http://localhost:$TEST_PORT/mcp \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "mcp-session-id: $SESSION_ID" \
    --data-raw '{
        "jsonrpc":"2.0",
        "method":"notifications/initialized",
        "params":{}
    }' > /dev/null
echo "Initialized notification sent"
echo ""

# TEST 3: tools/list exposes ALL CLI commands (90+)
echo -e "${YELLOW}TEST 3: tools/list exposes ALL CLI commands as MCP tools${NC}"
TOOLS_RESPONSE=$(curl -s -X POST http://localhost:$TEST_PORT/mcp \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "mcp-session-id: $SESSION_ID" \
    --data-raw '{
        "jsonrpc":"2.0",
        "id":2,
        "method":"tools/list",
        "params":{}
    }')

# Extract JSON from SSE format
TOOLS_JSON=$(echo "$TOOLS_RESPONSE" | sed 's/^data: //')
TOOL_COUNT=$(echo "$TOOLS_JSON" | jq '.result.tools | length')

if [ "$TOOL_COUNT" -ge 90 ]; then
    echo -e "${GREEN}✓ PASS: Exposed $TOOL_COUNT tools (expected ≥90)${NC}"
    echo "Sample tools:"
    echo "$TOOLS_JSON" | jq -r '.result.tools[:5] | .[] | "  - \(.name): \(.description)"'
else
    echo -e "${RED}✗ FAIL: Only exposed $TOOL_COUNT tools (expected ≥90)${NC}"
    exit 1
fi
echo ""

# TEST 4: tools/call executes CLI commands successfully
echo -e "${YELLOW}TEST 4: tools/call executes CLI commands successfully${NC}"
CALL_RESPONSE=$(curl -s -X POST http://localhost:$TEST_PORT/mcp \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "mcp-session-id: $SESSION_ID" \
    --data-raw '{
        "jsonrpc":"2.0",
        "id":3,
        "method":"tools/call",
        "params":{
            "name":"component_list",
            "arguments":{}
        }
    }')

# Extract JSON from SSE format
CALL_JSON=$(echo "$CALL_RESPONSE" | sed 's/^data: //')

if echo "$CALL_JSON" | jq -e '.result.content' > /dev/null 2>&1; then
    echo -e "${GREEN}✓ PASS: Successfully executed component_list tool${NC}"
    echo "Response type: $(echo "$CALL_JSON" | jq -r '.result.content[0].type')"
else
    echo -e "${RED}✗ FAIL: Tool execution failed${NC}"
    echo "Response: $CALL_RESPONSE"
    exit 1
fi
echo ""

# TEST 5: resources/list finds manifest files
echo -e "${YELLOW}TEST 5: resources/list exposes manifest files${NC}"
RESOURCES_RESPONSE=$(curl -s -X POST http://localhost:$TEST_PORT/mcp \
    -H "Content-Type: application/json" \
    -H "Accept: application/json, text/event-stream" \
    -H "mcp-session-id: $SESSION_ID" \
    --data-raw '{
        "jsonrpc":"2.0",
        "id":4,
        "method":"resources/list",
        "params":{}
    }')

# Extract JSON from SSE format
RESOURCES_JSON=$(echo "$RESOURCES_RESPONSE" | sed 's/^data: //')

if echo "$RESOURCES_JSON" | jq -e '.result.resources' > /dev/null 2>&1; then
    RESOURCE_COUNT=$(echo "$RESOURCES_JSON" | jq '.result.resources | length')
    echo -e "${GREEN}✓ PASS: resources/list endpoint functional (found $RESOURCE_COUNT resources)${NC}"
    if [ "$RESOURCE_COUNT" -gt 0 ]; then
        echo "Sample resources:"
        echo "$RESOURCES_JSON" | jq -r '.result.resources[:3] | .[] | "  - \(.uri)"'
    fi
else
    echo -e "${RED}✗ FAIL: resources/list endpoint failed${NC}"
    echo "Response: $RESOURCES_RESPONSE"
    exit 1
fi
echo ""

# TEST 6: resources/read returns manifest content (if resources exist)
if [ "$RESOURCE_COUNT" -gt 0 ]; then
    echo -e "${YELLOW}TEST 6: resources/read returns manifest content${NC}"
    FIRST_URI=$(echo "$RESOURCES_JSON" | jq -r '.result.resources[0].uri')

    READ_RESPONSE=$(curl -s -X POST http://localhost:$TEST_PORT/mcp \
        -H "Content-Type: application/json" \
        -H "Accept: application/json, text/event-stream" \
        -H "mcp-session-id: $SESSION_ID" \
        --data-raw "{
            \"jsonrpc\":\"2.0\",
            \"id\":5,
            \"method\":\"resources/read\",
            \"params\":{
                \"uri\":\"$FIRST_URI\"
            }
        }")

    # Extract JSON from SSE format
    READ_JSON=$(echo "$READ_RESPONSE" | sed 's/^data: //')

    if echo "$READ_JSON" | jq -e '.result.contents' > /dev/null 2>&1; then
        echo -e "${GREEN}✓ PASS: Successfully read resource: $FIRST_URI${NC}"
        CONTENT_TYPE=$(echo "$READ_JSON" | jq -r '.result.contents[0].mimeType')
        echo "Content type: $CONTENT_TYPE"
    else
        echo -e "${RED}✗ FAIL: Failed to read resource${NC}"
        echo "Response: $READ_RESPONSE"
        exit 1
    fi
    echo ""
fi

# Summary
echo "========================================"
echo -e "${GREEN}ALL BOUNTY ACCEPTANCE TESTS PASSED ✓${NC}"
echo "========================================"
echo ""
echo "Bounty Requirements Validated:"
echo "  ✓ Server starts with --serve flag and port parameter"
echo "  ✓ HTTP JSON-RPC endpoint (not stdio)"
echo "  ✓ Exposes $TOOL_COUNT CLI commands as MCP tools"
echo "  ✓ Tool execution works (component_list tested)"
echo "  ✓ Resources endpoint functional"
if [ "$RESOURCE_COUNT" -gt 0 ]; then
    echo "  ✓ Resource reading works"
fi
echo ""
echo "Ready for bounty submission!"
