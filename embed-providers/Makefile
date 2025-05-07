# Makefile for Golem Embedding Providers

.PHONY: all build test clean

# Default target
all: build

# Build all providers
build:
	./build.sh

# Run all tests
test:
	./test.sh

# Clean build artifacts
clean:
	rm -rf target

# Build individual providers
build-openai:
	cargo build --release --target wasm32-wasi --package embed-openai
	wasm-tools component new \
		target/wasm32-wasi/release/embed-openai.wasm \
		-o target/wasm/golem_embed_openai.wasm \
		--adapt wasi_snapshot_preview1=./wasi_snapshot_preview1.wasm


build-cohere:
	cargo build --release --target wasm32-wasi --package embed-cohere
	wasm-tools component new \
		target/wasm32-wasi/release/embed-cohere.wasm \
		-o target/wasm/golem_embed_cohere.wasm \
		--adapt wasi_snapshot_preview1=./wasi_snapshot_preview1.wasm

build-huggingface:
	cargo build --release --target wasm32-wasi --package embed-huggingface
	wasm-tools component new \
		target/wasm32-wasi/release/embed-huggingface.wasm \
		-o target/wasm/golem_embed_huggingface.wasm \
		--adapt wasi_snapshot_preview1=./wasi_snapshot_preview1.wasm

build-voyageai:
	cargo build --release --target wasm32-wasi --package embed-voyageai
	wasm-tools component new \
		target/wasm32-wasi/release/embed-voyageai.wasm \
		-o target/wasm/golem_embed_voyageai.wasm \
		--adapt wasi_snapshot_preview1=./wasi_snapshot_preview1.wasm

# Test individual providers
test-openai:
	cargo test -p embed-openai

test-cohere:
	cargo test -p embed-cohere

test-huggingface:
	cargo test -p embed-huggingface

test-voyageai:
	cargo test -p embed-voyageai