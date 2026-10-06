#Requires -Version 5.1
<#
.SYNOPSIS
Record source identity and preservation differences for the resident tile proof.
.DESCRIPTION
Reads the initial manifest saved before implementation. Does not run acceptance
gates or interpret screenshots. Requested allocations are not measured VRAM.
#>
[CmdletBinding()]
param(
    [string] $EvidenceRoot = 'target/terrain-redesign/slice2a',
    [string] $Label = 'final'
)
$ErrorActionPreference = 'Stop'
$repoRoot = Split-Path -Parent $PSScriptRoot
Push-Location $repoRoot
try {
    $root = [IO.Path]::GetFullPath($EvidenceRoot)
    if ($Label -notmatch '^[A-Za-z0-9_-]+$') { throw 'Invalid evidence label' }
    $initialPath = Join-Path $root 'initial-files.json'
    $initial = Get-Content -LiteralPath $initialPath -Raw | ConvertFrom-Json
    $sourcePaths = @(& git ls-files --cached --others --exclude-standard -- Cargo.toml Cargo.lock rust-toolchain.toml crates)
    if ($LASTEXITCODE -ne 0) { throw 'Source discovery failed' }
    $source = @($sourcePaths | Sort-Object -Unique | ForEach-Object {
        if (Test-Path -LiteralPath $_ -PathType Leaf) {
            [pscustomobject]@{ path=$_; sha256=(Get-FileHash -LiteralPath $_ -Algorithm SHA256).Hash }
        }
    })
    $differences = @(foreach ($entry in $initial) {
        $current = if (Test-Path -LiteralPath $entry.path -PathType Leaf) {
            (Get-FileHash -LiteralPath $entry.path -Algorithm SHA256).Hash
        } else { $null }
        if ($current -ne $entry.sha256) {
            [pscustomobject]@{ path=$entry.path; initial_sha256=$entry.sha256; current_sha256=$current }
        }
    })
    $binaries = @()
    foreach ($releaseRoot in @('target/release','target/slice2a-validation/release')) {
        $binaries += @(Get-ChildItem -LiteralPath $releaseRoot -Filter '*.exe' -File -ErrorAction SilentlyContinue |
            Where-Object Name -In @('mundaris_app.exe','mundaris_dev.exe') | ForEach-Object {
                [pscustomobject]@{ path=$_.FullName; sha256=(Get-FileHash -LiteralPath $_.FullName -Algorithm SHA256).Hash }
            })
        foreach ($exampleName in @('resident_tile_capture','resident_hierarchy_capture','resident_tile_samples')) {
            $example = "$releaseRoot/examples/$exampleName.exe"
            if (Test-Path -LiteralPath $example) {
                $binaries += [pscustomobject]@{ path=[IO.Path]::GetFullPath($example); sha256=(Get-FileHash -LiteralPath $example -Algorithm SHA256).Hash }
            }
        }
    }
    $utf8 = New-Object Text.UTF8Encoding($false)
    foreach ($item in @(@('source-files',$source), @('preservation-differences',$differences), @('binaries',$binaries))) {
        [IO.File]::WriteAllText((Join-Path $root "$Label-$($item[0]).json"),
            (ConvertTo-Json -InputObject @($item[1]) -Depth 6), $utf8)
    }
    & git status --short | Set-Content (Join-Path $root "$Label-git-status.txt")
    & git rev-parse HEAD | Set-Content (Join-Path $root "$Label-head.txt")
    Write-Output "Recorded $($source.Count) source files and $($differences.Count) changed pre-existing paths. Review differences against task ownership."
} finally { Pop-Location }
