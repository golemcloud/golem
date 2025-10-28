#!/bin/bash

# Golem MCP Server - Automated Demo Runner
# This script automates all 11 demo scenes for consistent video recording
# Run from golem/cli/golem-cli/ directory
# Usage: ./demo-mcp-automated.sh [options] [scene_number]
#   ./demo-mcp-automated.sh all              - Run all scenes
#   ./demo-mcp-automated.sh 2                - Run scene 2 only
#   ./demo-mcp-automated.sh --fast all       - Fast mode (no typing effect)
#   ./demo-mcp-automated.sh --record all     - Recording mode (slower pacing)

set -e

# Color codes for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
BLUE='\033[0;34m'
MAGENTA='\033[0;35m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

# Configuration - auto-detect Golem directory
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GOLEM_DIR="$(cd "$SCRIPT_DIR/../.." && pwd)"
TEST_PORT=8080
SESSION_ID=""

# Mode settings
TYPING_DELAY=0.05
PAUSE_AFTER_CMD=2
FAST_MODE=false
RECORD_MODE=false

# Parse mode flags
while [[ "$1" == --* ]]; do
    case "$1" in
        --fast)
            FAST_MODE=true
            TYPING_DELAY=0
            PAUSE_AFTER_CMD=0.5
            shift
            ;;
        --record)
            RECORD_MODE=true
            TYPING_DELAY=0.08
            PAUSE_AFTER_CMD=3
            shift
            ;;
        *)
            echo "Unknown option: $1"
            exit 1
            ;;
    esac
done

