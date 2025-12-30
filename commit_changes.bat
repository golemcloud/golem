@echo off
cd /d "C:\Users\matias.magni2\Documents\dev\mine\Algora\golem"

set GIT="C:\Program Files\Git\bin\git.exe"

echo Checking git status...
%GIT% status

echo.
echo Adding modified files...
%GIT% add cli/golem-cli/src/command_handler/mcp_server.rs

echo.
echo Committing changes...
%GIT% commit -m "fix: resolve compilation error in MCP server implementation" -m "" -m "- Remove invalid .into_service() call on StreamableHttpService" -m "- Remove unused imports (stdin, stdout, ServiceExt)" -m "- StreamableHttpService already implements the service trait required by Axum" -m "" -m "This fixes the compilation error:" -m "error[E0599]: no method named 'into_service' found for struct 'StreamableHttpService<S, M>'" -m "" -m "The StreamableHttpService type from rmcp 0.12.0 can be used directly with" -m "Axum's nest_service() without conversion."

echo.
echo Done!
%GIT% log -1 --stat
