#!/bin/bash
set -e

# Test script for Golem Embedding Providers
# This script runs tests for all embedding providers

# Load environment variables from .env file if it exists
if [ -f ".env" ]; then
  echo "Loading environment variables from .env file..."
  export $(grep -v '^#' .env | xargs)
fi

echo "Running tests for Golem Embedding Providers..."

# Run tests for all providers
echo "Running tests for all providers..."
cargo test --workspace

# Run individual provider tests
echo "\nRunning tests for OpenAI provider..."
cargo test -p embed-openai

echo "\nRunning tests for Cohere provider..."
cargo test -p embed-cohere

echo "\nRunning tests for HuggingFace provider..."
cargo test -p embed-huggingface

echo "\nRunning tests for VoyageAI provider..."
cargo test -p embed-voyageai

echo "\nAll tests completed!"