#!/bin/sh

set -ex

(cd ../golem_sdk_tools && moon run cmd -- reexports ../golem_sdk ../golem_sdk_example1/golem_moonbit_examples)
(cd ../golem_sdk_tools && moon run cmd -- agents ../golem_sdk_example1/golem_moonbit_examples)
moon build --target wasm --release
wasm-tools component embed wit _build/wasm/release/build/golem_moonbit_examples/golem_moonbit_examples.wasm --encoding utf16 --output _build/wasm/release/golem_moonbit_examples.embed.wasm
wasm-tools component new _build/wasm/release/golem_moonbit_examples.embed.wasm --output _build/wasm/release/golem_moonbit_examples.agent.wasm
