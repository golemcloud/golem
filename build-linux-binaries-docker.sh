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

# Use Ubuntu 22.04 (same as GitHub Actions ubuntu-latest)
DOCKER_IMAGE="ubuntu:22.04"

echo "1️⃣  Pulling Docker image: $DOCKER_IMAGE"
docker pull "$DOCKER_IMAGE"
echo ""

echo "2️⃣  Building services in Docker container..."
echo "   This will take 10-15 minutes on first run (uses Docker layer caching)"
echo ""

# Get git information for build scripts (shadow-rs)
GIT_COMMIT=$(git rev-parse HEAD)
GIT_BRANCH=$(git rev-parse --abbrev-ref HEAD)
GIT_TAG=$(git describe --tags --match "v*" --always 2>/dev/null || echo "v0.0.0")

echo "   Git context: $GIT_TAG ($GIT_BRANCH)"
echo ""

# Create persistent cargo cache directory on host
CARGO_CACHE_DIR="${HOME}/.cache/golem-docker-cargo"
mkdir -p "$CARGO_CACHE_DIR"/{registry,git}

echo "   Using cargo cache: $CARGO_CACHE_DIR"
echo ""

# Mount the repository and build
# Note: Mounting .git separately to ensure build scripts can access git metadata
# Note: Mounting cargo cache for faster subsequent builds
docker run --rm \
    -v "$(pwd)":/workspace \
    -v "$(pwd)/.git":/workspace/.git:ro \
    -v "$CARGO_CACHE_DIR/registry":/root/.cargo/registry \
    -v "$CARGO_CACHE_DIR/git":/root/.cargo/git \
    -w /workspace \
    -e "GIT_COMMIT=$GIT_COMMIT" \
    -e "GIT_BRANCH=$GIT_BRANCH" \
    -e "GIT_TAG=$GIT_TAG" \
    "$DOCKER_IMAGE" \
    bash -c '
        set -e
        echo "Installing system dependencies..."
        apt-get update -qq
        apt-get install -y -qq curl build-essential protobuf-compiler pkg-config libssl-dev git > /dev/null

        echo "Installing Rust..."
        curl --proto "=https" --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain stable
        source "$HOME/.cargo/env"

        echo "Verifying git access..."
        git --version
        git log -1 --oneline || echo "Warning: git log failed"

        echo "Building golem-cli..."
        cargo build --package golem-cli --release 2>&1 | grep -E "(Compiling golem-|Finished|error)" || true

        echo "Building golem-shard-manager..."
        cargo build --package golem-shard-manager --release 2>&1 | grep -E "(Compiling golem-|Finished|error)" || true

        echo "Building golem-component-service..."
        cargo build --package golem-component-service --release 2>&1 | grep -E "(Compiling golem-|Finished|error)" || true

        echo "Building golem-worker-service..."
        cargo build --package golem-worker-service --release 2>&1 | grep -E "(Compiling golem-|Finished|error)" || true

        echo "✅ Build complete!"
        ls -lh target/release/golem-cli target/release/golem-shard-manager target/release/golem-component-service target/release/golem-worker-service 2>/dev/null || echo "Some binaries may not have been built"
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
