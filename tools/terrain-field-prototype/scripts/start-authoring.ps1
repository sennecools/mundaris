#Requires -Version 7.0
[CmdletBinding()]
param(
    [ValidateRange(1024, 65535)][int] $Port = 4179,
    [string] $OutputRoot = 'D:/Astrum/target/terrain-authoring/exports',
    [switch] $NoBuild,
    [switch] $Background
)

$ErrorActionPreference = 'Stop'
$packageRoot = Split-Path -Parent $PSScriptRoot
$buildRoot = 'D:/Astrum/target/procedural-terrain-data'
$env:CARGO_TARGET_DIR = $buildRoot
$binaryPath = Join-Path $buildRoot 'debug/terrain-field-prototype.exe'
$manifestPath = Join-Path $packageRoot 'Cargo.toml'
$outputPath = [System.IO.Path]::GetFullPath($OutputRoot)
if (Get-NetTCPConnection -LocalPort $Port -State Listen -ErrorAction SilentlyContinue) {
    throw "Port $Port is already in use. Open its existing service or choose another -Port."
}

if (-not $NoBuild) {
    & cargo build --locked --manifest-path $manifestPath
    if ($LASTEXITCODE -ne 0) { throw 'The authoring tool build failed.' }
}
if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
    throw 'Authoring binary is missing; run without -NoBuild.'
}
New-Item -ItemType Directory -Path $outputPath -Force | Out-Null
$url = "http://127.0.0.1:$Port/"
if (-not $Background) {
    Write-Host "Terrain authoring: $url"
    & $binaryPath serve $outputPath $Port
    if ($LASTEXITCODE -ne 0) { throw 'The authoring service stopped with an error.' }
    return
}

$logRoot = 'D:/Astrum/target/terrain-authoring/logs'
New-Item -ItemType Directory -Path $logRoot -Force | Out-Null
$runId = [System.Guid]::NewGuid().ToString('N')
$stdoutPath = Join-Path $logRoot "$runId.stdout.log"
$stderrPath = Join-Path $logRoot "$runId.stderr.log"
$process = Start-Process -FilePath $binaryPath -ArgumentList @('serve', ('"' + $outputPath + '"'), $Port.ToString()) -WorkingDirectory 'D:/Astrum' -WindowStyle Hidden -RedirectStandardOutput $stdoutPath -RedirectStandardError $stderrPath -PassThru
$receipt = [pscustomobject]@{
    pid = $process.Id
    binary = $binaryPath
    binary_sha256 = (Get-FileHash -LiteralPath $binaryPath).Hash
    url = $url
    output_root = $outputPath
    stdout = $stdoutPath
    stderr = $stderrPath
}
$receiptPath = Join-Path $logRoot "$runId.receipt.json"
$receipt | ConvertTo-Json | Set-Content -LiteralPath $receiptPath
for ($attempt = 0; $attempt -lt 30; $attempt++) {
    if ($process.HasExited) {
        throw "Authoring service exited. Inspect $stderrPath"
    }
    try {
        $catalog = Invoke-RestMethod -Uri ($url + 'api/catalog') -TimeoutSec 2
        if ($catalog.templates.Count -gt 0) {
            Write-Host "Terrain authoring: $url"
            Write-Host "Process receipt: $receiptPath"
            return
        }
    } catch { }
    Start-Sleep -Milliseconds 250
}
throw "Service readiness was not confirmed. Inspect $receiptPath and its logs."
