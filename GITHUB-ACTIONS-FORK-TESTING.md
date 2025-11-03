# GitHub Actions Fork Testing Guide
## Bounty #1926 - MCP Server Implementation

This guide explains how to test the MCP server implementation using GitHub Actions in your fork before submitting a PR to the main repository.

## Why Test in Fork First?

- Validates all tests pass in CI environment (not just locally)
- Catches platform-specific issues (Linux vs macOS)
- Ensures workflow files are correct
- Provides proof of passing tests for PR review
- Avoids wasting maintainer time with broken PRs

## Setup Steps

### 1. Push Your Branch to Fork

```bash
cd "/Volumes/black box/github/golem"

# Add your fork as remote (if not already added)
git remote add fork git@github.com:YOUR_USERNAME/golem.git

# Push your bounty branch to fork
git push fork bounty/mcp-server-issue-1926
```

### 2. Enable GitHub Actions in Fork

1. Go to `https://github.com/YOUR_USERNAME/golem`
2. Click **Actions** tab
3. Click **"I understand my workflows, go ahead and enable them"**
4. This enables Actions for your fork

### 3. Trigger Workflow Run

The workflow will trigger automatically on push, or you can:

1. Go to **Actions** tab
2. Select **"MCP Server Tests"** workflow
3. Click **"Run workflow"** dropdown
4. Select your branch `bounty/mcp-server-issue-1926`
5. Click **"Run workflow"**

## What Gets Tested

### Workflow: `mcp-server-tests.yml`

**Jobs:**
1. **mcp-unit-tests** - Security and tools unit tests
2. **mcp-integration-tests** - 5 integration tests
3. **mcp-tool-execution-test** - Real tool execution demo
4. **mcp-security-audit** - cargo-audit + sensitive command check
5. **mcp-clippy** - Linting
6. **mcp-demo-validation** - Demo script validation
7. **mcp-coverage** - Code coverage
8. **bounty-validation** - Complete bounty requirement check

### Workflow: `mcp-pr-comment.yml`

- Posts detailed test results as PR comment
- Includes implementation summary
- Provides local testing instructions

## Viewing Results

### In GitHub UI

1. Go to **Actions** tab
2. Click on the workflow run
3. View each job's output:
   - ✅ Green checkmark = passed
   - ❌ Red X = failed
4. Click job name to see detailed logs

### Job Artifacts

After workflow completes:
1. Scroll to bottom of workflow run page
2. Download artifacts:
   - `mcp-integration-test-results`
   - `bounty-validation-report`

## Expected Results

All jobs should pass with ✅:

```
✅ mcp-unit-tests (2m 15s)
✅ mcp-integration-tests (3m 42s)
✅ mcp-tool-execution-test (1m 30s)
✅ mcp-security-audit (1m 45s)
✅ mcp-clippy (2m 10s)
✅ mcp-demo-validation (30s)
✅ mcp-coverage (4m 20s)
✅ bounty-validation (45s)
```

## Troubleshooting

### Job Fails in CI but Passes Locally

**Common causes:**

1. **Platform differences** (Linux CI vs macOS local)
   ```bash
   # Test on Linux locally using Docker
   docker run -it --rm -v "$PWD:/golem" rust:latest bash
   cd /golem
   cargo test --package golem-cli --test integration mcp_server_integration
   ```

2. **Timing issues** (CI may be slower)
   - Check `wait_for_server()` timeout (currently 30s)
   - Increase if needed in [mcp_server_integration.rs](cli/golem-cli/tests/mcp_server_integration.rs:32)

3. **Missing dependencies**
   - Check workflow file has all required tools
   - Verify Rust toolchain version

4. **Port conflicts**
   - Tests use ports 8090-8094
   - Should not conflict in CI (clean environment)

### Viewing Detailed Logs

```bash
# Download workflow logs using GitHub CLI
gh run list --workflow=mcp-server-tests.yml
gh run view RUN_ID --log

# Or download from web UI:
# Actions → Select run → Click "..." → Download log archive
```

### Re-running Failed Jobs

