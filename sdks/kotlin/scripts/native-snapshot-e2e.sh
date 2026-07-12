#!/usr/bin/env bash
# Host-level snapshot round-trip: prove the worker-executor calls guest save-snapshot (old
# revision) + load-snapshot (new revision) during a manual update, carrying typed agent state
# across a revision bump AND a server restart. Mirrors hot_update.rs:agent_can_be_invoked_after_
# manual_snapshot_update_and_restart. Requires the Snapshotted<S> CounterAgent (Task 10) and the
# republished toolchain (Task 9). GOLEM_BIN/GOLEM_CLI_BIN override the built binaries.
set -uo pipefail

WORKDIR="${1:?workdir}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GOLEM="${GOLEM_BIN:-$(cd "$SCRIPT_DIR/../../.." && pwd)/target/debug/golem}"
GOLEM_CLI="${GOLEM_CLI_BIN:-$(cd "$SCRIPT_DIR/../../.." && pwd)/target/debug/golem-cli}"
AGENT='CounterAgent("c1")'
SERVER_PID=""
fail() { echo "FAIL: $*"; exit 1; }

rm -rf "$WORKDIR"; mkdir -p "$WORKDIR/app-parent" "$WORKDIR/server-data"
(cd "$WORKDIR/app-parent" && "$GOLEM_CLI" new --template kotlin --component-name example:counter --yes app)
APP="$WORKDIR/app-parent/app"
SDK_DIR="$(cd "$SCRIPT_DIR/.." && pwd)/sdk"
cp "$SDK_DIR/gradlew" "$SDK_DIR/gradlew.bat" "$APP/"; mkdir -p "$APP/gradle/wrapper"
cp "$SDK_DIR/gradle/wrapper/gradle-wrapper.jar" "$SDK_DIR/gradle/wrapper/gradle-wrapper.properties" "$APP/gradle/wrapper/"
chmod +x "$APP/gradlew"
cp "$SCRIPT_DIR/contract-tests/CounterAgentSnapshot.kt.fixture" "$(find "$APP" -name CounterAgent.kt | head -1)"

# Start the server, sending its (very noisy) logs to a separate file so this driver's own output
# stays readable. Remove any stale ports-file first: on a RESTART the file persists from the prior
# run, so waiting for it to merely exist would return instantly against a not-yet-ready server.
start_server() {
    rm -f "$WORKDIR/ports.json"
    "$GOLEM" server run --data-dir "$WORKDIR/server-data" --ports-file "$WORKDIR/ports.json" >"$WORKDIR/server.log" 2>&1 &
    SERVER_PID=$!
    for _ in $(seq 1 60); do [ -s "$WORKDIR/ports.json" ] && break; sleep 1; done
}
stop_server() { [ -n "$SERVER_PID" ] && { kill "$SERVER_PID" 2>/dev/null || true; wait "$SERVER_PID" 2>/dev/null || true; } || true; }
trap 'stop_server' EXIT

# Invoke getValue and echo the integer result. Retries: writing the ports-file does not mean the
# executor/registry are ready to serve invokes yet (especially right after a restart), so a fresh
# invoke can transiently error before the value is available. Poll until a bare integer comes back.
get_value() {
    local out val
    for _ in $(seq 1 40); do
        out="$(cd "$APP" && "$GOLEM_CLI" agent invoke "$AGENT" getValue 2>/dev/null)"
        val="$(printf '%s\n' "$out" | grep -oE '^-?[0-9]+$' | tail -1)"
        [ -n "$val" ] && { printf '%s' "$val"; return 0; }
        sleep 1
    done
    printf '%s' "$out"
    return 1
}

echo "== build + deploy revision 1 =="
(cd "$APP" && "$GOLEM_CLI" build) || fail "build v1"
start_server
(cd "$APP" && "$GOLEM_CLI" deploy --yes) || fail "deploy v1"

echo "== drive state to 3 =="
for i in 1 2 3; do (cd "$APP" && "$GOLEM_CLI" agent invoke "$AGENT" increment) >/dev/null; done
before="$(get_value)"
[ "$before" = "3" ] || fail "expected 3 before update, got '$before'"

echo "== create revision 2 (patch source + clean + rebuild + redeploy) =="
# Add a new no-op method so the wasm binary changes and the host sees a genuinely new revision.
AGENT_KT="$(find "$APP" -name CounterAgent.kt | head -1)"
# Add a NEW @Endpoint method so the compiled wasm genuinely differs, forcing a new component
# revision. It MUST be an @Endpoint: an unexported/unreferenced method is dead-code-eliminated by
# Kotlin/Wasm, yielding a byte-identical binary and a "no changes required" no-op deploy.
sed -i '' 's|    fun getValue(): Int = state.value|    fun getValue(): Int = state.value\n\n    @Prompt("Get the component revision")\n    @Description("Returns the component revision (2 in revision 2)")\n    @Endpoint(get = "/version")\n    fun getVersion(): Int = 2|' "$AGENT_KT"
# Clean gradle build outputs and the golem-cli build cache so the changed source forces a full rebuild.
rm -rf "$APP/build" "$APP/.gradle" "$APP/golem-temp"
(cd "$APP" && "$GOLEM_CLI" build) || fail "build v2"
(cd "$APP" && "$GOLEM_CLI" deploy --yes) || fail "deploy v2"

echo "== manual (snapshot-based) update: save on old, load on new =="
(cd "$APP" && "$GOLEM_CLI" agent update "$AGENT" manual --await --yes) || fail "manual update"

echo "== restart server to force reload from snapshot =="
stop_server; start_server

after="$(get_value)"
[ "$after" = "3" ] || fail "state not preserved across snapshot update: expected 3, got '$after'"
echo "== snapshot e2e PASS: typed state 3 carried through manual update + restart =="
