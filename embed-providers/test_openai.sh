#!/bin/bash
set -e

# Test script for OpenAI Embedding Provider
echo "Testing OpenAI Embedding Provider..."

# Load environment variables from .env file if it exists
if [ -f ".env" ]; then
  echo "Loading environment variables from .env file..."
  export $(grep -v '^#' .env | xargs)
fi

# Check if OpenAI API key is set
if [ -z "$OPENAI_API_KEY" ]; then
  echo "Error: OPENAI_API_KEY environment variable not set"
  echo "Please set it in the .env file or as an environment variable"
  exit 1
fi

echo "Found OpenAI API key: ${OPENAI_API_KEY:0:5}***"

# Run the OpenAI provider tests
echo "Running tests for OpenAI provider..."
cargo test -p embed-openai -- --nocapture test_generate_embeddings

echo "\nOpenAI embedding test completed!"