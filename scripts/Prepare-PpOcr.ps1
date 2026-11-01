param(
    [string]$RepoRoot = (Split-Path -Parent $PSScriptRoot),
    [switch]$DownloadMissing
)
$ErrorActionPreference = 'Stop'
$repoPath = [IO.Path]::GetFullPath($RepoRoot)
$resourceDir = Join-Path $repoPath 'rust-app/src-tauri/resources/ppocr-v6-small'
$cacheDir = Join-Path $repoPath '.local/ppocr-download'
$manifestPath = Join-Path $PSScriptRoot 'ppocr-assets.json'
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
New-Item -ItemType Directory -Force -Path $resourceDir,$cacheDir | Out-Null

function Test-AssetHash([string]$Path,[string]$Expected) {
    return (Test-Path -LiteralPath $Path -PathType Leaf) -and ((Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash -eq $Expected)
}
function Get-VerifiedFile([string]$Url,[string]$Destination,[string]$Expected) {
    if (Test-AssetHash $Destination $Expected) { return }
    if (!$DownloadMissing) { throw ('OCR resource missing or checksum mismatch: '+$Destination+'. Run scripts/Prepare-PpOcr.ps1 -DownloadMissing to restore it.') }
    $partial = $Destination + '.download'
    & curl.exe --location --fail --max-time 240 $Url --output $partial
    if ($LASTEXITCODE -ne 0) { throw ('Download failed: '+$Url) }
    if (!(Test-AssetHash $partial $Expected)) { throw ('Downloaded resource checksum mismatch: '+$Url) }
    Move-Item -LiteralPath $partial -Destination $Destination -Force
}

$archive = $null
try {
    foreach ($asset in $manifest.assets) {
        $destination = Join-Path $resourceDir $asset.name
        if (Test-AssetHash $destination $asset.sha256) { continue }
        if ($asset.archive_entry) {
            if (!$archive) {
                $package = Join-Path $cacheDir 'onnxruntime.nupkg'
                Get-VerifiedFile $manifest.runtime.url $package $manifest.runtime.sha256
                Add-Type -AssemblyName System.IO.Compression.FileSystem
                $archive = [IO.Compression.ZipFile]::OpenRead($package)
            }
            $entry = $archive.GetEntry($asset.archive_entry)
            if (!$entry) { throw ('Missing runtime package entry: '+$asset.archive_entry) }
            [IO.Compression.ZipFileExtensions]::ExtractToFile($entry,$destination,$true)
            if (!(Test-AssetHash $destination $asset.sha256)) { throw ('Extracted resource checksum mismatch: '+$asset.name) }
        } else {
            Get-VerifiedFile $asset.url $destination $asset.sha256
        }
    }
} finally {
    if ($archive) { $archive.Dispose() }
}
Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $resourceDir 'manifest.json') -Force
Write-Output 'Verified PP-OCRv6 small models, ONNX Runtime x64 and license files.'
