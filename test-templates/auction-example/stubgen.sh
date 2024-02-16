#!/usr/bin/env sh
set -uox pipefail

rm -rf auction-stub
golem-wasm-rpc-stubgen generate --source-wit-root auction/wit --dest-crate-root auction-stub --wasm-rpc-path-override ../../../wasm-rpc/wasm-rpc
rm -rf auction-registry/wit/deps
mkdir -pv auction-registry/wit/deps
cp -rv auction-stub/wit/deps/* auction-registry/wit/deps
mkdir -pv auction-registry/wit/deps/auction-stub
cp auction-stub/wit/_stub.wit auction-registry/wit/deps/auction-stub/stub.wit
