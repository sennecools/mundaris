#Requires -Version 5.1
<#
.SYNOPSIS
Run the complete locked quality matrix, separately from the fast developer check.
.DESCRIPTION
GPU regressions require -IncludeGpu and an adapter. Native window/operator checks
remain separate. Historical acceptance/evidence scripts are preserved unchanged.
#>
[CmdletBinding()]
param(
    [string] $OutputDirectory,
    [switch] $IncludeGpu,
    [ValidateSet('format','workspace-check','clippy-all-features','clippy-default','tests-debug','tests-release','rustdoc','long-orbits','native_close_surface','native_full_frame','developer_interface','developer_bridge','developer_scenarios')]
    [string[]] $Only = @()
)
$ErrorActionPreference = 'Stop'
if (!$IncludeGpu -and @($Only | Where-Object { $_ -in @('native_close_surface','native_full_frame','developer_interface','developer_scenarios') }).Count -gt 0) {
    throw 'Selected GPU checks require -IncludeGpu; refusing an empty/unexecuted selection.'
}
$repo = Split-Path -Parent $PSScriptRoot
if (-not $OutputDirectory) { $OutputDirectory = Join-Path $repo ('../target/full-validation/' + [DateTime]::UtcNow.ToString('yyyyMMdd-HHmmss-fff')) }
$root = [IO.Path]::GetFullPath($OutputDirectory)
if (Test-Path -LiteralPath $root) {
    if (@(Get-ChildItem -LiteralPath $root -Force).Count -gt 0) { throw "Refusing to replace evidence: $root" }
}
$null = New-Item -ItemType Directory -Path $root -Force
$script:results = @()
function Run-Check([string] $Name, [string[]] $Arguments) {
    if ($Only.Count -gt 0 -and $Name -notin $Only) { return }
    $timer = [Diagnostics.Stopwatch]::StartNew()
    $start = [DateTime]::UtcNow
    $code = 1
    $previous = $ErrorActionPreference
    try {
        $cargo = (Get-Command cargo -CommandType Application -ErrorAction Stop | Select-Object -First 1).Source
        $ErrorActionPreference = 'Continue'
        $output = @(& $cargo @Arguments 2>&1 | ForEach-Object { $_.ToString() })
        $code = $LASTEXITCODE
    } catch { $output = @($_.Exception.Message) }
    finally { $ErrorActionPreference = $previous }
    $timer.Stop()
    $command = 'cargo ' + ($Arguments -join ' ')
    [IO.File]::WriteAllLines((Join-Path $root "$Name.txt"), [string[]](@($command) + $output), [Text.UTF8Encoding]::new($false))
    $script:results += [pscustomobject]@{ name=$Name; command=$command; exit_code=$code; duration_seconds=$timer.Elapsed.TotalSeconds; result=$(if ($code -eq 0) { 'pass' } else { 'fail' }); started_utc=$start.ToString('o') }
    Write-Output "$Name exit_code=$code duration=$($timer.Elapsed.TotalSeconds.ToString('F2'))s"
}
Push-Location $repo
try {
    $git = & git status --short
    if ($LASTEXITCODE -ne 0) { throw 'Git status failed' }
    [IO.File]::WriteAllLines((Join-Path $root 'git-status.txt'), [string[]]$git, [Text.UTF8Encoding]::new($false))
    $head = & git rev-parse HEAD
    if ($LASTEXITCODE -ne 0) { throw 'Git revision failed' }
    [IO.File]::WriteAllText((Join-Path $root 'head.txt'), ($head | Out-String), [Text.UTF8Encoding]::new($false))
    Run-Check 'format' @('fmt','--all','--','--check')
    Run-Check 'workspace-check' @('check','--locked','--workspace','--all-targets','--all-features')
    Run-Check 'clippy-all-features' @('clippy','--locked','--workspace','--all-targets','--all-features','--','-D','warnings')
    Run-Check 'clippy-default' @('clippy','--locked','--workspace','--all-targets','--','-D','warnings')
    Run-Check 'tests-debug' @('test','--locked','--workspace','--all-features')
    Run-Check 'tests-release' @('test','--locked','--release','--workspace','--all-features')
    $oldFlags = $env:RUSTDOCFLAGS
    try { $env:RUSTDOCFLAGS='-D warnings'; Run-Check 'rustdoc' @('doc','--locked','--workspace','--all-features','--no-deps') }
    finally { $env:RUSTDOCFLAGS=$oldFlags }
    Run-Check 'long-orbits' @('test','--locked','--release','-p','mundaris_simulation','--test','orbits','--','--ignored','--nocapture')
    Run-Check 'developer_bridge' @('test','--locked','--release','-p','mundaris_app','--features','developer-tools','--test','developer_bridge','--','--nocapture')
    if ($IncludeGpu) {
        foreach ($test in @('native_close_surface','native_full_frame','developer_interface','developer_scenarios')) {
            Run-Check $test @('test','--locked','--release','-p','mundaris_app','--all-features','--test',$test,'--','--ignored','--nocapture')
        }
    }
} finally {
    Pop-Location
    [IO.File]::WriteAllText((Join-Path $root 'validation.json'), (ConvertTo-Json -InputObject @($script:results) -Depth 5), [Text.UTF8Encoding]::new($false))
    Write-Output "Quality evidence: $root (selected checks: $($Only -join ','); native window checks are separate)"
}
if (@($script:results | Where-Object exit_code -ne 0).Count -gt 0) { exit 1 }
