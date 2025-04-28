#!/bin/bash
set -e

# Load environment variables from .env file if it exists
if [ -f ".env" ]; then
  echo "Loading environment variables from .env file..."
  export $(grep -v '^#' .env | xargs)
fi

# Check if OPENAI_API_KEY is set
if [ -z "$OPENAI_API_KEY" ]; then
  echo "ERROR: OPENAI_API_KEY environment variable is not set."
  echo "Please set it in the .env file or with: export OPENAI_API_KEY=your-api-key"
  exit 1
fi

COMPONENT_PATH="$HOME/.golem/components/embed_openai.wasm"

# Check if the component exists
if [ ! -f "$COMPONENT_PATH" ]; then
  echo "ERROR: Component not found at $COMPONENT_PATH"
  echo "Please run ./deploy.sh first to build and install the component."
  exit 1
fi

echo "OpenAI embedding component is installed at: $COMPONENT_PATH"
echo "Environment is properly configured with OPENAI_API_KEY."
echo "The component is ready to use in your Golem applications."
echo "\nTo use this component in your Golem application, reference it in your WIT configuration."