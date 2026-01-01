# PowerShell test runner for MCP Server
# Runs both unit and integration tests

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "MCP Server Test Suite" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

$ErrorActionPreference = "Continue"
$testsFailed = $false

# Step 1: Run unit tests
Write-Host "[1/3] Running unit tests..." -ForegroundColor Yellow
Write-Host ""

$unitTestResult = cargo test --package golem-cli --test mcp_server 2>&1
$unitTestExitCode = $LASTEXITCODE

if ($unitTestExitCode -eq 0) {
    Write-Host "✓ Unit tests PASSED" -ForegroundColor Green
} else {
    Write-Host "✗ Unit tests FAILED" -ForegroundColor Red
    Write-Host $unitTestResult
    $testsFailed = $true
}

Write-Host ""

# Step 2: Check if server binary exists
Write-Host "[2/3] Checking server binary..." -ForegroundColor Yellow
$binaryPath = "target\release\golem-cli.exe"
if (-not (Test-Path $binaryPath)) {
    $binaryPath = "target\debug\golem-cli.exe"
}

if (-not (Test-Path $binaryPath)) {
    Write-Host "✗ Binary not found. Build it first with: cargo build --package golem-cli" -ForegroundColor Red
    Write-Host ""
    Write-Host "============================================" -ForegroundColor Cyan
    Write-Host "Test Summary" -ForegroundColor Cyan
    Write-Host "============================================" -ForegroundColor Cyan
    Write-Host "Unit tests: $(if ($unitTestExitCode -eq 0) {'PASSED'} else {'FAILED'})" -ForegroundColor $(if ($unitTestExitCode -eq 0) {'Green'} else {'Red'})
    Write-Host "Integration tests: SKIPPED (no binary)" -ForegroundColor Yellow
    exit 1
}

Write-Host "✓ Using binary: $binaryPath" -ForegroundColor Green
Write-Host ""

# Step 3: Run integration tests
Write-Host "[3/3] Running integration tests..." -ForegroundColor Yellow
Write-Host "Starting MCP server on port 13337..." -ForegroundColor Gray

$serverJob = Start-Job -ScriptBlock {
    param($path)
    Set-Location "C:\Users\matias.magni2\Documents\dev\mine\Algora\golem"
    & $path mcp-server start --port 13337 2>&1
} -ArgumentList (Resolve-Path $binaryPath).Path

# Wait for server to start
Write-Host "Waiting for server to be ready..." -ForegroundColor Gray
Start-Sleep -Seconds 3

# Check if server is running
try {
    $healthCheck = Invoke-WebRequest -Uri "http://127.0.0.1:13337" -UseBasicParsing -TimeoutSec 2 -ErrorAction Stop
    Write-Host "✓ Server is running" -ForegroundColor Green
} catch {
    Write-Host "✗ Server failed to start" -ForegroundColor Red
    Write-Host "Server output:"
    Receive-Job $serverJob
    Stop-Job $serverJob
    Remove-Job $serverJob
    
    Write-Host ""
    Write-Host "============================================" -ForegroundColor Cyan
    Write-Host "Test Summary" -ForegroundColor Cyan
    Write-Host "============================================" -ForegroundColor Cyan
    Write-Host "Unit tests: $(if ($unitTestExitCode -eq 0) {'PASSED'} else {'FAILED'})" -ForegroundColor $(if ($unitTestExitCode -eq 0) {'Green'} else {'Red'})
    Write-Host "Integration tests: FAILED (server didn't start)" -ForegroundColor Red
    exit 1
}

Write-Host ""

# Run integration tests
$integrationTestResult = cargo test --package golem-cli --test mcp_integration -- --ignored --test-threads=1 2>&1
$integrationTestExitCode = $LASTEXITCODE

if ($integrationTestExitCode -eq 0) {
    Write-Host "✓ Integration tests PASSED" -ForegroundColor Green
} else {
    Write-Host "✗ Integration tests FAILED" -ForegroundColor Red
    Write-Host $integrationTestResult
    $testsFailed = $true
}

# Stop server
Write-Host ""
Write-Host "Stopping server..." -ForegroundColor Gray
Stop-Job $serverJob
Remove-Job $serverJob

# Summary
Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Test Summary" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Unit tests: $(if ($unitTestExitCode -eq 0) {'PASSED'} else {'FAILED'})" -ForegroundColor $(if ($unitTestExitCode -eq 0) {'Green'} else {'Red'})
Write-Host "Integration tests: $(if ($integrationTestExitCode -eq 0) {'PASSED'} else {'FAILED'})" -ForegroundColor $(if ($integrationTestExitCode -eq 0) {'Green'} else {'Red'})
Write-Host ""

if ($testsFailed) {
    Write-Host "SOME TESTS FAILED" -ForegroundColor Red
    exit 1
} else {
    Write-Host "ALL TESTS PASSED ✓" -ForegroundColor Green
    exit 0
}
