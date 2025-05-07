#!/bin/bash
set -eo pipefail

# Check for rustup installation
if ! command -v rustup &> /dev/null; then
    echo "Error: rustup is required but not installed. Please install rustup first."
    exit 1
fi

echo "Starting Golem Embedding Providers build process..."
rustup target add wasm32-wasi

# Build with release profile
cargo build --release --target wasm32-wasi

# Generate WASM components
mkdir -p ./components
for file in target/wasm32-wasi/release/*.wasm; do
    [ -f "$file" ] || continue
    name=$(basename "$file" .wasm)
    wasm-tools component new "$file" -o "./components/${name}.component.wasm"
done

echo "Build completed successfully. Components in ./components directory"