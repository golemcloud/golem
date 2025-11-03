#!/bin/bash
# Push MCP bounty branch to fork and monitor CI tests
set -e

echo "🚀 MCP Bounty #1926 - Fork CI Testing"
echo "========================================"
echo ""

# Configuration
BRANCH="bounty/mcp-server-issue-1926"
FORK="fork"
FORK_URL="https://github.com/michaeloboyle/golem"

# Check we're on correct branch
CURRENT_BRANCH=$(git branch --show-current)
if [ "$CURRENT_BRANCH" != "$BRANCH" ]; then
    echo "❌ Not on correct branch!"
    echo "   Current: $CURRENT_BRANCH"
    echo "   Expected: $BRANCH"
    exit 1
fi

echo "✅ On branch: $BRANCH"
echo ""

# Check for uncommitted changes
if ! git diff --quiet || ! git diff --cached --quiet; then
    echo "⚠️  You have uncommitted changes"
    git status --short
    echo ""
    read -p "Commit them first? (y/n) " -n 1 -r
    echo
    if [[ $REPLY =~ ^[Yy]$ ]]; then
        echo "Committing changes..."
        git add .
        git commit -m "MCP Server: Prepare for CI testing"
    else
        echo "Skipping commit. Pushing current state..."
    fi
fi

# Push to fork
echo "📤 Pushing to fork..."
if git push $FORK $BRANCH --force-with-lease; then
    echo "✅ Pushed to fork successfully"
else
    echo "❌ Push failed"
    exit 1
fi
echo ""

# Get the workflow run URL
echo "🔗 GitHub Actions URLs:"
echo "   Fork Actions: $FORK_URL/actions"
echo "   Workflow: $FORK_URL/actions/workflows/mcp-server-tests.yml"
echo ""

# Check if gh CLI is installed
if command -v gh &> /dev/null; then
    echo "📊 GitHub CLI detected - monitoring workflow..."
    echo ""

    # Wait a moment for workflow to start
    sleep 5

    # Show recent runs
    echo "Recent workflow runs:"
    gh run list --repo michaeloboyle/golem --workflow=mcp-server-tests.yml --limit 3
    echo ""

    # Watch latest run
    echo "Watching latest run (Ctrl+C to stop monitoring)..."
    echo ""
    gh run watch --repo michaeloboyle/golem --exit-status

else
    echo "💡 Install GitHub CLI for automatic monitoring:"
    echo "   brew install gh"
    echo "   gh auth login"
    echo ""
    echo "Or visit manually:"
    echo "   $FORK_URL/actions"
    echo ""
    echo "✅ Branch pushed - check GitHub Actions tab in your fork"
fi

echo ""
echo "========================================"
echo "✅ CI Testing in Progress"
echo "========================================"
echo ""
echo "Next steps:"
echo "1. Monitor tests at: $FORK_URL/actions"
echo "2. If all pass, create PR to origin"
echo "3. If any fail, fix locally and re-run this script"
echo ""
