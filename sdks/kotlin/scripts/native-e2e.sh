#!/usr/bin/env bash
# End-to-end: scaffold -> build -> deploy -> invoke, entirely through the real toolchain
# (golem new --template kotlin, golem build, golem deploy, golem agent invoke) -- no
# hand-built components, no manual golem.yaml editing. Requires the SDK/KSP/gradle-plugin
# already published to mavenLocal (see sdks/kotlin/{sdk,ksp,gradle-plugin}/README or just
# `./gradlew publishToMavenLocal` in each), and `golem`/`golem-cli` built from this branch
# (`cargo build -p golem -p golem-cli`) so the 3 wasmtime engine flags (wasm_gc,
# wasm_function_references, wasm_exceptions) are present.
#
# Usage: native-e2e.sh <workdir>
#
#   workdir   a scratch directory to scaffold the app + local server data into (created fresh)
#
# Requires on PATH: gradle wrapper prerequisites (java 17+, cargo for wit-bindgen adapter
# discovery), curl. GOLEM_BIN/GOLEM_CLI_BIN env vars override the built binary paths.
set -euo pipefail

WORKDIR="${1:?workdir}"
GOLEM="${GOLEM_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/target/debug/golem}"
GOLEM_CLI="${GOLEM_CLI_BIN:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)/target/debug/golem-cli}"

rm -rf "$WORKDIR"
mkdir -p "$WORKDIR/app-parent" "$WORKDIR/server-data"

echo "== Step 1: scaffold (golem new --template kotlin) =="
(cd "$WORKDIR/app-parent" && "$GOLEM_CLI" new --template kotlin --component-name example:counter --yes app)

APP="$WORKDIR/app-parent/app"
# Gradle wrapper isn't part of the template yet (golem new doesn't run `gradle wrapper` for
# Kotlin) -- reuse the one checked in under sdks/kotlin/sdk/.
SDK_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)/sdk"
cp "$SDK_DIR/gradlew" "$SDK_DIR/gradlew.bat" "$APP/"
mkdir -p "$APP/gradle/wrapper"
cp "$SDK_DIR/gradle/wrapper/gradle-wrapper.jar" "$SDK_DIR/gradle/wrapper/gradle-wrapper.properties" "$APP/gradle/wrapper/"
chmod +x "$APP/gradlew"

echo "== Step 2: build (golem build) =="
(cd "$APP" && "$GOLEM_CLI" build)

echo "== Step 2b: start local server (golem server run) =="
"$GOLEM" server run --data-dir "$WORKDIR/server-data" --ports-file "$WORKDIR/ports.json" &
SERVER_PID=$!
trap 'kill "$SERVER_PID" 2>/dev/null || true' EXIT
for _ in $(seq 1 30); do
  [ -s "$WORKDIR/ports.json" ] && break
  sleep 1
done
CUSTOM_PORT="$(grep -o '"customRequestPort": *[0-9]*' "$WORKDIR/ports.json" | grep -o '[0-9]*')"

echo "== Step 2c: deploy (golem deploy --yes) =="
(cd "$APP" && "$GOLEM_CLI" deploy --yes)

echo "== Step 3: invoke via REPL/CLI =="
for i in 1 2 3; do
  echo -n "increment #$i -> "
  (cd "$APP" && "$GOLEM_CLI" agent invoke 'CounterAgent("c1")' increment) | tail -1
done
echo -n "getValue -> "
(cd "$APP" && "$GOLEM_CLI" agent invoke 'CounterAgent("c1")' getValue) | tail -1

echo "== Step 4: invoke via HTTP =="
echo -n "POST /counters/c1/increment -> "
curl -s -o /dev/null -w '%{http_code}\n' -X POST "http://localhost:$CUSTOM_PORT/counters/c1/increment" -H 'Host: app.localhost:9006'
echo -n "GET /counters/c1/value -> "
curl -s "http://localhost:$CUSTOM_PORT/counters/c1/value" -H 'Host: app.localhost:9006'
echo ""
echo -n "POST /counters/c2/increment (fresh instance) -> "
curl -s "http://localhost:$CUSTOM_PORT/counters/c2/increment" -X POST -H 'Host: app.localhost:9006'
echo ""

echo "== e2e complete =="
