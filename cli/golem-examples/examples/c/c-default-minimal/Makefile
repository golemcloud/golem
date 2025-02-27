.PHONY: build bindings compile clean

build: compile
	wasm-tools component new component_name.module.wasm -o component_name.wasm --adapt adapters/tier1/wasi_snapshot_preview1.wasm

bindings:
	wit-bindgen c --autodrop-borrows yes  --out-dir component_name  ./wit

compile: bindings
	${WASI_SDK_PATH}/bin/clang --sysroot ${WASI_SDK_PATH}/share/wasi-sysroot main.c component_name/component_name.c component_name/component_name_component_type.o -o component_name.module.wasm

clean:
	rm -rf component_name
	rm *.wasm
