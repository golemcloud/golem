#!/usr/bin/env sh
set -uox pipefail

cargo component build --release
mkdir -pv target/wasm32-wasi/release/auction:auction-stub
cp target/wasm32-wasi/release/auction_stub.wasm target/wasm32-wasi/release/auction:auction-stub/stub-auction.wasm
wasm-tools compose -v target/wasm32-wasi/release/auction_registry.wasm -o target/wasm32-wasi/release/auction_registry_composed.wasm

cp target/wasm32-wasi/release/auction_registry_composed.wasm ..
cp target/wasm32-wasi/release/auction.wasm ..