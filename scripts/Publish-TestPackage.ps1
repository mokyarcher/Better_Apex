param([string]$RepoRoot = (Split-Path -Parent $PSScriptRoot))

$ErrorActionPreference = 'Stop'
$repoPath = [IO.Path]::GetFullPath($RepoRoot)
$env:DOTNET_CLI_HOME = Join-Path $repoPath '.local\dotnet'
$env:DOTNET_CLI_TELEMETRY_OPTOUT = '1'
$packageName = 'BetterApex-auto-record-dxgi-2s-' + (Get-Date -Format 'yyyyMMdd-HHmmss')
$artifactRoot = Join-Path $repoPath 'artifacts'
$packagePath = Join-Path $artifactRoot $packageName
$zipPath = Join-Path $artifactRoot ($packageName + '.zip')
if (Test-Path -LiteralPath $packagePath) { throw 'Package directory already exists. Retry with a new timestamp.' }

& dotnet publish (Join-Path $repoPath 'BetterApex\BetterApex.csproj') --configuration Release `
    --runtime win-x64 --self-contained true --output $packagePath `
    -p:PublishSingleFile=false -p:PublishTrimmed=false -p:NuGetAudit=false
if ($LASTEXITCODE -ne 0) { throw 'Publishing failed; no test ZIP was created.' }

Copy-Item -LiteralPath (Join-Path $repoPath 'docs\自动记录测试说明.md') -Destination (Join-Path $packagePath '测试说明.md')
$launcher = @'
@echo off
setlocal
cd /d "%~dp0"
set "BETTER_APEX_DATA_DIR=%~dp0data"
start "" "%~dp0BetterApex.exe"
'@
Set-Content -LiteralPath (Join-Path $packagePath '启动.cmd') -Value $launcher -Encoding Ascii
Set-Content -LiteralPath (Join-Path $packagePath '版本.txt') -Encoding UTF8 -Value @"
Better Apex - automatic match recording test
Built: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss zzz')
Target: Windows 10 2004+ / Windows 11, x64, .NET self-contained
Scope: in-match detection, frozen roster, settlement detection, automatic OCR/association, SQLite saving and pending-job retry.
Capture: DXGI monitor duplication, GPU crop to selected client, no WGC yellow border; foreground only.
Pacing: <=1 normal acquisition/2sec; one settlement verification >=500ms apart, >=5sec burst cooldown; no catch-up.
Performance: BelowNormal capture/OCR workers, reusable GPU resources, no continuous PNG writes; real-game FPS testing still required.
Compatibility: explicit WGC checkbox retained; Windows 10 may show yellow border in that mode.
"@
foreach ($relative in @('BetterApex.exe', 'coreclr.dll', 'PresentationFramework.dll', 'WinRT.Runtime.dll',
    'Ocr\tessdata\eng.traineddata', 'Ocr\tessdata\chi_sim.traineddata', 'x64\tesseract50.dll', 'e_sqlite3.dll',
    'Vortice.Direct3D11.dll', 'Vortice.DXGI.dll', 'Vortice.Mathematics.dll', 'SharpGen.Runtime.dll', 'THIRD-PARTY-NOTICES.txt')) {
    if (!(Test-Path -LiteralPath (Join-Path $packagePath $relative))) { throw "Missing package file: $relative" }
}
if (Test-Path -LiteralPath (Join-Path $packagePath 'data')) { throw 'Fresh package must not contain user match data.' }
Compress-Archive -LiteralPath $packagePath -DestinationPath $zipPath -CompressionLevel Optimal
Get-Item -LiteralPath $zipPath | Select-Object FullName, Length
Get-FileHash -LiteralPath $zipPath -Algorithm SHA256 | Format-List
