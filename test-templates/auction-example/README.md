# Steps to compile

λ golem-wasm-rpc-stubgen generate --source-wit-root auction/wit --dest-crate-root auction-stub
λ cp -rv auction-stub/wit/deps/* auction-registry/wit/deps
λ mkdir -pv auction-registry/wit/deps/auction-stub
λ cp auction-stub/wit/_stub.wit auction-registry/wit/deps/auction-stub/stub.wit

λ cargo component build
λ mkdir -pv target/wasm32-wasi/debug/auction:auction-stub
λ cp target/wasm32-wasi/debug/auction_stub.wasm target/wasm32-wasi/debug/auction:auction-stub/stub-auction.wasm
λ wasm-tools compose -v target/wasm32-wasi/debug/auction_registry.wasm -o target/wasm32-wasi/debug/auction_registry_composed.wasm 

λ cp target/wasm32-wasi/debug/auction_registry_composed.wasm ..
