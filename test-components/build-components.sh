#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_apps=("oplog-processor" "host-api-tests" "http-tests" "initial-file-system" "agent-counters" "agent-updates-v1" "agent-updates-v2" "agent-updates-v3" "agent-updates-v4" "scalability" "agent-sdk-rust" "agent-invocation-context" "agent-rpc" "agent-mcp")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-sdk-ts")
benchmark_apps=("benchmarks")

RUST_CHUNKS=3 # Number of chunks to split rust apps into for parallel CI builds
TS_CHUNKS=1   # Number of chunks to split ts apps into for parallel CI builds

# Optional arguments:
# - clean: clean all projects without building
# - rebuild: clean all projects before building them
# - rust / ts / benchmarks: build only the specified group
# - rust-N / ts-N: build only the Nth chunk of that group (for parallel CI)
# - list-groups: print available group names for CI matrix generation

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
    rust-[0-9]*)
      single_group=true
      group="$arg"
      ;;
    c)
      single_group=true
      group="c"
      ;;
    ts)
      single_group=true
      group="ts"
      ;;
    ts-[0-9]*)
      single_group=true
      group="$arg"
      ;;
    benchmarks)
      single_group=true
      group="benchmarks"
      ;;
    list-groups)
      # Output JSON array of groups for CI matrix generation
      groups="["
      for i in $(seq 1 $RUST_CHUNKS); do
        groups="$groups{\"name\":\"rust-$i\",\"needs-node\":false},"
      done
      for i in $(seq 1 $TS_CHUNKS); do
        groups="$groups{\"name\":\"ts-$i\",\"needs-node\":true},"
      done
      groups="$groups{\"name\":\"benchmarks\",\"needs-node\":true}]"
      echo "$groups"
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg"
      exit 1
      ;;
  esac
done

# Get a chunk of an array by eval. Usage: get_chunk array_name chunk_index total_chunks
# chunk_index is 1-based. Prints the chunk elements space-separated.
get_chunk() {
  local arr_name=$1
  local chunk_idx=$2
  local total_chunks=$3
  eval "local len=\${#${arr_name}[@]}"
  local chunk_size=$(( (len + total_chunks - 1) / total_chunks ))
  local start=$(( (chunk_idx - 1) * chunk_size ))
  local count=$(( chunk_size ))
  if [ $(( start + count )) -gt $len ]; then
    count=$(( len - start ))
  fi
  eval "echo \"\${${arr_name}[@]:$start:$count}\""
}

GOLEM_CLI="${TEST_COMP_DIR:-$(pwd)}/../target/debug/golem-cli"

should_clean() {
  [ "$clean_only" = true ] || [ "$rebuild" = true ]
}

build_rust_apps() {
  local apps=("$@")
  if [ "$clean_only" = true ]; then
    echo "Cleaning Rust test apps"
  else
    echo "Building Rust test apps"
  fi
  TEST_COMP_DIR="$(pwd)"
  export GOLEM_RUST_PATH="${TEST_COMP_DIR}/../sdks/rust/golem-rust"
  for subdir in "${apps[@]}"; do
    pushd "$subdir" || exit

    if should_clean; then
      echo "Cleaning $subdir..."
      "$GOLEM_CLI" clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      "$GOLEM_CLI" --preset release  build
      "$GOLEM_CLI" --preset release exec copy
    fi

    popd || exit
  done
}

build_node_apps() {
  local apps=("$@")
  local label="${NODE_GROUP_LABEL:-Node}"
  if [ "$clean_only" = true ]; then
    echo "Cleaning $label test apps"
  else
    echo "Building $label test apps"
  fi
  for subdir in "${apps[@]}"; do
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
}

# Handle chunk groups (rust-N, ts-N)
if [[ "$group" =~ ^rust-([0-9]+)$ ]]; then
  chunk_idx="${BASH_REMATCH[1]}"
  chunk_apps=($(get_chunk rust_test_apps "$chunk_idx" "$RUST_CHUNKS"))
  echo "Rust chunk $chunk_idx/$RUST_CHUNKS: ${chunk_apps[*]}"
  build_rust_apps "${chunk_apps[@]}"
elif [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  build_rust_apps "${rust_test_apps[@]}"
fi

if [[ "$group" =~ ^ts-([0-9]+)$ ]]; then
  chunk_idx="${BASH_REMATCH[1]}"
  chunk_apps=($(get_chunk ts_test_apps "$chunk_idx" "$TS_CHUNKS"))
  echo "TS chunk $chunk_idx/$TS_CHUNKS: ${chunk_apps[*]}"
  NODE_GROUP_LABEL="TS" build_node_apps "${chunk_apps[@]}"
elif [ "$single_group" = "false" ] || [ "$group" = "ts" ]; then
  NODE_GROUP_LABEL="TS" build_node_apps "${ts_test_apps[@]}"
fi

if [ "$single_group" = "false" ] || [ "$group" = "benchmarks" ]; then
  NODE_GROUP_LABEL="benchmark" build_node_apps "${benchmark_apps[@]}"
fi
