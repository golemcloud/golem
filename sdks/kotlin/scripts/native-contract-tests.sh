#!/usr/bin/env bash
# Contract-test harness: scaffold -> build -> deploy -> probe, entirely through the real
# toolchain, swapping the scaffolded CounterAgent for ContractProbeAgent. Proves the
# compiled-Kotlin <-> host ABI boundary for each capability (contract-only: no trap + expected
# shape; NOT functional correctness). Mirrors native-e2e.sh's scaffold/build/server/deploy.
#
# Usage: native-contract-tests.sh <workdir>
# Requires: SDK/KSP/gradle-plugin published to mavenLocal; golem/golem-cli built from this branch.
# GOLEM_BIN/GOLEM_CLI_BIN override the built binary paths.
set -uo pipefail   # NOTE: not -e; a failing probe must not abort the run.

WORKDIR="${1:?workdir}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
GOLEM="${GOLEM_BIN:-$(cd "$SCRIPT_DIR/../../.." && pwd)/target/debug/golem}"
GOLEM_CLI="${GOLEM_CLI_BIN:-$(cd "$SCRIPT_DIR/../../.." && pwd)/target/debug/golem-cli}"
FIXTURE="$SCRIPT_DIR/contract-tests/ContractProbeAgent.kt"
# Each probe runs on its OWN durable agent (keyed by the method name) so a wasm trap in one probe
# wedges only that worker -- it can't cascade false FAILs onto later probes via shared-agent
# recovery. (Learned the hard way: a shared "p1" made the oplog trap fail every subsequent probe.)
agent_for() { printf 'ContractProbeAgent("%s")' "$1"; }

PASS=0; FAIL=0; RESULTS=()

record() { # record <label> <verdict> <detail>
  RESULTS+=("$1|$2|$3")
  if [ "$2" = PASS ]; then PASS=$((PASS+1)); else FAIL=$((FAIL+1)); fi
}

probe() { # probe <label> <method> [args...]
  local label="$1" method="$2"; shift 2
  local out ec
  out="$(cd "$APP" && "$GOLEM_CLI" agent invoke "$(agent_for "$method")" "$method" "$@" 2>&1)"; ec=$?
  if [ $ec -ne 0 ]; then record "$label" FAIL "trap/invoke-error (exit $ec): $(echo "$out" | tail -1)"
  elif echo "$out" | grep -q 'OK'; then record "$label" PASS "$(echo "$out" | grep 'OK' | tail -1)"
  else record "$label" FAIL "no OK in result: $(echo "$out" | tail -1)"; fi
}

probe_json() { # probe_json <label> <method> <grep-pattern>
  local label="$1" method="$2" pat="$3"
  local out ec
  out="$(cd "$APP" && "$GOLEM_CLI" agent invoke "$(agent_for "$method")" "$method" --format json 2>&1)"; ec=$?
  if [ $ec -ne 0 ]; then record "$label" FAIL "trap/invoke-error (exit $ec)"
  elif echo "$out" | grep -Eq "$pat"; then record "$label" PASS "matched /$pat/"
  else record "$label" FAIL "pattern /$pat/ not in result"; fi
}

probe_http() { # probe_http <label> <verb> <path> [grep-pattern]
  local label="$1" verb="$2" path="$3" pat="${4:-OK}"
  local out
  out="$(curl -s -X "$verb" "http://localhost:$CUSTOM_PORT$path" -H 'Host: app.localhost:9006')"
  if echo "$out" | grep -q "$pat"; then record "$label" PASS "http $verb $path"
  else record "$label" FAIL "http $verb $path -> $out"; fi
}

rm -rf "$WORKDIR"; mkdir -p "$WORKDIR/app-parent" "$WORKDIR/server-data"

echo "== scaffold =="
(cd "$WORKDIR/app-parent" && "$GOLEM_CLI" new --template kotlin --component-name example:probe --yes app)
APP="$WORKDIR/app-parent/app"

