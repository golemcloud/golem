#!/bin/sh

set -ex

moon build --target wasm
wasm-tools component embed ../golem_sdk/wit target/wasm/release/build/counter/counter.wasm  --encoding utf16 --output target/wasm/release/counter.embed.wasm 
wasm-tools component new target/wasm/release/counter.embed.wasm --output target/wasm/release/counter.agent.wasm
