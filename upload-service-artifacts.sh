#!/bin/bash
# Upload pre-built Golem service binaries as GitHub Release
# This allows CI to download and run real services without building them

set -e

REPO="michaeloboyle/golem"
TAG="mcp-services-v1"
RELEASE_NAME="Pre-built Services for MCP Integration Testing"

echo "📦 Uploading Golem Service Binaries to GitHub Release"
echo "============================================================"
echo

# Check if release exists
if gh release view "$TAG" --repo "$REPO" &>/dev/null; then
    echo "Release $TAG already exists. Deleting..."
    gh release delete "$TAG" --repo "$REPO" --yes
    echo

fi

# Create release
echo "Creating release $TAG..."
gh release create "$TAG" \
    --repo "$REPO" \
    --title "$RELEASE_NAME" \
    --notes "Pre-built debug binaries for MCP integration testing in CI.

**Services included:**
- golem-cli (257MB)
- golem-shard-manager (58MB)
- golem-component-service (115MB)
- golem-worker-service (153MB)

**Usage in CI:**
These binaries are downloaded in the MCP integration workflow to run real Golem services alongside the MCP server, proving end-to-end integration without building from source in CI (saves ~10GB disk space).

**Built from:** $(git rev-parse HEAD)
**Build date:** $(date -u +"%Y-%m-%d %H:%M:%S UTC")
"

echo "✅ Release created"
echo

# Upload binaries
echo "Uploading service binaries..."
cd target/debug/

for binary in golem-cli golem-shard-manager golem-component-service golem-worker-service; do
    if [ -f "$binary" ]; then
        echo "  Uploading $binary..."
        gh release upload "$TAG" "$binary" --repo "$REPO" --clobber
        echo "  ✅ $binary uploaded"
    else
        echo "  ❌ $binary not found, skipping"
    fi
done

cd ../..

echo
echo "============================================================"
echo "✅ All service binaries uploaded successfully!"
echo
echo "Release URL:"
gh release view "$TAG" --repo "$REPO" --web
echo
echo "To use in CI, download with:"
echo "  gh release download $TAG --repo $REPO --pattern 'golem-*'"
echo
