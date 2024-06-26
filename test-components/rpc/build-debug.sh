#!/usr/bin/env sh
set -uox pipefail

cargo component build
mkdir -pv target/wasm32-wasi/debug/rpc:counters-stub
cp target/wasm32-wasi/debug/counters_stub.wasm target/wasm32-wasi/debug/rpc:counters-stub/stub-counters.wasm
wasm-tools compose -v target/wasm32-wasi/debug/caller.wasm -o target/wasm32-wasi/debug/caller_composed.wasm

cp target/wasm32-wasi/debug/caller_composed.wasm ..
cp target/wasm32-wasi/debug/counters.wasm ..
