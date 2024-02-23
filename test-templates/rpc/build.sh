#!/usr/bin/env sh
set -uox pipefail

cargo component build --release
mkdir -pv target/wasm32-wasi/release/rpc:counters-stub
cp target/wasm32-wasi/release/counters_stub.wasm target/wasm32-wasi/release/rpc:counters-stub/stub-counters.wasm
wasm-tools compose -v target/wasm32-wasi/release/caller.wasm -o target/wasm32-wasi/release/caller_composed.wasm

cp target/wasm32-wasi/release/caller_composed.wasm ..
cp target/wasm32-wasi/release/counters.wasm ..
