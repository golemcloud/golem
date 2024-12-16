#!/usr/bin/env sh
set -uox pipefail

rm -rf target/wasm32-wasi/debug/rpc:counters-stub
rm -rf target/wasm32-wasi/debug/rpc:ephemeral-stub

cargo component build
mkdir -pv target/wasm32-wasi/debug/rpc:counters-stub
cp target/wasm32-wasi/debug/counters_stub.wasm target/wasm32-wasi/debug/rpc:counters-stub/stub-counters.wasm
wasm-tools compose -v target/wasm32-wasi/debug/caller.wasm -o target/wasm32-wasi/debug/caller_composed1.wasm

mkdir -pv target/wasm32-wasi/debug/rpc:ephemeral-stub
cp target/wasm32-wasi/debug/ephemeral_stub.wasm target/wasm32-wasi/debug/rpc:ephemeral-stub/stub-ephemeral.wasm
wasm-tools compose -v target/wasm32-wasi/debug/caller_composed1.wasm -o target/wasm32-wasi/debug/caller_composed.wasm

cp target/wasm32-wasi/debug/caller_composed.wasm ..
cp target/wasm32-wasi/debug/caller.wasm ..
cp target/wasm32-wasi/debug/counters.wasm ..
cp target/wasm32-wasi/debug/ephemeral.wasm ..
