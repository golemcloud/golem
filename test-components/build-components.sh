#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_components=("oplog-processor")
rust_test_apps=("host-api-tests" "http-tests" "initial-file-system" "agent-counters" "agent-updates-v1" "agent-updates-v2" "agent-updates-v3" "agent-updates-v4" "scalability" "agent-http-routes-rust" "agent-invocation-context" "agent-rpc")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-http-routes-ts")
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

should_clean() {
  [ "$clean_only" = true ] || [ "$rebuild" = true ]
}

if [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  if [ "$clean_only" = true ]; then
    echo "Cleaning the Rust test components"
  else
    echo "Building the Rust test components"
  fi
  for subdir in "${rust_test_components[@]}"; do
    pushd "$subdir" || exit

    if should_clean; then
      echo "Cleaning $subdir..."
      cargo clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      cargo-component build --release

      echo "Turning the module into a WebAssembly Component..."
      target="../$subdir.wasm"
      target_wat="../$subdir.wat"
      cp -v $(find target/wasm32-wasip1/release -name '*.wasm' -maxdepth 1) "$target"
      wasm-tools print "$target" >"$target_wat"
    fi

    popd || exit
  done
fi

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
      golem-cli clean
      cargo clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      golem-cli --preset release  build
      golem-cli --preset release exec copy
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
      golem-cli clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      npm install
      golem-cli build
      golem-cli exec copy
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
      golem-cli clean
      cargo clean
    fi

    if [ "$clean_only" = false ]; then
      echo "Building $subdir..."
      npm install
      golem-cli build
      golem-cli exec copy
    fi

    popd || exit
  done
fi