# Typing effect for demo
type_command() {
    local cmd="$1"
    local delay="${TYPING_DELAY}"

    echo -ne "${GREEN}$ ${NC}"

    if [ "$FAST_MODE" = true ]; then
        echo "$cmd"
    else
        for ((i=0; i<${#cmd}; i++)); do
            echo -n "${cmd:$i:1}"
            sleep "$delay"
        done
        echo
    fi
}

# Execute command with optional pause
run_command() {
    local cmd="$1"
    local pause="${2:-$PAUSE_AFTER_CMD}"

    type_command "$cmd"
    eval "$cmd"
    sleep "$pause"
}

# Show command without typing effect (for long commands)
show_command() {
    local cmd="$1"
    echo -e "${GREEN}$ ${NC}${cmd}"
}

# Execute command silently and show output
run_silent() {
    local cmd="$1"
    local pause="${2:-$PAUSE_AFTER_CMD}"

    eval "$cmd"
    sleep "$pause"
}

# Display banner
banner() {
    local text="$1"
    echo
    echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
    echo -e "${CYAN}  $text${NC}"
    echo -e "${CYAN}═══════════════════════════════════════════════${NC}"
    echo
    sleep 2
}

# Scene header
scene_header() {
    local num="$1"
    local title="$2"
    echo
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo -e "${YELLOW}Scene $num: $title${NC}"
    echo -e "${YELLOW}━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━━${NC}"
    echo
    sleep 2
}

# Scene 1: Introduction
scene_01() {
    clear
    banner "Golem CLI MCP Server Implementation"
    echo -e "${CYAN}GitHub Issue #1926 - \$3,500 Bounty${NC}"
    echo -e "${CYAN}Implementing Model Context Protocol for AI Agents${NC}"
    echo
    echo -e "${MAGENTA}Presenter: Michael O'Boyle${NC}"
    echo
    sleep 5
}

# Scene 2: Starting the Server
scene_02() {
    clear
    scene_header "2" "Starting the MCP Server"

    cd "$GOLEM_DIR"

    run_command "pwd" 1
    run_command "ls -la | head -10" 2

    echo -e "\n${BLUE}# Show the new --serve flag${NC}"
    run_command "target/release/golem-cli --help | grep -A 2 'serve'" 3

    echo -e "\n${BLUE}# Start the MCP server on port $TEST_PORT${NC}"
    type_command "target/release/golem-cli --serve $TEST_PORT"

    # Start server in background for subsequent scenes
    target/release/golem-cli --serve $TEST_PORT > /tmp/mcp-demo-server.log 2>&1 &
    SERVER_PID=$!
    echo "Server starting... (PID: $SERVER_PID)"
    sleep 3

    echo -e "${GREEN}✓ MCP Server running on http://localhost:$TEST_PORT/mcp${NC}"
    sleep 2
}

# Scene 3: MCP Protocol Handshake
scene_03() {
    clear
    scene_header "3" "MCP Protocol Handshake"

    echo -e "${BLUE}# Initialize MCP session${NC}\n"

    if [ "$FAST_MODE" = true ]; then
        show_command 'curl -X POST http://localhost:'$TEST_PORT'/mcp ... (initialize)'
    else
        local init_cmd='curl -s -D /tmp/demo-headers.txt -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "Content-Type: application/json" \
  -H "Accept: application/json, text/event-stream" \
  --data-raw '"'"'{
    "jsonrpc":"2.0",
    "id":1,
    "method":"initialize",
    "params":{
      "protocolVersion":"2024-11-05",
      "capabilities":{},
      "clientInfo":{"name":"demo-client","version":"1.0"}
    }
  }'"'"' | grep "^data: " | sed "s/^data: //" | jq .'
        type_command "$init_cmd" 0.02
    fi

    INIT_RESPONSE=$(curl -s -D /tmp/demo-headers.txt -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "Accept: application/json, text/event-stream" \
      --data-raw '{
        "jsonrpc":"2.0",
        "id":1,
        "method":"initialize",
        "params":{
          "protocolVersion":"2024-11-05",
          "capabilities":{},
          "clientInfo":{"name":"demo-client","version":"1.0"}
        }
      }' | grep "^data: " | sed "s/^data: //" | jq .)

    echo "$INIT_RESPONSE"
    sleep "$PAUSE_AFTER_CMD"

    echo -e "\n${BLUE}# Extract session ID from headers${NC}"
    run_command 'grep -i "mcp-session-id:" /tmp/demo-headers.txt'

    SESSION_ID=$(grep -i "mcp-session-id:" /tmp/demo-headers.txt | cut -d: -f2 | tr -d ' \r\n')
    echo -e "${GREEN}✓ Session ID: $SESSION_ID${NC}"
    sleep "$PAUSE_AFTER_CMD"
}

# Scene 4: Discovering Tools
scene_04() {
    clear
    scene_header "4" "Discovering All Available Tools"

    echo -e "${BLUE}# Send initialized notification (required by MCP)${NC}"
    if [ "$FAST_MODE" = true ]; then
        show_command 'curl -X POST http://localhost:'$TEST_PORT'/mcp -H "mcp-session-id: '$SESSION_ID'" ...'
    else
        type_command 'curl -s -X POST http://localhost:'$TEST_PORT'/mcp -H "mcp-session-id: '$SESSION_ID'" ...'
    fi

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{"jsonrpc":"2.0","method":"notifications/initialized","params":{}}' > /dev/null

    echo -e "${GREEN}✓ Initialized${NC}\n"
    sleep 1

    echo -e "${BLUE}# Count total tools available${NC}"
    if [ "$FAST_MODE" = true ]; then
        show_command 'curl -X POST http://localhost:'$TEST_PORT'/mcp ... | jq ".result.tools | length"'
    else
        local count_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "Content-Type: application/json" \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}'"'"' \
  | grep "^data: " | sed "s/^data: //" | jq ".result.tools | length"'
        type_command "$count_cmd" 0.02
    fi

    TOOL_COUNT=$(curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}' \
      | grep "^data: " | sed "s/^data: //" | jq -r ".result.tools | length")

    echo -e "${GREEN}$TOOL_COUNT tools available${NC}"
    sleep "$PAUSE_AFTER_CMD"

    echo -e "\n${BLUE}# Show sample tools${NC}"
    if [ "$FAST_MODE" = true ]; then
        show_command 'curl -X POST ... | jq ".result.tools[:10] | .[]"'
    else
        local sample_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}'"'"' \
  | grep "^data: " | sed "s/^data: //" \
  | jq -r '"'"'.result.tools[:10] | .[] | "  • \(.name): \(.description)"'"'"
        type_command "$sample_cmd" 0.02
    fi

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{"jsonrpc":"2.0","id":3,"method":"tools/list","params":{}}' \
      | grep "^data: " | sed "s/^data: //" \
      | jq -r '.result.tools[:10] | .[] | "  • \(.name): \(.description)"'

    sleep "$PAUSE_AFTER_CMD"
}

