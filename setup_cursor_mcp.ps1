# Setup script for Golem MCP Server in Cursor
# This script configures Cursor to use the Golem CLI MCP server

$ErrorActionPreference = "Stop"

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Setting up Golem MCP Server in Cursor" -ForegroundColor Yellow
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""

# Get paths
$projectRoot = "C:\Users\matias.magni2\Documents\dev\mine\Algora\golem"
$binaryPath = Join-Path $projectRoot "target\debug\golem-cli.exe"
$mcpConfigPath = "$env:APPDATA\Cursor\User\globalStorage\saoudrizwan.claude-dev\settings\cline_mcp_settings.json"

# Verify binary exists
if (-not (Test-Path $binaryPath)) {
    Write-Host "Binary not found. Building..." -ForegroundColor Yellow
    Push-Location $projectRoot
    cargo build --package golem-cli
    Pop-Location
    
    if (-not (Test-Path $binaryPath)) {
        Write-Host "ERROR: Failed to build binary" -ForegroundColor Red
        exit 1
    }
}

Write-Host "Binary found: $binaryPath" -ForegroundColor Green

# Create MCP config directory if needed
$mcpDir = Split-Path $mcpConfigPath -Parent
if (-not (Test-Path $mcpDir)) {
    New-Item -ItemType Directory -Path $mcpDir -Force | Out-Null
    Write-Host "Created MCP config directory" -ForegroundColor Green
}

# Read or create MCP config
$mcpConfig = @{}
if (Test-Path $mcpConfigPath) {
    try {
        $mcpConfig = Get-Content $mcpConfigPath -Raw | ConvertFrom-Json -AsHashtable
        Write-Host "Loaded existing MCP config" -ForegroundColor Green
    } catch {
        Write-Host "Could not parse existing config, creating new one" -ForegroundColor Yellow
        $mcpConfig = @{}
    }
} else {
    Write-Host "Creating new MCP config" -ForegroundColor Yellow
}

# Add Golem MCP server configuration
# Cursor uses command-based MCP servers
$mcpConfig["golem-cli"] = @{
    command = $binaryPath
    args = @("mcp-server", "start", "--host", "127.0.0.1", "--port", "3000")
}

# Save config
$mcpConfig | ConvertTo-Json -Depth 10 | Out-File $mcpConfigPath -Encoding UTF8
Write-Host "MCP configuration saved to: $mcpConfigPath" -ForegroundColor Green
Write-Host ""

# Display configuration
Write-Host "Configuration:" -ForegroundColor Cyan
Write-Host "  Server: golem-cli" -ForegroundColor White
Write-Host "  Command: $binaryPath" -ForegroundColor White
Write-Host "  Args: mcp-server start --host 127.0.0.1 --port 3000" -ForegroundColor White
Write-Host ""

Write-Host "========================================" -ForegroundColor Cyan
Write-Host "Setup Complete!" -ForegroundColor Green
Write-Host "========================================" -ForegroundColor Cyan
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "1. Restart Cursor to load the MCP configuration" -ForegroundColor White
Write-Host "2. The MCP server will start automatically when Cursor connects" -ForegroundColor White
Write-Host "3. You can test it by asking Cursor to use Golem tools" -ForegroundColor White
Write-Host ""