1. Click on failed workflow run
2. Click **"Re-run jobs"** dropdown
3. Select **"Re-run failed jobs"** or **"Re-run all jobs"**

## Making Changes After Failure

If tests fail in CI:

```bash
# Fix the issue locally
# ... make changes ...

# Test locally first
cargo test --package golem-cli --test integration mcp_server_integration
./test-mcp-tool-execution.sh

# Commit and push to trigger new CI run
git add .
git commit -m "Fix: [describe what you fixed]"
git push fork bounty/mcp-server-issue-1926
```

The workflow will automatically trigger on the new push.

## Before Creating PR to Main Repo

**Checklist:**

- [ ] All GitHub Actions jobs passing in fork
- [ ] Downloaded and reviewed bounty-validation-report
- [ ] All 5 integration tests passing
- [ ] Tool execution test demonstrates real command execution
- [ ] No clippy warnings
- [ ] Security audit passes
- [ ] Coverage report generated

**Get final confirmation:**

```bash
# View final status
gh run list --branch bounty/mcp-server-issue-1926 --limit 1

# Should show:
# STATUS  CONCLUSION  WORKFLOW           BRANCH
# ✓       success     MCP Server Tests   bounty/mcp-server-issue-1926
```

## Creating PR to Main Repo

Once all tests pass in fork:

```bash
# Create PR using GitHub CLI
gh pr create \
  --repo golemcloud/golem \
  --base main \
  --head YOUR_USERNAME:bounty/mcp-server-issue-1926 \
  --title "feat: MCP Server support for Golem CLI (Bounty #1926)" \
  --body-file PR-DESCRIPTION.md

# Or create via web UI:
# https://github.com/golemcloud/golem/compare/main...YOUR_USERNAME:bounty/mcp-server-issue-1926
```

## PR Description Template

```markdown
# MCP Server Implementation - Bounty #1926

Closes #1926

## Summary

Implements Model Context Protocol (MCP) server support for Golem CLI, exposing all 96 CLI commands as MCP tools for AI agent integration.

## Changes

- Added MCP server implementation in `cli/golem-cli/src/mcp_server/`
- Dynamic tool generation from Clap command metadata
- Security filtering for 16 sensitive commands
- 5 integration tests + 7 security unit tests
- Real tool execution demonstration

## Test Results (Fork CI)

✅ All tests passing: https://github.com/YOUR_USERNAME/golem/actions/runs/RUN_ID

- 5/5 integration tests PASSED
- 7/7 security unit tests PASSED
- Tool execution test PASSED
- Security audit PASSED
- Clippy PASSED

## Testing

```bash
# Run integration tests
cargo test --package golem-cli --test integration mcp_server_integration

# Test real tool execution
./test-mcp-tool-execution.sh

# Start MCP server
cargo run --package golem-cli -- --serve 8088
```

## Documentation

- [MCP-SERVER-IMPLEMENTATION.md](MCP-SERVER-IMPLEMENTATION.md)
- [test-mcp-tool-execution.sh](test-mcp-tool-execution.sh)

## Checklist

- [x] Tests pass locally
- [x] Tests pass in fork CI
- [x] No clippy warnings
- [x] Security audit passes
- [x] Real tool execution demonstrated
- [x] Documentation updated
```

## Advanced: Running Workflows Locally

Test workflows before pushing using `act`:

```bash
# Install act (GitHub Actions local runner)
brew install act

# Run MCP tests locally
act -j mcp-unit-tests -P ubuntu-latest=catthehacker/ubuntu:rust-latest

# Run all jobs
act -W .github/workflows/mcp-server-tests.yml
```

## Monitoring CI in Real-Time

```bash
# Watch workflow status (requires gh CLI)
watch -n 5 'gh run list --workflow=mcp-server-tests.yml --limit 1'

# Or use GitHub CLI run watch (beta)
gh run watch
```

## Summary

Testing in your fork gives you:
- Confidence before PR submission
- Proof that tests pass in CI environment
- Faster feedback loop (no waiting for maintainer review)
- Professional presentation (shows you've done due diligence)

The maintainers will appreciate that you've already validated everything works in CI! 🎯