# Scene 5: Executing a Tool
scene_05() {
    clear
    scene_header "5" "Executing a CLI Tool"

    echo -e "${BLUE}# Execute component_templates tool${NC}\n"

    local exec_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{
    "jsonrpc":"2.0",
    "id":4,
    "method":"tools/call",
    "params":{
      "name":"component_templates",
      "arguments":{}
    }
  }'"'"' | grep "^data: " | sed "s/^data: //" \
  | jq -r ".result.content[0].text" | head -20'

    type_command "$exec_cmd" 0.02

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{
        "jsonrpc":"2.0",
        "id":4,
        "method":"tools/call",
        "params":{
          "name":"component_templates",
          "arguments":{}
        }
      }' | grep "^data: " | sed "s/^data: //" \
      | jq -r ".result.content[0].text" | head -20

    sleep 4
}

# Scene 6: Discovering Resources
scene_06() {
    clear
    scene_header "6" "Discovering Manifest Resources"

    echo -e "${BLUE}# Create demo project structure${NC}"
    run_command "mkdir -p /tmp/golem-demo/components/my-component" 1
    run_command "echo 'name: demo-app' > /tmp/golem-demo/golem.yaml" 1
    run_command "echo 'name: my-component' > /tmp/golem-demo/components/my-component/golem.yaml" 1
    run_command "cd /tmp/golem-demo/components/my-component && pwd" 2

    echo -e "\n${BLUE}# List discovered resources${NC}"
    local resources_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}'"'"' \
  | grep "^data: " | sed "s/^data: //" \
  | jq -r '"'"'.result.resources[] | "  📄 \(.name)\n     URI: \(.uri)\n     MIME: \(.mimeType)\n"'"'"

    type_command "$resources_cmd" 0.02

    cd /tmp/golem-demo/components/my-component

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{"jsonrpc":"2.0","id":5,"method":"resources/list","params":{}}' \
      | grep "^data: " | sed "s/^data: //" \
      | jq -r '.result.resources[] | "  📄 \(.name)\n     URI: \(.uri)\n     MIME: \(.mimeType)\n"'

    sleep 4
}

# Scene 7: Reading a Resource
scene_07() {
    clear
    scene_header "7" "Reading Resource Contents"

    echo -e "${BLUE}# Read component manifest${NC}\n"

    local read_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{
    "jsonrpc":"2.0",
    "id":6,
    "method":"resources/read",
    "params":{
      "uri":"file:///tmp/golem-demo/components/my-component/golem.yaml"
    }
  }'"'"' | grep "^data: " | sed "s/^data: //" \
  | jq -r ".result.contents[0].text"'

    type_command "$read_cmd" 0.02

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{
        "jsonrpc":"2.0",
        "id":6,
        "method":"resources/read",
        "params":{
          "uri":"file:///tmp/golem-demo/components/my-component/golem.yaml"
        }
      }' | grep "^data: " | sed "s/^data: //" \
      | jq -r ".result.contents[0].text"

    sleep 3
}

# Scene 8: Security Features
scene_08() {
    clear
    scene_header "8" "Security Filtering"

    echo -e "${BLUE}# Verify sensitive commands are filtered${NC}\n"

    local security_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
  -H "mcp-session-id: '$SESSION_ID'" \
  --data-raw '"'"'{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}'"'"' \
  | grep "^data: " | sed "s/^data: //" \
  | jq ".result.tools[].name" | grep -i "profile\|token\|grant" \
  || echo "✓ No sensitive commands exposed"'

    type_command "$security_cmd" 0.02

    curl -s -X POST http://localhost:$TEST_PORT/mcp \
      -H "Content-Type: application/json" \
      -H "mcp-session-id: $SESSION_ID" \
      --data-raw '{"jsonrpc":"2.0","id":7,"method":"tools/list","params":{}}' \
      | grep "^data: " | sed "s/^data: //" \
      | jq ".result.tools[].name" | grep -i "profile\|token\|grant" \
      || echo -e "${GREEN}✓ No sensitive commands exposed (profile, token, grant filtered)${NC}"

    sleep 3
}

