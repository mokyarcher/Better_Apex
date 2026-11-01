param([string]$RepoRoot = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$repoPath = [IO.Path]::GetFullPath($RepoRoot)
$targetDir = Join-Path $repoPath '.local/rust-target-gui'
$deliveryDir = Join-Path $repoPath '.local/BetterApex-Rust-AutoRecord'
$deliveryExe = Join-Path $deliveryDir 'BetterApex-Rust-AutoRecord.exe'
$previousTarget = $env:CARGO_TARGET_DIR
# Offline preflight: do not publish an executable without its selectable model.
& (Join-Path $PSScriptRoot 'Prepare-PpOcr.ps1') -RepoRoot $repoPath
try {
    $env:CARGO_TARGET_DIR = $targetDir
    Push-Location (Join-Path $repoPath 'rust-app')
    try {
        # Tauri builds embedded frontend assets and enables the production
        # protocol. Plain cargo --release without that feature uses devUrl.
        & npm.cmd run tauri -- build --no-bundle --features custom-protocol -- --offline
        if ($LASTEXITCODE -ne 0) { throw 'Portable Release build failed; existing delivery was not replaced.' }
    } finally {
        Pop-Location
    }
    New-Item -ItemType Directory -Force -Path $deliveryDir | Out-Null
    $compiledExe = Join-Path $targetDir 'release/better-apex-lab.exe'
    Copy-Item -LiteralPath $compiledExe -Destination $deliveryExe
    Copy-Item -LiteralPath (Join-Path $repoPath 'rust-app/src-tauri/resources') -Destination $deliveryDir -Recurse -Force
    if ((Get-FileHash -LiteralPath $compiledExe).Hash -ne (Get-FileHash -LiteralPath $deliveryExe).Hash) {
        throw 'Copied executable hash mismatch.'
    }
    Write-Output ('Updated portable application: ' + $deliveryExe)
    Write-Output 'No ZIP or installer generated. Verify startup with the frontend dev server stopped.'
} finally {
    $env:CARGO_TARGET_DIR = $previousTarget
}
