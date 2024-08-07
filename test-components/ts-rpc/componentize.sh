#!/bin/bash
set -euo pipefail

pushd counter
npm install
npm run gen:ts
npm run build
npm run componentize
popd

pushd counter-stub
cargo component build --release
popd

pushd caller
npm install
npm run gen:ts
npm run build
npm run componentize
popd

cp caller/dist/caller-ts.wasm caller-ts.wasm
cp counter/dist/counter-ts.wasm counter-ts.wasm

golem-cli stubgen compose \
    --source-wasm caller-ts.wasm \
    --stub-wasm counter-stub/target/wasm32-wasi/release/counters_stub.wasm \
    --dest-wasm caller-composed-ts.wasm
