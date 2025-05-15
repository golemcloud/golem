#!/bin/bash
set -e

# Load environment variables from .env file if it exists
if [ -f ".env" ]; then
  echo "Loading environment variables from .env file..."
  export $(grep -v '^#' .env | xargs)
fi

# Check if OPENAI_API_KEY is set
if [ -z "$OPENAI_API_KEY" ]; then
  echo "WARNING: OPENAI_API_KEY environment variable is not set."
  echo "Please set it in the .env file or with: export OPENAI_API_KEY=your-api-key"
fi

# Build the WASM component
echo "Building embed-openai WASM component..."
cargo build --target wasm32-wasi --release

# Create the target directory if it doesn't exist
TARGET_DIR="$HOME/.golem/components"
mkdir -p "$TARGET_DIR"

# Copy the WASM file to the components directory
echo "Installing component to $TARGET_DIR..."
cp "$(pwd)/target/wasm32-wasi/release/embed_openai.wasm" "$TARGET_DIR/embed_openai.wasm"

echo "Deployment complete! The embed-openai component is now available at $TARGET_DIR/embed_openai.wasm"
echo "You can now use the embed-openai component in your Golem applications."
echo "Remember to set your OPENAI_API_KEY environment variable if you haven't already."