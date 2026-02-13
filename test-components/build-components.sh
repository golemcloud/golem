#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_components=("runtime-service" "oplog-processor" "update-test-env-var")
rust_test_apps=("host-api-tests" "http-tests" "scheduled-invocation" "ifs-update" "ifs-update-inside-exported-function" "agent-counters" "rpc" "agent-updates-v1" "agent-updates-v2" "agent-updates-v3" "agent-updates-v4")
c_test_components=("large-initial-memory" "large-dynamic-memory")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-self-rpc" "agent-rpc")
benchmark_apps=("benchmarks")

# Optional arguments:
# - rebuild: clean all projects before building them
# - rust / c / ts / benchmarks: build only the specified group

rebuild=false
single_group=false
group=""
for arg in "$@"; do
  case $arg in
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

if [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  echo "Building the Rust test components"
  for subdir in "${rust_test_components[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$rebuild" = true ]; then
      cargo clean
    fi
    cargo component build --release

    echo "Turning the module into a WebAssembly Component..."
    target="../$subdir.wasm"
    target_wat="../$subdir.wat"
    cp -v $(find target/wasm32-wasip1/release -name '*.wasm' -maxdepth 1) "$target"
    wasm-tools print "$target" >"$target_wat"

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "rust" ]; then
  echo "Building the Rust test apps"
  TEST_COMP_DIR="$(pwd)"
  export GOLEM_RUST_PATH="${TEST_COMP_DIR}/../sdks/rust/golem-rust"
  for subdir in "${rust_test_apps[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$rebuild" = true ]; then
      golem-cli clean
      cargo clean
    fi

    golem-cli --preset release  build
    golem-cli --preset release exec copy

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "c" ]; then
  echo "Building the C test components"
  for subdir in "${c_test_components[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$rebuild" = true ]; then
      rm *.wasm
    fi
    wit-bindgen c --autodrop-borrows yes ./wit
    # last built with wasi-sdk-0.25.0
    $WASI_SDK_PATH/bin/clang --sysroot $WASI_SDK_PATH/share/wasi-sysroot main.c c_api1.c c_api1_component_type.o -o main.wasm

    echo "Turning the module into a WebAssembly Component..."
    target="../$subdir.wasm"
    target_wat="../$subdir.wat"
    wasm-tools component new main.wasm -o "$target" --adapt ../../../golem-wit/adapters/tier1/wasi_snapshot_preview1.wasm
    wasm-tools print "$target" >"$target_wat"

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "ts" ]; then
  echo "Building the TS test apps"
  for subdir in "${ts_test_apps[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$rebuild" = true ]; then
      rm -rf node_modules
      npm install
      golem-cli clean
    fi

    golem-cli build
    golem-cli exec copy

    popd || exit
  done
fi

if [ "$single_group" = "false" ] || [ "$group" = "benchmarks" ]; then
  echo "Building benchmark apps"
  for subdir in "${benchmark_apps[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$rebuild" = true ]; then
      rm -rf node_modules
      npm install
      golem-cli clean
      cargo clean
    fi

    golem-cli build
    golem-cli exec copy

    popd || exit
  done
fi
