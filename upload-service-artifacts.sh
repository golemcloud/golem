#!/bin/bash
# Upload Linux service binaries to GitHub Release
# Bounty #1926 - Replace macOS binaries with Linux binaries for CI

set -e

RELEASE_TAG="mcp-services-v1"
REPO="michaeloboyle/golem"

echo "🚀 Uploading Linux Service Binaries to Release"
echo "============================================================"
echo "📦 Release: $RELEASE_TAG"
echo "📂 Repository: $REPO"
echo ""

# Verify binaries exist and are Linux format
echo "1️⃣  Verifying Linux binaries..."
for binary in golem-cli golem-shard-manager golem-component-service golem-worker-service; do
    BINARY_PATH="target/release/$binary"
    
    if [ ! -f "$BINARY_PATH" ]; then
        echo "❌ $binary not found at $BINARY_PATH"
        exit 1
    fi
    
    # Check file format
    FILE_TYPE=$(file "$BINARY_PATH")
    if ! echo "$FILE_TYPE" | grep -q "ELF 64-bit"; then
        echo "❌ $binary is not Linux ELF format:"
        echo "   $FILE_TYPE"
        exit 1
    fi
    
    echo "✅ $binary - $(du -h "$BINARY_PATH" | cut -f1)"
done

echo ""
echo "2️⃣  Deleting old macOS binaries from release..."

# Delete existing assets (macOS binaries)
for binary in golem-cli golem-shard-manager golem-component-service golem-worker-service; do
    echo "   Deleting $binary..."
    gh release delete-asset "$RELEASE_TAG" "$binary" \
        --repo "$REPO" \
        --yes 2>/dev/null || echo "   (Asset $binary not found, skipping)"
done

echo ""
echo "3️⃣  Uploading new Linux binaries to release..."

# Upload new Linux binaries
gh release upload "$RELEASE_TAG" \
    target/release/golem-cli \
    target/release/golem-shard-manager \
    target/release/golem-component-service \
    target/release/golem-worker-service \
    --repo "$REPO" \
    --clobber

echo ""
echo "============================================================"
echo "✅ UPLOAD COMPLETE"
echo "============================================================"
echo ""
echo "Uploaded binaries:"
gh release view "$RELEASE_TAG" --repo "$REPO" --json assets --jq '.assets[] | "  - \(.name) (\(.size / 1024 / 1024 | floor)MB)"'
echo ""
echo "Next step: Rerun CI workflow"
echo "  gh run rerun <run-id> --repo $REPO"
echo ""
