param([string]$RepoRoot = (Split-Path -Parent $PSScriptRoot))
$ErrorActionPreference = 'Stop'
$repoPath = [IO.Path]::GetFullPath($RepoRoot)
$resourceRoot = Join-Path $repoPath 'rust-app\src-tauri\resources'
New-Item -ItemType Directory -Force -Path (Join-Path $resourceRoot 'tessdata'),(Join-Path $resourceRoot 'x64'),(Join-Path $resourceRoot 'licenses') | Out-Null
foreach ($name in @('eng.traineddata','chi_sim.traineddata','LICENSE')) {
    Copy-Item -LiteralPath (Join-Path $repoPath ('BetterApex\Ocr\tessdata\'+$name)) -Destination (Join-Path $resourceRoot 'tessdata')
}
$nativeRoot = Join-Path $repoPath '.local\packages\tesseract\5.2.0\x64'
foreach ($name in @('tesseract50.dll','leptonica-1.82.0.dll')) {
    $sourcePath = Join-Path $nativeRoot $name
    if (!(Test-Path -LiteralPath $sourcePath)) { throw ('Native resource missing: '+$sourcePath+'. Restore the existing Tesseract NuGet package first.') }
    Copy-Item -LiteralPath $sourcePath -Destination (Join-Path $resourceRoot 'x64')
}
$vueLicense = Join-Path $repoPath 'rust-app\node_modules\vue\LICENSE'
if (Test-Path -LiteralPath $vueLicense) { Copy-Item -LiteralPath $vueLicense -Destination (Join-Path $resourceRoot 'licenses\Vue-MIT.txt') }
# Include the licenses present in cached package sources; deduplicate full text
# while retaining each package and its source URL in a manifest.
$cargoRoot = Join-Path $env:USERPROFILE '.cargo\registry\src'
$registry = Get-ChildItem -LiteralPath $cargoRoot -Directory | Select-Object -First 1 -ExpandProperty FullName
$lock = [IO.File]::ReadAllText((Join-Path $repoPath 'rust-app\src-tauri\Cargo.lock'))
$licenseTexts = @{}; $manifest = [Collections.Generic.List[string]]::new()
foreach ($entry in ($lock -split '\[\[package\]\]')) {
    if ($entry -notmatch '(?m)^name = "([^"]+)"') {continue}; $name=$Matches[1]
    if ($entry -notmatch '(?m)^version = "([^"]+)"') {continue}; $version=$Matches[1]
    $crate = Join-Path $registry ($name+'-'+$version)
    if (!(Test-Path -LiteralPath $crate)) {continue}
    $manifest.Add($name+' '+$version+' https://crates.io/crates/'+$name+'/'+$version)
    foreach ($license in (Get-ChildItem -LiteralPath $crate -File | Where-Object { $_.Name -match '^(LICENSE|LICENCE|COPYING|NOTICE)([._-]|$)' })) {
        $contents=[IO.File]::ReadAllText($license.FullName)
        $digest=[Convert]::ToHexString([Security.Cryptography.SHA256]::HashData([Text.Encoding]::UTF8.GetBytes($contents)))
        if (!$licenseTexts.ContainsKey($digest)) { $licenseTexts[$digest]=$name+' '+$version+' / '+$license.Name+"`n"+$contents }
    }
}
$output = "Cargo package sources and bundled license texts. Includes build-time packages for completeness.`n`n" + ($manifest -join "`n") + "`n`n" + (($licenseTexts.GetEnumerator() | Sort-Object Name | ForEach-Object { $_.Value }) -join "`n`n------------------------------------------------------------`n`n")
[IO.File]::WriteAllText((Join-Path $resourceRoot 'licenses\Cargo-Notices.txt'),$output,[Text.Encoding]::UTF8)
Write-Output ('Prepared x64 OCR resources and license notices: '+$resourceRoot)