# Reuse the checked-in gradle wrapper (golem new does not scaffold one for Kotlin).
SDK_DIR="$(cd "$SCRIPT_DIR/.." && pwd)/sdk"
cp "$SDK_DIR/gradlew" "$SDK_DIR/gradlew.bat" "$APP/"
mkdir -p "$APP/gradle/wrapper"
cp "$SDK_DIR/gradle/wrapper/gradle-wrapper.jar" "$SDK_DIR/gradle/wrapper/gradle-wrapper.properties" "$APP/gradle/wrapper/"
chmod +x "$APP/gradlew"

echo "== swap in probe fixture =="
# Delete the scaffolded CounterAgent and drop the probe into a fresh package dir.
find "$APP" -name 'CounterAgent.kt' -delete
PROBE_DIR="$APP/src/wasmWasiMain/kotlin/contractprobe"
mkdir -p "$PROBE_DIR"
cp "$FIXTURE" "$PROBE_DIR/ContractProbeAgent.kt"
# Point the app's httpApi at the probe agent instead of CounterAgent.
HTTP_YAML="$(grep -rl 'CounterAgent' "$APP" --include=golem.yaml | head -1)"
[ -n "$HTTP_YAML" ] && perl -0pi -e 's/CounterAgent/ContractProbeAgent/g' "$HTTP_YAML"

echo "== build =="
(cd "$APP" && "$GOLEM_CLI" build) || { echo "BUILD FAILED"; exit 2; }

echo "== server =="
"$GOLEM" server run --data-dir "$WORKDIR/server-data" --ports-file "$WORKDIR/ports.json" &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT
for _ in $(seq 1 30); do [ -s "$WORKDIR/ports.json" ] && break; sleep 1; done
CUSTOM_PORT="$(grep -o '"customRequestPort": *[0-9]*' "$WORKDIR/ports.json" | grep -o '[0-9]*')"

echo "== deploy =="
(cd "$APP" && "$GOLEM_CLI" deploy --yes) || { echo "DEPLOY FAILED"; exit 2; }

echo "== probes =="
probe      "1 agent-model"  probeAgentModel
probe_http "1 http-gateway" POST "/probe/p1/http-echo"

# NOTE: Kotlin agents use golem-cli's fallback TypeScript literal syntax (SourceLanguage::Other),
# so args are TS literals: records as { field: value }, lists as [a,b,c], option-some as the bare
# value. The WAVE/structural forms ((3,4), s(5)) are rejected client-side for Kotlin.
probe_json "2 type-lower"     returnAllTypes 'hello'
probe_json "2 type-lower-num" returnAllTypes '(-64|64)'
probe      "2 type-lift-rec"  echoRecord '{ x: 3, y: 4 }'
probe      "2 type-lift-list" echoList   '[1,2,3]'
probe      "2 type-lift-opt"  echoOpt    '5'

probe "3 host-api" probeHostApi
probe "4 oplog"    probeOplog
probe "5 retry-dsl" probeRetry
probe "6 transactions" probeTransactions
probe "7 guards"        probeGuards

# Secrets: PASS on clean SecretError (OK) OR host trap (invalid handle rejected); only a silent
# value return is a real FAIL. Custom classification (not the standard probe helper).
{
  out="$(cd "$APP" && "$GOLEM_CLI" agent invoke "$(agent_for probeSecrets)" probeSecrets 2>&1)"; ec=$?
  if [ $ec -ne 0 ]; then record "8 secrets" PASS "host rejected invalid handle (trap): $(echo "$out" | tail -1)"
  elif echo "$out" | grep -q 'OK'; then record "8 secrets" PASS "$(echo "$out" | grep 'OK' | tail -1)"
  else record "8 secrets" FAIL "silent value return: $(echo "$out" | tail -1)"; fi
}
probe "9 context" probeContext
probe "10 durability" probeDurability

# (later tasks append probe rows here)

echo ""
echo "==================== CONTRACT TEST RESULTS ===================="
for r in "${RESULTS[@]}"; do
  IFS='|' read -r label verdict detail <<< "$r"
  printf '  %-4s %-22s %s\n' "$verdict" "$label" "$detail"
done
echo "--------------------------------------------------------------"
echo "  PASS=$PASS FAIL=$FAIL"
[ "$FAIL" -eq 0 ]
