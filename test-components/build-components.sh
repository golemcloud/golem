#!/bin/bash
set -euo pipefail
IFS=$'\n\t'

rust_test_components=("write-stdout" "write-stderr" "read-stdin" "clocks" "shopping-cart" "file-write-read-delete" "file-service" "http-client" "directories" "environment-service" "promise" "interruption" "clock-service"
"option-service" "flags-service" "http-client-2" "failing-component" "variant-service" "key-value-service" "blob-store-service" "runtime-service" "networking" "shopping-cart-resource"
"update-test-v1" "update-test-v2-11" "update-test-v3-11" "update-test-v4" "rust-echo" "logging" "oplog-processor" "rdbms-service" "component-resolve" "http-client-3" "golem-rust-tests")

rust_test_apps=("auction-example" "rpc" "rust-service/rpc" "custom-durability" "invocation-context" "scheduled-invocation" "high-volume-logging")
c_test_components=("large-initial-memory" "large-dynamic-memory")
ts_test_apps=("agent-constructor-parameter-echo" "agent-promise" "agent-self-rpc" "agent-rpc" "benchmarks")

# Optional arguments:
# - rebuild: clean all projects before building them
# - update-wit: update the wit/deps directories
# - rust / c / ts: build only the specified language

rebuild=false
single_lang=false
update_wit=false
lang=""
for arg in "$@"; do
  case $arg in
    rebuild)
      rebuild=true
      ;;
    update-wit)
      update_wit=true
      ;;
    rust)
      single_lang=true
      lang="rust"
      ;;
    c)
      single_lang=true
      lang="c"
      ;;
    ts)
      single_lang=true
      lang="ts"
      ;;
    *)
      echo "Unknown argument: $arg"
      exit 1
      ;;
  esac
done

if [ "$single_lang" = "false" ] || [ "$lang" = "rust" ]; then
  echo "Building the Rust test components"
  for subdir in "${rust_test_components[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$update_wit" = true ] && [ -f "wit/deps.toml" ]; then
      wit-deps update
    fi

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

if [ "$single_lang" = "false" ] || [ "$lang" = "rust" ]; then
  echo "Building the Rust test apps"
  TEST_COMP_DIR="$(pwd)"
  export GOLEM_RUST_PATH="${TEST_COMP_DIR}/../sdks/rust/golem-rust"
  export GOLEM_CLI=${TEST_COMP_DIR}/../target/debug/golem-cli
  for subdir in "${rust_test_apps[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$update_wit" = true ] && [ -f "wit/deps.toml" ]; then
      wit-deps update
    fi

    if [ "$rebuild" = true ]; then
      $GOLEM_CLI app clean
      cargo clean
    fi

    $GOLEM_CLI app -b release build
    $GOLEM_CLI app -b release copy

    popd || exit
  done
fi

if [ "$single_lang" = "false" ] || [ "$lang" = "c" ]; then
  echo "Building the C test components"
  for subdir in "${c_test_components[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$update_wit" = true ] && [ -f "wit/deps.toml" ]; then
      wit-deps update
    fi

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

if [ "$single_lang" = "false" ] || [ "$lang" = "ts" ]; then
  echo "Building the TS test apps"
  for subdir in "${ts_test_apps[@]}"; do
    echo "Building $subdir..."
    pushd "$subdir" || exit

    if [ "$update_wit" = true ]; then
      golem-cli app update-wit-deps
    fi

    if [ "$rebuild" = true ]; then
      golem-cli app clean
    fi

    golem-cli app build
    golem-cli app copy

    popd || exit
  done
fi
