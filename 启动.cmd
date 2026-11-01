@echo off
cd /d "%~dp0"
set "DOTNET_CLI_HOME=%~dp0.local\dotnet"
set "DOTNET_CLI_TELEMETRY_OPTOUT=1"
set "DOTNET_SKIP_FIRST_TIME_EXPERIENCE=1"
set "BETTER_APEX_DATA_DIR=%~dp0data"
dotnet run --project "%~dp0BetterApex\BetterApex.csproj" --configuration Release --no-launch-profile -p:NuGetAudit=false -p:RestoreIgnoreFailedSources=true
if errorlevel 1 pause
