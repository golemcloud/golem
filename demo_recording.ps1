# Demo Recording Script for MCP Server
# This script automates the demo with proper timing for recording

Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Golem CLI MCP Server Demo" -ForegroundColor Cyan
Write-Host "============================================" -ForegroundColor Cyan
Write-Host ""

# Check if binary exists
$binaryPath = "target\release\golem-cli.exe"
if (-not (Test-Path $binaryPath)) {
    Write-Host "ERROR: Binary not found at $binaryPath" -ForegroundColor Red
    Write-Host "Build it first with: cargo build --release --bin golem-cli" -ForegroundColor Yellow
    exit 1
}

Write-Host "[DEMO STEP 1] Binary Location" -ForegroundColor Green
Write-Host "Binary: $binaryPath" -ForegroundColor White
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "[DEMO STEP 2] Starting MCP Server..." -ForegroundColor Green
Write-Host "Command: .\$binaryPath mcp-server start --port 3000" -ForegroundColor White
Write-Host ""

# Start server in background
$serverJob = Start-Job -ScriptBlock {
    param($path)
    Set-Location "C:\Users\matias.magni2\Documents\dev\mine\Algora\golem"
    & $path mcp-server start --port 3000
} -ArgumentList (Resolve-Path $binaryPath).Path

Start-Sleep -Seconds 3

Write-Host "Server should be starting..." -ForegroundColor Yellow
Write-Host "Waiting for server to be ready..." -ForegroundColor Yellow
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "[DEMO STEP 3] Health Check" -ForegroundColor Green
Write-Host "Command: curl http://127.0.0.1:3000/" -ForegroundColor White
try {
    $health = Invoke-WebRequest -Uri "http://127.0.0.1:3000/" -UseBasicParsing
    Write-Host "Response: $($health.Content)" -ForegroundColor Cyan
} catch {
    Write-Host "Server not ready yet, waiting..." -ForegroundColor Yellow
    Start-Sleep -Seconds 3
    try {
        $health = Invoke-WebRequest -Uri "http://127.0.0.1:3000/" -UseBasicParsing
        Write-Host "Response: $($health.Content)" -ForegroundColor Cyan
    } catch {
        Write-Host "ERROR: Server failed to start" -ForegroundColor Red
        Stop-Job $serverJob
        Remove-Job $serverJob
        exit 1
    }
}
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "[DEMO STEP 4] List MCP Tools" -ForegroundColor Green
Write-Host "Command: curl tools/list" -ForegroundColor White
$toolsListBody = @{
    jsonrpc = "2.0"
    id = 2
    method = "tools/list"
    params = @{}
} | ConvertTo-Json

try {
    $toolsResponse = Invoke-RestMethod -Uri "http://127.0.0.1:3000/mcp" -Method Post -Body $toolsListBody -ContentType "application/json"
    Write-Host "Available Tools:" -ForegroundColor Cyan
    Write-Host ($toolsResponse | ConvertTo-Json -Depth 5) -ForegroundColor White
} catch {
    Write-Host "Response: $($_.Exception.Message)" -ForegroundColor Yellow
}
Start-Sleep -Seconds 3

Write-Host ""
Write-Host "[DEMO STEP 5] Call list_agent_types Tool" -ForegroundColor Green
$agentTypesBody = @{
    jsonrpc = "2.0"
    id = 3
    method = "tools/call"
    params = @{
        name = "list_agent_types"
        arguments = @{}
    }
} | ConvertTo-Json

try {
    $agentTypesResponse = Invoke-RestMethod -Uri "http://127.0.0.1:3000/mcp" -Method Post -Body $agentTypesBody -ContentType "application/json"
    Write-Host "Response:" -ForegroundColor Cyan
    Write-Host ($agentTypesResponse | ConvertTo-Json -Depth 5) -ForegroundColor White
} catch {
    Write-Host "Response: $($_.Exception.Message)" -ForegroundColor Yellow
}
Start-Sleep -Seconds 3

Write-Host ""
Write-Host "[DEMO STEP 6] Call list_components Tool" -ForegroundColor Green
$componentsBody = @{
    jsonrpc = "2.0"
    id = 4
    method = "tools/call"
    params = @{
        name = "list_components"
        arguments = @{}
    }
} | ConvertTo-Json

try {
    $componentsResponse = Invoke-RestMethod -Uri "http://127.0.0.1:3000/mcp" -Method Post -Body $componentsBody -ContentType "application/json"
    Write-Host "Response:" -ForegroundColor Cyan
    Write-Host ($componentsResponse | ConvertTo-Json -Depth 5) -ForegroundColor White
} catch {
    Write-Host "Response: $($_.Exception.Message)" -ForegroundColor Yellow
}
Start-Sleep -Seconds 2

Write-Host ""
Write-Host "============================================" -ForegroundColor Cyan
Write-Host "Demo Complete!" -ForegroundColor Green
Write-Host "Stopping server..." -ForegroundColor Yellow
Write-Host "============================================" -ForegroundColor Cyan

Stop-Job $serverJob
Remove-Job $serverJob

Write-Host ""
Write-Host "Press any key to exit..." -ForegroundColor Gray
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
