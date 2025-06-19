@echo off
mkdir data 2>nul
mkdir logs 2>nul

setlocal enabledelayedexpansion

start /B redis-server --port 6380 --save "" --appendonly no >> logs\redis.log 2>&1
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq redis-server.exe" /FO CSV ^| findstr /I "redis-server.exe"') do set REDIS_PID=%%A

set RUST_BACKTRACE=1
if not defined GOLEM__TRACING__FILE_DIR set GOLEM__TRACING__FILE_DIR=..\logs
if not defined GOLEM__TRACING__FILE__ANSI set GOLEM__TRACING__FILE__ANSI=true
if not defined GOLEM__TRACING__FILE__ENABLED set GOLEM__TRACING__FILE__ENABLED=true
if not defined GOLEM__TRACING__FILE__JSON set GOLEM__TRACING__FILE__JSON=false
if not defined GOLEM__TRACING__STDOUT__ENABLED set GOLEM__TRACING__STDOUT__ENABLED=false

cd /d golem-shard-manager || exit /b
start /B ../target/debug/golem-shard-manager
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq golem-shard-manager.exe" /FO CSV ^| findstr /I "golem-shard-manager.exe"') do set shard_manager_pid=%%A
cd ..

cd /d golem-component-compilation-service || exit /b
start /B ../target/debug/golem-component-compilation-service
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq golem-component-compilation-service.exe" /FO CSV ^| findstr /I "golem-component-compilation-service.exe"') do set component_compilation_service_pid=%%A
cd ..

cd /d golem-component-service || exit /b
start /B ../target/debug/golem-component-service
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq golem-component-service.exe" /FO CSV ^| findstr /I "golem-component-service.exe"') do set component_service_pid=%%A
cd ..

cd /d golem-worker-service || exit /b
start /B ../target/debug/golem-worker-service
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq golem-worker-service.exe" /FO CSV ^| findstr /I "golem-worker-service.exe"') do set worker_service_pid=%%A
cd ..

cd /d golem-worker-executor || exit /b
start /B ../target/debug/worker-executor
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq worker-executor.exe" /FO CSV ^| findstr /I "worker-executor.exe"') do set worker_executor_pid=%%A
cd ..

set "WORKSPACE_DIR=%CD%"
cd /d C:\dev\nginx || exit /b
start /B nginx.exe -c "%WORKSPACE_DIR%\golem-router\golem-services.local.conf" >> "%WORKSPACE_DIR%\logs\nginx.log" 2>&1
timeout /t 2 >nul
for /f "tokens=2 delims=," %%A in ('tasklist /FI "IMAGENAME eq nginx.exe" /FO CSV ^| findstr /I "nginx.exe"') do set ROUTER_PID=%%A
cd /d %WORKSPACE_DIR%

echo Started services
echo  - worker executor:               %worker_executor_pid%
echo  - worker service:                %worker_service_pid%
echo  - component service:             %component_service_pid%
echo  - component compilation service: %component_compilation_service_pid%
echo  - shard manager:                 %shard_manager_pid%
echo  - router:                        %router_pid%
echo  - redis:                         %redis_pid%
echo.

echo Kill all manually:
echo taskkill /F /PID %worker_executor_pid% %worker_service_pid% %component_service_pid% %component_compilation_service_pid% %shard_manager_pid% %router_pid% %redis_pid%

lnav logs

for %%P in (%worker_executor_pid% %worker_service_pid% %component_service_pid% %component_compilation_service_pid% %shard_manager_pid% %router_pid% %redis_pid%) do (
    if not "%%P"=="" (
        taskkill /F /PID %%P 2>nul
    )
)

endlocal
