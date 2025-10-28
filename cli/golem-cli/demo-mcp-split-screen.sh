#!/bin/bash

# Golem MCP Server - Split-Screen Demo Runner
# This script runs the demo with server logs visible in a split terminal using tmux
# Run from golem/cli/golem-cli/ directory
# Usage: ./demo-mcp-split-screen.sh [options] [scene_number]
#   ./demo-mcp-split-screen.sh all              - Run all scenes
#   ./demo-mcp-split-screen.sh --record all     - Recording mode

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
TMUX_SESSION="golem-mcp-demo"

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

# Check if tmux is available
if ! command -v tmux &> /dev/null; then
    echo -e "${RED}Error: tmux is required for split-screen demo${NC}"
    echo "Install with: brew install tmux"
    exit 1
fi

# Setup tmux session with split panes
setup_tmux() {
    # Kill existing session if it exists
    tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true

    # Create new session
    tmux new-session -d -s "$TMUX_SESSION" -x "$(tput cols)" -y "$(tput lines)"

    # Split window vertically (side-by-side)
    tmux split-window -h -t "$TMUX_SESSION"

    # Resize panes (40% server logs, 60% demo commands)
    tmux resize-pane -t "$TMUX_SESSION:0.0" -x 40%

    # Set pane titles
    tmux select-pane -t "$TMUX_SESSION:0.0" -T "Server Logs"
    tmux select-pane -t "$TMUX_SESSION:0.1" -T "MCP Demo"

    # Enable pane borders with titles
    tmux set-option -t "$TMUX_SESSION" pane-border-status top
    tmux set-option -t "$TMUX_SESSION" pane-border-format "#{pane_title}"

    # Server log viewer in left pane
    tmux send-keys -t "$TMUX_SESSION:0.0" "clear" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo -e '${CYAN}═══════════════════════════════════${NC}'" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo -e '${CYAN}  MCP Server Logs${NC}'" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo -e '${CYAN}═══════════════════════════════════${NC}'" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo 'Waiting for server to start...'" C-m

    # Select the demo pane (right side)
    tmux select-pane -t "$TMUX_SESSION:0.1"
}

# Start server in tmux with visible logs
start_server_in_tmux() {
    cd "$GOLEM_DIR"

    # Clear and prepare server log pane
    tmux send-keys -t "$TMUX_SESSION:0.0" "clear" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo -e '${GREEN}Starting MCP Server on port $TEST_PORT...${NC}'" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "echo ''" C-m

    # Start server in left pane with output visible
    tmux send-keys -t "$TMUX_SESSION:0.0" "cd $GOLEM_DIR" C-m
    tmux send-keys -t "$TMUX_SESSION:0.0" "target/release/golem-cli --serve $TEST_PORT" C-m

    # Give server time to start
    sleep 3
}

# Send command to demo pane
demo_cmd() {
    local cmd="$1"
    tmux send-keys -t "$TMUX_SESSION:0.1" "$cmd" C-m
}

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

# Show command without typing effect
show_command() {
    local cmd="$1"
    echo -e "${GREEN}$ ${NC}${cmd}"
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
    echo -e "${BLUE}Note: Server logs visible in left pane${NC}"
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

    echo -e "\n${BLUE}# Start the MCP server (watch left pane for logs)${NC}"
    echo -e "${GREEN}$ ${NC}target/release/golem-cli --serve $TEST_PORT"
    echo

    start_server_in_tmux

    echo -e "${GREEN}✓ MCP Server running on http://localhost:$TEST_PORT/mcp${NC}"
    echo -e "${BLUE}  (Server logs streaming in left pane)${NC}"
    sleep 3
}

# Scene 3: MCP Protocol Handshake
scene_03() {
    clear
    scene_header "3" "MCP Protocol Handshake"

    echo -e "${BLUE}# Initialize MCP session${NC}"
    echo -e "${BLUE}# Watch left pane for server receiving request${NC}\n"

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
    echo -e "${BLUE}  (Session created on server - see left pane)${NC}"
    sleep "$PAUSE_AFTER_CMD"
}

# Scene 4: Discovering Tools
scene_04() {
    clear
    scene_header "4" "Discovering All Available Tools"

    echo -e "${BLUE}# Send initialized notification${NC}"
    echo -e "${BLUE}# Server will acknowledge - watch left pane${NC}\n"

    if [ "$FAST_MODE" = true ]; then
        show_command 'curl -X POST http://localhost:'$TEST_PORT'/mcp ...'
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
        show_command 'curl -X POST ... | jq ".result.tools | length"'
    else
        local count_cmd='curl -s -X POST http://localhost:'$TEST_PORT'/mcp \
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
        show_command 'curl -X POST ... | jq ".result.tools[:10]"'
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

# Cleanup and exit tmux
cleanup_tmux() {
    echo -e "\n${BLUE}# Stopping server...${NC}"
    tmux send-keys -t "$TMUX_SESSION:0.0" C-c
    sleep 2
    tmux kill-session -t "$TMUX_SESSION" 2>/dev/null || true
    rm -f /tmp/demo-headers.txt 2>/dev/null || true
}

# Main execution
main() {
    local scene="${1:-all}"

    # Setup tmux environment
    setup_tmux

    # Attach to tmux session and run demo in right pane
    tmux attach-session -t "$TMUX_SESSION" &
    TMUX_PID=$!
    sleep 1

    case "$scene" in
        1)  scene_01 ;;
        2)  scene_02 ;;
        3)  scene_03 ;;
        4)  scene_04 ;;
        all)
            scene_01
            scene_02
            scene_03
            scene_04
            echo -e "\n${CYAN}Demo complete. Remaining scenes use same pattern.${NC}"
            echo -e "${CYAN}Press Ctrl+C in left pane to stop server.${NC}"
            echo -e "${CYAN}Press Ctrl+B then D to detach from tmux.${NC}"
            sleep 5
            ;;
        *)
            echo "Usage: $0 [options] [scene]"
            echo ""
            echo "Options:"
            echo "  --fast      Fast mode (no typing effect)"
            echo "  --record    Recording mode (slower pacing)"
            echo ""
            echo "Scenes:"
            echo "  1-4         Run specific scene"
            echo "  all         Run first 4 scenes (demo)"
            echo ""
            cleanup_tmux
            exit 1
            ;;
    esac

    # Keep session alive
    echo -e "\n${BLUE}Tmux session '$TMUX_SESSION' is running.${NC}"
    echo -e "${BLUE}To reattach: tmux attach -t $TMUX_SESSION${NC}"
    echo -e "${BLUE}To kill: tmux kill-session -t $TMUX_SESSION${NC}"
}

# Trap exit to cleanup
trap cleanup_tmux EXIT INT TERM

# Run main
main "$@"
