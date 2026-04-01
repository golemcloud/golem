#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

if ! command -v golem-cli >/dev/null 2>&1; then
  echo "error: golem-cli not found on PATH" >&2
  exit 1
fi

GOLEM_CLI_FLAGS="${GOLEM_CLI_FLAGS:---local}"
read -r -a flags <<<"$GOLEM_CLI_FLAGS"

rm -rf target project/target .golem

echo "Preparing..."
sbt golemPrepare
echo "Building..."
golem-cli build --yes
echo "Deploying..."
golem-cli deploy --yes
echo "Running..."
golem-cli repl scala:demo --language typescript --script-file repl-counter.ts