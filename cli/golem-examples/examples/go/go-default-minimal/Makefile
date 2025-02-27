.PHONY: build bindings compile clean

build: compile
	wasm-tools component embed ./wit component_name.module.wasm --output component_name.embed.wasm
	wasm-tools component new component_name.embed.wasm -o component_name.wasm --adapt adapters/tier1/wasi_snapshot_preview1.wasm

bindings:
	wit-bindgen tiny-go --out-dir component_name ./wit

compile: bindings
	tinygo build -target=wasi -tags=purego -o component_name.module.wasm main.go

clean:
	rm -rf component_name
	rm *.wasm
