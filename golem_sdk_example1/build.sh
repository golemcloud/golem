#!/bin/sh

set -ex

(cd ../golem_sdk_tools && moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/counter)
moon build --target wasm --release
wasm-tools component embed ../golem_sdk/wit _build/wasm/release/build/counter/counter.wasm  --encoding utf16 --output _build/wasm/release/counter.embed.wasm 
wasm-tools component new _build/wasm/release/counter.embed.wasm --output _build/wasm/release/counter.agent.wasm