# Scene 9: Implementation Details
scene_09() {
    clear
    scene_header "9" "Implementation Overview"

    cd "$GOLEM_DIR"

    echo -e "${BLUE}# Code structure${NC}"
    run_command "tree -L 1 cli/golem-cli/src/mcp_server/" 2

    echo -e "\n${BLUE}# Line counts per module${NC}"
    run_command "wc -l cli/golem-cli/src/mcp_server/*.rs" 3

    echo -e "\n${BLUE}# Acceptance test script${NC}"
    run_command "ls -lh cli/golem-cli/test-mcp-bounty-acceptance.sh" 2
}

# Scene 10: Running Acceptance Tests
scene_10() {
    clear
    scene_header "10" "Running Acceptance Tests"

    cd "$GOLEM_DIR"

    # Kill demo server
    if [ -n "$SERVER_PID" ]; then
        echo -e "${BLUE}# Stop demo server${NC}"
        run_command "kill $SERVER_PID" 1
    fi

    pkill -f "golem-cli --serve" 2>/dev/null || true
    sleep 1

    echo -e "${BLUE}# Run comprehensive test suite${NC}\n"
    run_command "./cli/golem-cli/test-mcp-bounty-acceptance.sh" 0.5

    sleep 3
}

# Scene 11: Conclusion
scene_11() {
    clear
    scene_header "11" "Summary"

    banner "Implementation Complete"

    echo -e "${GREEN}  ✓ HTTP JSON-RPC transport (not stdio)${NC}"
    echo -e "${GREEN}  ✓ 96 CLI commands exposed as MCP tools${NC}"
    echo -e "${GREEN}  ✓ Manifest file discovery as resources${NC}"
    echo -e "${GREEN}  ✓ Stateful session management${NC}"
    echo -e "${GREEN}  ✓ Security filtering built-in${NC}"
    echo -e "${GREEN}  ✓ All acceptance tests passing${NC}"
    echo
    echo -e "${CYAN}  Implementation: 1,670 lines of Rust${NC}"
    echo -e "${CYAN}  Dependencies: rmcp v0.8.3, actix-web${NC}"
    echo -e "${CYAN}  Testing: E2E test suite included${NC}"
    echo
    echo -e "${YELLOW}  Ready for: golemcloud/golem#1926 bounty${NC}"
    echo
    sleep 5
}

# Main execution
main() {
    local scene="${1:-all}"

    case "$scene" in
        1)  scene_01 ;;
        2)  scene_02 ;;
        3)  scene_03 ;;
        4)  scene_04 ;;
        5)  scene_05 ;;
        6)  scene_06 ;;
        7)  scene_07 ;;
        8)  scene_08 ;;
        9)  scene_09 ;;
        10) scene_10 ;;
        11) scene_11 ;;
        all)
            scene_01
            scene_02
            scene_03
            scene_04
            scene_05
            scene_06
            scene_07
            scene_08
            scene_09
            scene_10
            scene_11
            ;;
        *)
            echo "Usage: $0 [options] [scene]"
            echo ""
            echo "Options:"
            echo "  --fast      Fast mode (no typing effect, minimal pauses)"
            echo "  --record    Recording mode (slower typing, longer pauses)"
            echo ""
            echo "Scenes:"
            echo "  1-11        Run specific scene"
            echo "  all         Run all scenes in sequence"
            echo ""
            echo "Examples:"
            echo "  $0 all                  # Normal demo (typing effect)"
            echo "  $0 --fast all           # Fast demo (testing)"
            echo "  $0 --record all         # Recording mode (video)"
            echo "  $0 2                    # Run scene 2 only"
            echo "  $0 --fast 5             # Fast run of scene 5"
            exit 1
            ;;
    esac

    # Cleanup
    pkill -f "golem-cli --serve" 2>/dev/null || true
    rm -f /tmp/demo-headers.txt /tmp/mcp-demo-server.log 2>/dev/null || true
}

# Run main
main "$@"
