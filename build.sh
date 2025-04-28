#!/bin/bash
set -e

# Root build script for Golem Embedding Providers
# This script delegates to the build script in the embed-providers directory

echo "Starting Golem Embedding Providers build process..."

# Check if embed-providers directory exists
if [ ! -d "embed-providers" ]; then
    echo "Error: embed-providers directory not found!"
    exit 1
fi

# Change to embed-providers directory and run the build script
cd embed-providers
./build.sh

echo "Build process completed successfully!"