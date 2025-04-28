#!/bin/bash
set -e

# Deployment script for Golem Embedding Providers
# This script helps deploy the embedding providers to a Golem environment

# Load environment variables from .env file if it exists
if [ -f ".env" ]; then
  echo "Loading environment variables from .env file..."
  export $(grep -v '^#' .env | xargs)
fi

echo "Deploying Golem Embedding Providers..."

# Check if build exists, if not build it
if [ ! -d "target/wasm" ] || [ -z "$(ls -A target/wasm)" ]; then
    echo "No build found. Building embedding providers..."
    ./build.sh
fi

# Default deployment directory
DEPLOY_DIR="${GOLEM_HOME:-$HOME/.golem}/components"

# Allow custom deployment directory
if [ "$1" != "" ]; then
    DEPLOY_DIR="$1"
fi

# Create deployment directory if it doesn't exist
mkdir -p "$DEPLOY_DIR"

# Copy WASM components to deployment directory
echo "Copying components to $DEPLOY_DIR..."
cp target/wasm/golem_embed_*.wasm "$DEPLOY_DIR"/

echo "Deployment complete!"
echo "The following components have been deployed:"
ls -la "$DEPLOY_DIR"/golem_embed_*.wasm

echo ""
echo "To use these components, set the following environment variables:"
echo "  OPENAI_API_KEY - for OpenAI embeddings"
echo "  COHERE_API_KEY - for Cohere embeddings"
echo "  HUGGINGFACE_API_KEY - for HuggingFace embeddings"
echo "  VOYAGEAI_API_KEY - for VoyageAI embeddings"