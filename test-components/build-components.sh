#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_apps=("host-api-tests" "http-tests" "initial-file-system" "agent-counters" "agent-updates-v1" "agent-updates-v2" "agent-updates-v3" "agent-updates-v4" "scalability" "agent-sdk-rust" "agent-invocation-context" "agent-rpc" "agent-mcp" "oplog-processor")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-sdk-ts")
benchmark_apps=("benchmarks")

# Optional arguments:
# - clean: clean all projects without building
# - rebuild: clean all projects before building them
# - rust / c / ts / benchmarks: build only the specified group

clean_only=false
rebuild=false
single_group=false
group=""
for arg in "$@"; do
  case $arg in
    clean)
      clean_only=true
      ;;
    rebuild)
      rebuild=true
      ;;
    rust)
      single_group=true
      group="rust"
      ;;
    c)
      single_group=true
      group="c"
      ;;
    ts)
      single_group=true
      group="ts"
      ;;
    benchmarks)
      single_group=true
      group="benchmarks"
      ;;
    *)
      echo "Unknown argument: $arg"
      exit 1
      ;;
  esac
done

GOLEM_CLI="${TEST_COMP_DIR:-$(pwd)}/../target/debug/golem-cli"

should_clean() {
  [ "$clean_only" = true ] || [ "$rebuild" = true ]
}

if [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  if [ "$clean_only" = true ]; then
    echo "Cleaning the Rust test apps"
  else
    echo "Building the Rust test apps"
  fi
  TEST_COMP_DIR="$(pwd)"
  export GOLEM_RUST_PATH="${TEST_COMP_DIR}/../sdks/rust/golem-rust"
  for subdir in "${rust_test_apps[@]}"; do
    pushd "$subdir" || exit

    if should_clean; then
      echo "Cleaning $subdir..."
      "$GOLEM_CLI" clean
      cargo clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      "$GOLEM_CLI" --preset release  build
      "$GOLEM_CLI" --preset release exec copy
    fi

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "ts" ]; then
  if [ "$clean_only" = true ]; then
    echo "Cleaning the TS test apps"
  else
    echo "Building the TS test apps"
  fi
  for subdir in "${ts_test_apps[@]}"; do
    pushd "$subdir" || exit

    if should_clean; then
      echo "Cleaning $subdir..."
      rm -rf node_modules
      "$GOLEM_CLI" clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      npm install
      "$GOLEM_CLI" build
      "$GOLEM_CLI" exec copy
    fi

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "benchmarks" ]; then
  if [ "$clean_only" = true ]; then
    echo "Cleaning benchmark apps"
  else
    echo "Building benchmark apps"
  fi
  for subdir in "${benchmark_apps[@]}"; do
    pushd "$subdir" || exit

    if should_clean; then
      echo "Cleaning $subdir..."
      rm -rf node_modules
      "$GOLEM_CLI" clean
      cargo clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      npm install
      "$GOLEM_CLI" build
      "$GOLEM_CLI" exec copy
    fi

    popd || exit
  done
fi
