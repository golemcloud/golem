#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_apps=("oplog-processor" "host-api-tests" "http-tests" "initial-file-system" "agent-counters" "agent-updates-v1" "agent-updates-v2" "agent-updates-v3" "agent-updates-v4" "scalability" "agent-sdk-rust" "agent-invocation-context" "agent-mcp")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-sdk-ts" "agent-rpc")
benchmark_apps=("benchmarks")

RUST_CHUNKS=3 # Number of chunks to split rust apps into for parallel CI builds
TS_CHUNKS=1   # Number of chunks to split ts apps into for parallel CI builds

# Optional arguments:
# - clean: clean all projects without building
# - rebuild: clean all projects before building them
# - check: run only `golem-cli build --step check` for selected projects
# - rust / ts / benchmarks: build only the specified group
# - rust-N / ts-N: build only the Nth chunk of that group (for parallel CI)
# - list-groups: print available group names for CI matrix generation

# Slice an array into a chunk and assign the result to an output array variable.
# Usage: get_chunk output_var source_array_var chunk_index total_chunks
# chunk_index is 1-based.
get_chunk() {
  local out_name=$1
  local arr_name=$2
  local chunk_idx=$3
  local total_chunks=$4
  local len chunk_size start count

  eval "len=\${#${arr_name}[@]}"

  if [ "$total_chunks" -lt 1 ] || [ "$chunk_idx" -lt 1 ] || [ "$chunk_idx" -gt "$total_chunks" ]; then
    echo "Invalid chunk index: $chunk_idx (expected 1..$total_chunks)" >&2
    return 1
  fi

  chunk_size=$(( (len + total_chunks - 1) / total_chunks ))
  start=$(( (chunk_idx - 1) * chunk_size ))

  if [ "$start" -ge "$len" ] || [ "$chunk_size" -eq 0 ]; then
    eval "$out_name=()"
    return 0
  fi

  count=$chunk_size
  if [ $(( start + count )) -gt "$len" ]; then
    count=$(( len - start ))
  fi

  local i
  eval "$out_name=()"
  for ((i=start; i<start+count; i++)); do
    eval "$out_name+=(\"\${${arr_name}[$i]}\")"
  done
}

# Print available groups as a JSON array for CI matrix generation.
print_groups_json() {
  local i sep=""
  printf '['
  for ((i=1; i<=RUST_CHUNKS; i++)); do
    printf '%s{"name":"rust-%d","needs-node":false}' "$sep" "$i"
    sep=","
  done
  for ((i=1; i<=TS_CHUNKS; i++)); do
    printf '%s{"name":"ts-%d","needs-node":true}' "$sep" "$i"
    sep=","
  done
  printf '%s{"name":"benchmarks","needs-node":true}]\n' "$sep"
}

clean_only=false
rebuild=false
check_only=false
single_group=false
group=""
for arg in "$@"; do
  case "$arg" in
    clean)
      clean_only=true
      ;;
    rebuild)
      rebuild=true
      ;;
    check)
      check_only=true
      ;;
    rust|ts|benchmarks)
      single_group=true
      group="$arg"
      ;;
    rust-*|ts-*)
      if [[ "$arg" =~ ^(rust|ts)-([0-9]+)$ ]]; then
        single_group=true
        group="$arg"
      else
        echo "Unknown argument: $arg" >&2
        exit 1
      fi
      ;;
    list-groups)
      print_groups_json
      exit 0
      ;;
    *)
      echo "Unknown argument: $arg" >&2
      exit 1
      ;;
  esac
done

if [ "$check_only" = true ] && ([ "$clean_only" = true ] || [ "$rebuild" = true ]); then
  echo "'check' mode cannot be combined with 'clean' or 'rebuild'" >&2
  exit 1
fi

GOLEM_CLI="${TEST_COMP_DIR:-$(pwd)}/../target/debug/golem-cli"

should_clean() {
  [ "$clean_only" = true ] || [ "$rebuild" = true ]
}

build_rust_apps() {
  local apps=("$@")
  if [ "$clean_only" = true ]; then
    echo "Cleaning Rust test apps"
  elif [ "$check_only" = true ]; then
    echo "Checking Rust test apps"
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

    if [ "$check_only" = true ]; then
      echo "Checking $subdir..."
      "$GOLEM_CLI" build --step check --yes
    elif [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      "$GOLEM_CLI" --preset release build --yes
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
  elif [ "$check_only" = true ]; then
    echo "Checking $label test apps"
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

    if [ "$check_only" = true ]; then
      echo "Checking $subdir..."
      "$GOLEM_CLI" build --step check --yes
    elif [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      npm install
      "$GOLEM_CLI" build --yes
      "$GOLEM_CLI" exec copy
    fi

    popd || exit
  done
}

# Handle chunk groups (rust-N, ts-N) or full groups
if [[ "$group" =~ ^rust-([0-9]+)$ ]]; then
  chunk_idx="${BASH_REMATCH[1]}"
  if [ "$chunk_idx" -lt 1 ] || [ "$chunk_idx" -gt "$RUST_CHUNKS" ]; then
    echo "Invalid rust chunk: $chunk_idx (expected 1..$RUST_CHUNKS)" >&2
    exit 1
  fi
  get_chunk chunk_apps rust_test_apps "$chunk_idx" "$RUST_CHUNKS"
  echo "Rust chunk $chunk_idx/$RUST_CHUNKS: ${chunk_apps[*]}"
  build_rust_apps "${chunk_apps[@]}"
elif [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  build_rust_apps "${rust_test_apps[@]}"
fi

if [[ "$group" =~ ^ts-([0-9]+)$ ]]; then
  chunk_idx="${BASH_REMATCH[1]}"
  if [ "$chunk_idx" -lt 1 ] || [ "$chunk_idx" -gt "$TS_CHUNKS" ]; then
    echo "Invalid ts chunk: $chunk_idx (expected 1..$TS_CHUNKS)" >&2
    exit 1
  fi
  get_chunk chunk_apps ts_test_apps "$chunk_idx" "$TS_CHUNKS"
  echo "TS chunk $chunk_idx/$TS_CHUNKS: ${chunk_apps[*]}"
  NODE_GROUP_LABEL="TS" build_node_apps "${chunk_apps[@]}"
elif [ "$single_group" = "false" ] || [ "$group" = "ts" ]; then
  NODE_GROUP_LABEL="TS" build_node_apps "${ts_test_apps[@]}"
fi

if [ "$single_group" = "false" ] || [ "$group" = "benchmarks" ]; then
  NODE_GROUP_LABEL="benchmark" build_node_apps "${benchmark_apps[@]}"
fi
