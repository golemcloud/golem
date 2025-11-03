#!/bin/bash
# Build Linux x86_64 binaries using Docker for CI compatibility
# This allows building Linux binaries on macOS for upload to GitHub Release

set -e

echo "🐳 Building Golem Services for Linux (x86_64) using Docker"
echo "============================================================"
echo ""

# Check if Docker is running
if ! docker info > /dev/null 2>&1; then
    echo "❌ Docker is not running. Please start Docker Desktop and try again."
    exit 1
fi

# Use official Rust image with same platform as GitHub Actions (ubuntu-latest)
DOCKER_IMAGE="rust:1.83"

echo "1️⃣  Pulling Docker image: $DOCKER_IMAGE"
docker pull "$DOCKER_IMAGE"
echo ""

echo "2️⃣  Building services in Docker container..."
echo "   This will take 10-15 minutes on first run (uses Docker layer caching)"
echo ""

# Mount the repository and build
docker run --rm \
    -v "$(pwd)":/workspace \
    -w /workspace \
    "$DOCKER_IMAGE" \
    bash -c '
        set -e
        echo "Installing build dependencies..."
        apt-get update -qq
        apt-get install -y -qq protobuf-compiler > /dev/null

        echo "Building golem-cli..."
        cargo build --package golem-cli --release

        echo "Building golem-shard-manager..."
        cargo build --package golem-shard-manager --release

        echo "Building golem-component-service..."
        cargo build --package golem-component-service --release

        echo "Building golem-worker-service..."
        cargo build --package golem-worker-service --release

        echo "✅ Build complete!"
        ls -lh target/release/golem-*
    '

echo ""
echo "3️⃣  Verifying binaries are Linux ELF format..."
file target/release/golem-cli | grep "ELF 64-bit LSB" && echo "✅ golem-cli: Linux x86_64"
file target/release/golem-shard-manager | grep "ELF 64-bit LSB" && echo "✅ golem-shard-manager: Linux x86_64"
file target/release/golem-component-service | grep "ELF 64-bit LSB" && echo "✅ golem-component-service: Linux x86_64"
file target/release/golem-worker-service | grep "ELF 64-bit LSB" && echo "✅ golem-worker-service: Linux x86_64"
echo ""

echo "4️⃣  Binary sizes:"
ls -lh target/release/golem-cli \
    target/release/golem-shard-manager \
    target/release/golem-component-service \
    target/release/golem-worker-service
echo ""

echo "============================================================"
echo "✅ Linux binaries ready for upload!"
echo ""
echo "Next steps:"
echo "  1. Update GitHub Release with these binaries:"
echo "     ./upload-service-artifacts.sh"
echo ""
echo "  2. These binaries will work in GitHub Actions (ubuntu-latest)"
echo ""
